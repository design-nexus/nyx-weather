//! Radar & Overlays section: radar (the NOAA NEXRAD mosaic via IEM for US places,
//! RainViewer elsewhere) and infrared satellite over the basemap, and the current wind,
//! temperature and isobars from a grid of Open-Meteo points.
//!
//! The map is composed off the GTK thread at native resolution ([`crate::maprender`]):
//! one basemap picture and one picture per radar frame, all loaded (newest first) when
//! the page opens. Playback runs on the frame clock and cross-fades each frame into the
//! next.

use crate::charts::{self, MapScene};
use crate::maprender::{self, BasemapColors, MapGeom, Recolor};
use crate::widgets::{self, Page};
use crate::{backend, cmd, prefs, theme};
use gtk::prelude::*;
use gtk::{cairo, glib};
use nyx_weather_core::{
    FieldGrid, ForecastSnapshot, Location, OpenMeteoConfig, RAINVIEWER_MAX_ZOOM, RadarClient, RadarFrame,
    RadarLayerKind, RadarManifest, TileView, cardinal_direction, iem_radar_frames, iem_satellite_frame,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

thread_local! {
    /// The zoom last used, so a refresh (which rebuilds the page) keeps it.
    static ZOOM: Cell<u8> = const { Cell::new(6) };
}

/// Seconds of playback per 10 minutes of weather; 5-minute frames step twice as often.
const SECS_PER_TEN_MINUTES: f64 = 0.7;
/// How long the loop rests on the newest frame before starting again.
const HOLD_SECS: f64 = 1.5;

/// What the composed pictures were made for.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SceneKey {
    zoom: u8,
    layer: RadarLayerKind,
    w: i32,
    h: i32,
    scale: i32,
}

#[derive(Default)]
struct RadarViewState {
    layer: RadarLayerKind,
    zoom: u8,
    opacity: f64,
    manifest: Option<RadarManifest>,
    /// The frames the layer shows, oldest first (one for satellite, none for fields).
    frames: Vec<RadarFrame>,
    /// Where playback is, in frames; fractional while one fades into the next.
    pos: f64,
    playing: bool,
    /// Seconds left resting on the newest frame.
    hold: f64,
    ticking: bool,
    last_tick: Option<i64>,
    anim_phase: f64,
    pinch_scale: f64,
    scene: Option<SceneKey>,
    basemap: Option<cairo::ImageSurface>,
    /// Each frame's composed picture, by frame time.
    overlays: HashMap<i64, cairo::ImageSurface>,
    /// The wind/temperature/pressure grid, and the view it was fetched for.
    field: Option<(String, FieldGrid)>,
    /// What the map can't show here, drawn over it.
    notice: Option<String>,
    /// Bumped when the scene changes, so work for an older one is dropped.
    generation: u64,
    /// Frames composed so far, while loading.
    loading: Option<(usize, usize)>,
}

impl RadarViewState {
    /// The first frame of the run of composed frames that ends with the newest: the
    /// loop plays those while the older ones load.
    fn ready_start(&self) -> usize {
        let mut start = self.frames.len();
        while start > 0 && self.overlays.contains_key(&self.frames[start - 1].time) {
            start -= 1;
        }
        start.min(self.frames.len().saturating_sub(1))
    }

    /// The pictures to show at `pos`: the frame, the next one and the fade between
    /// them (none when reducing motion), falling back to the nearest composed frame.
    fn frames_at(&self, reduce_motion: bool) -> Option<(&cairo::ImageSurface, Option<&cairo::ImageSurface>, f64)> {
        let n = self.frames.len();
        if n == 0 {
            return None;
        }
        let i = (self.pos.floor().max(0.0) as usize).min(n - 1);
        let f = self.pos - i as f64;
        let get = |j: usize| self.frames.get(j).and_then(|fr| self.overlays.get(&fr.time));
        let a = get(i).or_else(|| (0..n).rev().filter_map(get).next())?;
        let b = (!reduce_motion && f > 0.001).then(|| get(i + 1)).flatten();
        Some((a, b, f))
    }

    /// The seconds of playback one frame takes.
    fn secs_per_frame(&self) -> f64 {
        let step = match self.frames.as_slice() {
            [a, b, ..] => (b.time - a.time).max(60) as f64,
            _ => 600.0,
        };
        SECS_PER_TEN_MINUTES * step / 600.0
    }

    fn advance(&mut self, dt: f64) {
        let n = self.frames.len();
        if n < 2 || self.overlays.is_empty() {
            return;
        }
        let start = self.ready_start() as f64;
        if self.hold > 0.0 {
            self.hold -= dt;
            if self.hold <= 0.0 {
                self.pos = start;
            }
            return;
        }
        self.pos = (self.pos + dt / self.secs_per_frame()).max(start);
        if self.pos >= (n - 1) as f64 {
            self.pos = (n - 1) as f64;
            self.hold = HOLD_SECS;
        }
    }
}

/// True for a tile layer (radar or satellite) rather than a field drawn from points.
fn is_tile_layer(layer: RadarLayerKind) -> bool {
    matches!(layer, RadarLayerKind::PrecipitationRadar | RadarLayerKind::InfraredSatellite)
}

/// Everything the page's handlers share.
struct Ctx {
    state: Rc<RefCell<RadarViewState>>,
    canvas: gtk::DrawingArea,
    loc: Location,
    cfg: OpenMeteoConfig,
    utc_offset: i64,
    play_bar: gtk::Box,
    play_btn: gtk::Button,
    frame_slider: gtk::Scale,
    frame_time_lbl: gtk::Label,
    opacity: gtk::Box,
    sub_lbl: gtk::Label,
    /// The frames the timeline's marks were made for: how many, and the first's time.
    marked: Cell<(usize, i64)>,
    /// Set while the slider is moved by playback, not the user.
    syncing: Cell<bool>,
    resize_timer: RefCell<Option<glib::SourceId>>,
}

impl Ctx {
    /// The closest zoom this layer allows here: NEXRAD reaches street level in the US;
    /// elsewhere radar stops where RainViewer does.
    fn max_zoom(&self) -> u8 {
        if self.state.borrow().layer == RadarLayerKind::PrecipitationRadar && !self.loc.is_us() {
            RAINVIEWER_MAX_ZOOM
        } else {
            10
        }
    }

    fn set_zoom(self: &Rc<Self>, z: u8) {
        let z = z.clamp(4, self.max_zoom());
        {
            let mut st = self.state.borrow_mut();
            st.pinch_scale = 1.0;
            if st.zoom == z {
                return;
            }
            st.zoom = z;
        }
        ZOOM.with(|c| c.set(z));
        self.load_scene();
    }

    fn zoom_by(self: &Rc<Self>, by: i32) {
        let z = self.state.borrow().zoom as i32 + by;
        self.set_zoom(z.clamp(4, 12) as u8);
    }

    /// The layer's frames for this place, oldest first.
    fn frames_for(&self, layer: RadarLayerKind, manifest: Option<&RadarManifest>) -> Vec<RadarFrame> {
        match layer {
            RadarLayerKind::PrecipitationRadar if self.loc.is_us() => iem_radar_frames(backend::now_epoch()),
            RadarLayerKind::PrecipitationRadar => {
                manifest.map(|m| m.radar_frames.iter().filter(|f| !f.is_nowcast).cloned().collect()).unwrap_or_default()
            }
            RadarLayerKind::InfraredSatellite => vec![iem_satellite_frame(backend::now_epoch())],
            _ => Vec::new(),
        }
    }

    /// The data credits for what's on the map.
    fn credit(&self, layer: RadarLayerKind) -> &'static str {
        match layer {
            RadarLayerKind::PrecipitationRadar if self.loc.is_us() => "© OpenStreetMap contributors · Radar: NOAA via IEM",
            RadarLayerKind::PrecipitationRadar => "© OpenStreetMap contributors · Radar: RainViewer",
            RadarLayerKind::InfraredSatellite => "© OpenStreetMap contributors · Satellite: NOAA GOES via IEM",
            _ => "© OpenStreetMap contributors · Data: Open-Meteo",
        }
    }

    /// What the layer shows, short, after the place in the page bar.
    fn summary(&self, layer: RadarLayerKind) -> &'static str {
        match layer {
            RadarLayerKind::PrecipitationRadar if self.loc.is_us() => "a frame every 5 minutes",
            RadarLayerKind::PrecipitationRadar => "a frame every 10 minutes",
            RadarLayerKind::InfraredSatellite => "the latest infrared image",
            RadarLayerKind::WindStreamlines => "the wind now",
            _ => "the temperature and pressure now",
        }
    }

    fn description(&self, layer: RadarLayerKind) -> &'static str {
        match layer {
            RadarLayerKind::PrecipitationRadar if self.loc.is_us() => {
                "The NOAA NEXRAD mosaic via IEM: a frame every 5 minutes for the past hour, on the classic NWS scale (dBZ)."
            }
            RadarLayerKind::PrecipitationRadar => {
                "RainViewer's radar composite: a frame every 10 minutes for the past two hours, on the classic NWS scale (dBZ)."
            }
            other => other.description(),
        }
    }

    /// The canvas size to compose for, and its device scale.
    fn canvas_size(&self) -> (i32, i32, i32) {
        let (w, h) = (self.canvas.width(), self.canvas.height());
        let (w, h) = if w > 0 && h > 0 { (w, h) } else { (900, 520) };
        (w, h, self.canvas.scale_factor().max(1))
    }

    /// Compose the map for the current zoom, layer and size: the basemap, the frames
    /// (newest first) or the field. Does nothing when the pictures already fit.
    fn load_scene(self: &Rc<Self>) {
        let (w, h, scale) = self.canvas_size();
        let (key, generation, keep_basemap, layer, zoom) = {
            let mut st = self.state.borrow_mut();
            let key = SceneKey { zoom: st.zoom, layer: st.layer, w, h, scale };
            // A smaller canvas shows the middle of what's there.
            let fits = st.scene.is_some_and(|k| {
                k.zoom == key.zoom && k.layer == key.layer && k.scale == key.scale && w <= k.w && h <= k.h
            });
            if fits {
                drop(st);
                self.update_timeline();
                self.canvas.queue_draw();
                return;
            }
            let keep_basemap = st.basemap.is_some()
                && st.scene.is_some_and(|k| k.zoom == key.zoom && k.scale == key.scale && w <= k.w && h <= k.h);
            st.generation += 1;
            st.scene = Some(key);
            if !keep_basemap {
                st.basemap = None;
            }
            st.overlays.clear();
            st.frames = self.frames_for(st.layer, st.manifest.as_ref());
            st.pos = st.frames.len().saturating_sub(1) as f64;
            st.hold = 0.0;
            st.loading = None;
            st.notice = match st.layer {
                RadarLayerKind::PrecipitationRadar if st.zoom > RAINVIEWER_MAX_ZOOM && !self.loc.is_us() => {
                    Some("Radar this close in covers the US only — zoom out".to_string())
                }
                // The satellite imagery is GOES-East: the Americas.
                RadarLayerKind::InfraredSatellite if !(-170.0..=-20.0).contains(&self.loc.longitude) => {
                    Some("Satellite imagery isn't available here".to_string())
                }
                RadarLayerKind::PrecipitationRadar if st.frames.is_empty() => Some("Loading radar…".to_string()),
                _ => None,
            };
            (key, st.generation, keep_basemap, st.layer, st.zoom)
        };
        self.sub_lbl.set_text(&format!("{} · {}", self.loc.name, self.summary(layer)));
        self.sub_lbl.set_tooltip_text(Some(self.description(layer)));
        self.update_timeline();
        self.canvas.queue_draw();

        let (lat, lon) = (self.loc.latitude, self.loc.longitude);
        let geom = MapGeom::new(lat, lon, zoom, w as f64, h as f64, 1.0);
        if !keep_basemap {
            let base_z = if scale >= 2 { (zoom + 1).min(12) } else { zoom };
            let view = TileView::covering(lat, lon, zoom, base_z, w as f64, h as f64);
            self.load_basemap(generation, view, geom, scale as f64);
        }
        if is_tile_layer(layer) {
            let n = self.state.borrow().frames.len();
            if n > 0 {
                self.state.borrow_mut().loading = Some((0, n));
                self.update_timeline();
                self.load_frame(generation, geom, (0..n).rev().collect(), 0);
            }
        } else {
            self.load_field(TileView::covering(lat, lon, zoom, zoom, key.w as f64, key.h as f64));
        }
    }

    /// The basemap: what's cached at once, then the rest from the network.
    fn load_basemap(self: &Rc<Self>, generation: u64, view: TileView, geom: MapGeom, scale: f64) {
        let colors = Recolor::Basemap(BasemapColors::current());
        let me = self.clone();
        cmd::background(
            move || {
                let urls = RadarClient::basemap_urls(&view);
                let tiles = backend::cached_tiles(&urls);
                let complete = tiles.iter().zip(&urls).all(|(t, u)| t.is_some() || u.is_none());
                (maprender::compose(&view, &tiles, &geom, scale, &colors), complete)
            },
            move |(raster, complete)| {
                if me.state.borrow().generation != generation {
                    return;
                }
                if let Some(s) = raster.and_then(maprender::Raster::into_surface) {
                    me.state.borrow_mut().basemap = Some(s);
                    me.canvas.queue_draw();
                }
                if complete {
                    return;
                }
                let me2 = me.clone();
                cmd::background(
                    move || maprender::compose(&view, &backend::fetch_tiles(&RadarClient::basemap_urls(&view)), &geom, scale, &colors),
                    move |raster| {
                        if me2.state.borrow().generation != generation {
                            return;
                        }
                        if let Some(s) = raster.and_then(maprender::Raster::into_surface) {
                            me2.state.borrow_mut().basemap = Some(s);
                            me2.canvas.queue_draw();
                        }
                    },
                );
            },
        );
    }

    /// Frame `order[k]` (then the rest, one after another, so the newest shows first and
    /// a slow network never has more than one frame in flight).
    fn load_frame(self: &Rc<Self>, generation: u64, geom: MapGeom, order: Vec<usize>, k: usize) {
        let Some(&idx) = order.get(k) else { return };
        let (frame, host) = {
            let st = self.state.borrow();
            let Some(f) = st.frames.get(idx).cloned() else { return };
            (f, st.manifest.as_ref().map(|m| m.host.clone()).unwrap_or_default())
        };
        let view =
            TileView::covering(self.loc.latitude, self.loc.longitude, geom.z, frame.tile_zoom(geom.z), geom.w, geom.h);
        let recolor = match frame.palette() {
            Some(p) => Recolor::Radar(p),
            None => Recolor::infrared(),
        };
        let time = frame.time;
        let me = self.clone();
        cmd::background(
            move || {
                let tiles = backend::fetch_tiles(&RadarClient::overlay_urls(&view, &host, &frame));
                let empty = tiles.iter().all(Option::is_none);
                (maprender::compose(&view, &tiles, &geom, 1.0, &recolor), empty)
            },
            move |(raster, empty)| {
                {
                    let mut st = me.state.borrow_mut();
                    if st.generation != generation {
                        return;
                    }
                    if let Some(s) = raster.and_then(maprender::Raster::into_surface) {
                        st.overlays.insert(time, s);
                    } else if k == 0 && empty && st.notice.is_none() {
                        st.notice = Some("No radar or satellite tiles for this view".into());
                    }
                    let total = order.len();
                    st.loading = (k + 1 < total).then_some((k + 1, total));
                }
                me.update_timeline();
                me.canvas.queue_draw();
                me.load_frame(generation, geom, order, k + 1);
            },
        );
    }

    /// The wind and temperature grid over `view`, unless it's already there.
    fn load_field(self: &Rc<Self>, view: TileView) {
        let key = view.key();
        if self.state.borrow().field.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        self.state.borrow_mut().notice = Some("Loading the wind and temperature grid…".into());
        self.canvas.queue_draw();
        let (me, cfg) = (self.clone(), self.cfg.clone());
        cmd::background(
            move || backend::fetch_field(&view, &cfg),
            move |res| {
                let mut st = me.state.borrow_mut();
                match res {
                    Ok(grid) => {
                        st.field = Some((key, grid));
                        if !is_tile_layer(st.layer) {
                            st.notice = None;
                        }
                    }
                    Err(e) if !is_tile_layer(st.layer) => st.notice = Some(e.to_string()),
                    Err(_) => {}
                }
                drop(st);
                me.canvas.queue_draw();
                me.ensure_ticking();
            },
        );
    }

    /// The slider and the time beside it, following playback.
    fn update_timeline(&self) {
        let st = self.state.borrow();
        let n = st.frames.len();
        let tile = is_tile_layer(st.layer);
        // Only radar has frames to play; satellite keeps the time of its one image, and
        // the fields leave just the opacity.
        let plays = st.layer == RadarLayerKind::PrecipitationRadar;
        self.play_btn.set_visible(plays);
        self.frame_slider.set_visible(plays);
        self.frame_time_lbl.set_visible(tile);
        self.play_bar.set_halign(if plays { gtk::Align::Fill } else { gtk::Align::End });
        if plays {
            self.play_bar.remove_css_class("no-timeline");
        } else {
            self.play_bar.add_css_class("no-timeline");
        }
        if tile {
            self.opacity.remove_css_class("alone");
        } else {
            self.opacity.add_css_class("alone");
        }
        self.syncing.set(true);
        let first = st.frames.first().map_or(0, |f| f.time);
        if self.marked.get() != (n, first) {
            self.marked.set((n, first));
            self.mark_frames(&st.frames);
        }
        if tile && n > 1 {
            self.frame_slider.set_range(0.0, (n - 1) as f64);
            self.frame_slider.set_value(st.pos);
            self.frame_slider.set_sensitive(true);
        } else {
            self.frame_slider.set_sensitive(false);
        }
        self.syncing.set(false);
        let text = if let Some((done, total)) = st.loading {
            format!("Loading {done} of {total}")
        } else if tile && n > 0 {
            let i = (st.pos.floor().max(0.0) as usize).min(n - 1);
            let f = st.pos - i as f64;
            let t0 = st.frames[i].time;
            let t1 = st.frames.get(i + 1).map_or(t0, |fr| fr.time);
            let t = t0 + ((t1 - t0) as f64 * f) as i64;
            let local = nyx_weather_core::local_time(t, self.utc_offset as i32);
            let time = if prefs::get().use_24h_time { local.format("%H:%M") } else { local.format("%-I:%M %p") };
            format!("Observed · {time}")
        } else {
            st.layer.label().to_string()
        };
        if self.frame_time_lbl.text() != text {
            self.frame_time_lbl.set_text(&text);
        }
    }

    /// A tick under each frame, and the time under about every fourth (not the first,
    /// whose label would run off the timeline's start), the newest marked "now".
    fn mark_frames(&self, frames: &[RadarFrame]) {
        let s = &self.frame_slider;
        s.clear_marks();
        let n = frames.len();
        if n < 2 {
            return;
        }
        let every = n.div_ceil(5).max(1);
        let use_24h = prefs::get().use_24h_time;
        for (i, f) in frames.iter().enumerate() {
            let last = i + 1 == n;
            let labelled = last || (i > 0 && i % every == 0 && n - 1 - i > every / 2);
            let text = labelled.then(|| {
                let local = nyx_weather_core::local_time(f.time, self.utc_offset as i32);
                let t = local.format(if use_24h { "%H:%M" } else { "%-I:%M %p" }).to_string();
                if last {
                    format!("<span foreground=\"{}\" weight=\"600\">{t} · now</span>", theme::palette().accent)
                } else {
                    t
                }
            });
            s.add_mark(i as f64, gtk::PositionType::Bottom, text.as_deref());
        }
    }

    /// Run the frame clock while something moves: playback, or the wind arrows.
    fn ensure_ticking(self: &Rc<Self>) {
        {
            let mut st = self.state.borrow_mut();
            if st.ticking {
                return;
            }
            st.ticking = true;
            st.last_tick = None;
        }
        let me = self.clone();
        self.canvas.add_tick_callback(move |_, clock| {
            let now = clock.frame_time();
            let reduce = prefs::get().reduce_motion;
            {
                let mut st = me.state.borrow_mut();
                let dt = st.last_tick.map_or(0.0, |t| (now - t) as f64 / 1e6).clamp(0.0, 0.25);
                st.last_tick = Some(now);
                let wind = st.layer == RadarLayerKind::WindStreamlines && st.field.is_some() && !reduce;
                let playing = st.playing && is_tile_layer(st.layer) && st.frames.len() > 1;
                if !wind && !playing {
                    st.ticking = false;
                    return glib::ControlFlow::Break;
                }
                if wind {
                    st.anim_phase = (st.anim_phase + dt * 0.39).fract();
                }
                if playing {
                    st.advance(dt);
                }
            }
            me.update_timeline();
            me.canvas.queue_draw();
            glib::ControlFlow::Continue
        });
    }
}

pub fn build(
    page: &Page,
    snap: Option<&ForecastSnapshot>,
    _stale_fallback: bool,
    _last_error: Option<&str>,
) {
    let p = prefs::get();
    let utc_offset = snap.map(|s| s.utc_offset_seconds as i64).unwrap_or(0);
    let units = p.units();
    let loc = snap
        .map(|s| s.location.clone())
        .or_else(|| p.active_location());

    let Some(loc) = loc else {
        let g = page.group("Radar & overlays");
        g.add(&widgets::empty_state(
            "mark-location-symbolic",
            "No active location",
            "Select or search for a city to view radar and weather overlays.",
            None,
        ));
        return;
    };

    let state = Rc::new(RefCell::new(RadarViewState {
        layer: RadarLayerKind::from_id(&p.default_radar_layer),
        zoom: ZOOM.with(Cell::get),
        opacity: (p.radar_opacity_pct.clamp(35, 100) as f64) / 100.0,
        pinch_scale: 1.0,
        ..Default::default()
    }));

    // The page doesn't scroll: the map takes the height the window has left under
    // the page bar, with the zoom and the timeline floating over it.
    page.root.set_vscrollbar_policy(gtk::PolicyType::Never);
    page.body.set_vexpand(true);

    // 1. The page bar: the page, the place and what the layer shows; the layers.
    let bar = widgets::hbox(10);
    bar.add_css_class("radar-bar");
    bar.append(&widgets::label("Radar", "page-title"));
    let sub_lbl = widgets::label("", "page-sub");
    sub_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
    sub_lbl.set_hexpand(true);
    sub_lbl.set_xalign(0.0);
    bar.append(&sub_lbl);
    let layer_seg = widgets::segmented(
        &widgets::opts(&[
            ("radar", "Radar"),
            ("satellite", "Satellite"),
            ("wind", "Wind"),
            ("temp", "Temperature"),
        ]),
        state.borrow().layer.id(),
        {
            let state = state.clone();
            move |id| {
                let kind = RadarLayerKind::from_id(&id);
                prefs::update(|pr| pr.default_radar_layer = id);
                state.borrow_mut().layer = kind;
            }
        },
    );
    bar.append(&layer_seg);

    // The place's coordinates, wind and rain, behind one button so the map keeps the room.
    let details = widgets::vbox(4);
    details.add_css_class("radar-details");
    let details_btn = gtk::MenuButton::new();
    details_btn.set_icon_name("view-more-symbolic");
    details_btn.set_tooltip_text(Some("Map details"));
    details_btn.add_css_class("flat");
    details_btn.set_valign(gtk::Align::Center);
    let details_pop = gtk::Popover::new();
    details_pop.set_child(Some(&details));
    details_btn.set_popover(Some(&details_pop));
    bar.append(&details_btn);
    page.body.append(&bar);

    // 2. The map, filling the rest.
    let canvas = gtk::DrawingArea::new();
    canvas.set_content_height(MAP_MIN_HEIGHT);
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    canvas.add_css_class("radar-canvas");
    let map = gtk::Overlay::new();
    map.set_vexpand(true);
    map.set_child(Some(&canvas));
    page.body.append(&map);

    // Zoom in, out, and back to the place, on the map's top-right corner.
    let zoom_in = widgets::icon_button("list-add-symbolic", "Zoom in (Ctrl+scroll, double-click, or pinch)");
    let zoom_out = widgets::icon_button("list-remove-symbolic", "Zoom out (Ctrl+scroll, or pinch)");
    let zoom_home = widgets::icon_button("find-location-symbolic", &format!("Back to {}", loc.name));
    let map_zoom = widgets::vbox(2);
    map_zoom.add_css_class("map-zoom");
    map_zoom.set_halign(gtk::Align::End);
    map_zoom.set_valign(gtk::Align::Start);
    map_zoom.append(&zoom_in);
    map_zoom.append(&zoom_out);
    map_zoom.append(&zoom_home);
    map.add_overlay(&map_zoom);

    // 3. Play, the timeline and the overlay's opacity, floating at the map's foot.
    let play_bar = widgets::hbox(14);
    play_bar.add_css_class("radar-play");
    play_bar.set_valign(gtk::Align::End);
    let play_btn = widgets::icon_button("media-playback-start-symbolic", "Play the radar loop");
    play_btn.add_css_class("radar-play-btn");
    play_btn.set_valign(gtk::Align::Center);
    play_bar.append(&play_btn);

    let frame_slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 12.0, 1.0);
    frame_slider.add_css_class("radar-timeline");
    frame_slider.set_draw_value(false);
    frame_slider.set_hexpand(true);
    frame_slider.set_valign(gtk::Align::Center);
    frame_slider.set_margin_end(24);
    play_bar.append(&frame_slider);

    let frame_time_lbl = widgets::label("Live", "radar-frame-time");
    frame_time_lbl.set_valign(gtk::Align::Center);
    play_bar.append(&frame_time_lbl);

    let opacity = widgets::hbox(8);
    opacity.add_css_class("radar-opacity");
    opacity.set_valign(gtk::Align::Center);
    opacity.append(&widgets::label("Opacity", ""));
    let initial = (state.borrow().opacity * 100.0).round();
    let opacity_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 35.0, 100.0, 5.0);
    opacity_scale.set_draw_value(false);
    opacity_scale.set_width_request(90);
    opacity_scale.set_value(initial);
    opacity_scale.set_tooltip_text(Some("How strongly the layer shows over the map"));
    opacity.append(&opacity_scale);
    let pct = widgets::label(&format!("{initial:.0}%"), "tnum");
    pct.set_width_chars(4);
    pct.set_xalign(1.0);
    opacity.append(&pct);
    play_bar.append(&opacity);
    map.add_overlay(&play_bar);

    let ctx = Rc::new(Ctx {
        state: state.clone(),
        canvas: canvas.clone(),
        loc: loc.clone(),
        cfg: p.open_meteo_config(),
        utc_offset,
        play_bar: play_bar.clone(),
        play_btn: play_btn.clone(),
        frame_slider: frame_slider.clone(),
        frame_time_lbl: frame_time_lbl.clone(),
        opacity: opacity.clone(),
        sub_lbl: sub_lbl.clone(),
        marked: Cell::new((usize::MAX, 0)),
        syncing: Cell::new(false),
        resize_timer: RefCell::new(None),
    });

    {
        let state = state.clone();
        let canvas = canvas.clone();
        opacity_scale.connect_value_changed(move |s| {
            let v = s.value().round().clamp(35.0, 100.0);
            pct.set_text(&format!("{v:.0}%"));
            prefs::update(|pr| pr.radar_opacity_pct = v as u8);
            state.borrow_mut().opacity = v / 100.0;
            canvas.queue_draw();
        });
    }

    // Drawing: the composed pictures, the frame fade and what goes over them.
    {
        let state = state.clone();
        let snap_owned = snap.cloned();
        let c = ctx.clone();
        let play_bar = play_bar.clone();
        canvas.set_draw_func(move |_, cr, w, h| {
            let st = state.borrow();
            let reduce = prefs::get().reduce_motion;
            // A field is placed by latitude and longitude, so the last one fetched is right
            // until the next arrives.
            let field = st.field.as_ref().map(|(_, f)| f);
            let scene = MapScene {
                layer: st.layer,
                geom: Some(MapGeom::new(loc.latitude, loc.longitude, st.zoom, w as f64, h as f64, st.pinch_scale)),
                basemap: st.basemap.as_ref(),
                frames: if is_tile_layer(st.layer) { st.frames_at(reduce) } else { None },
                field,
                snap: snap_owned.as_ref(),
                units,
                opacity: st.opacity,
                anim_phase: st.anim_phase,
                notice: st.notice.as_deref(),
                credit: c.credit(st.layer),
                inset_bottom: play_bar
                    .compute_bounds(&c.canvas)
                    .map_or((play_bar.height() + PLAY_BAR_MARGIN) as f64, |b| h as f64 - b.y() as f64 + CREDIT_GAP),
            };
            charts::draw_radar_map(cr, w as f64, h as f64, &scene);
        });
    }

    // Theme changes recolour the basemap and the satellite clouds.
    {
        let c = ctx.clone();
        theme::subscribe(&canvas, move || {
            {
                let mut st = c.state.borrow_mut();
                st.scene = None;
                st.basemap = None;
            }
            c.marked.set((usize::MAX, 0));
            c.load_scene();
        });
    }

    // The map is composed for the canvas: the first size it gets, then (a moment after
    // a resize settles) any size it outgrows.
    {
        let c = ctx.clone();
        canvas.connect_resize(move |_, _, _| {
            if c.state.borrow().scene.is_none() {
                c.load_scene();
                return;
            }
            if let Some(old) = c.resize_timer.borrow_mut().take() {
                old.remove();
            }
            let c2 = c.clone();
            let id = glib::timeout_add_local_once(Duration::from_millis(200), move || {
                c2.resize_timer.borrow_mut().take();
                c2.load_scene();
            });
            *c.resize_timer.borrow_mut() = Some(id);
        });
    }

    // Wire layer switcher to load the layer
    {
        let mut child = layer_seg.first_child();
        while let Some(w) = child {
            if let Some(btn) = w.downcast_ref::<gtk::Button>() {
                let c = ctx.clone();
                btn.connect_clicked(move |_| {
                    let limit = c.max_zoom();
                    let z = c.state.borrow().zoom;
                    if z > limit {
                        c.set_zoom(limit);
                    } else {
                        c.load_scene();
                    }
                    c.ensure_ticking();
                });
            }
            child = w.next_sibling();
        }
    }

    // Wire zoom buttons
    {
        let c = ctx.clone();
        zoom_out.connect_clicked(move |_| c.zoom_by(-1));
        let c = ctx.clone();
        zoom_in.connect_clicked(move |_| c.zoom_by(1));
        let c = ctx.clone();
        zoom_home.connect_clicked(move |_| c.set_zoom(6));
    }

    // Wire touchpad / touchscreen pinch-to-zoom (gtk::GestureZoom)
    {
        let zoom_gesture = gtk::GestureZoom::new();
        zoom_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let pinch_base = Rc::new(Cell::new(1.0_f64));
        {
            let state = state.clone();
            let pinch_base = pinch_base.clone();
            zoom_gesture.connect_begin(move |_, _| {
                pinch_base.set(1.0);
                state.borrow_mut().pinch_scale = 1.0;
            });
        }
        {
            let c = ctx.clone();
            let pinch_base = pinch_base.clone();
            zoom_gesture.connect_scale_changed(move |_, scale| {
                let rel = scale / pinch_base.get().max(0.05);
                let (zoom, limit) = (c.state.borrow().zoom, c.max_zoom());
                if rel >= 1.38 && zoom < limit {
                    pinch_base.set(scale);
                    c.set_zoom(zoom + 1);
                } else if rel <= 0.72 && zoom > 4 {
                    pinch_base.set(scale);
                    c.set_zoom(zoom - 1);
                } else {
                    c.state.borrow_mut().pinch_scale = rel.clamp(0.85, 1.85);
                    c.canvas.queue_draw();
                }
            });
        }
        {
            let c = ctx.clone();
            zoom_gesture.connect_end(move |_, _| {
                let (pinch, zoom) = {
                    let st = c.state.borrow();
                    (st.pinch_scale, st.zoom)
                };
                if pinch >= 1.18 {
                    c.set_zoom(zoom + 1);
                } else if pinch <= 0.88 {
                    c.set_zoom(zoom.saturating_sub(1));
                }
                c.state.borrow_mut().pinch_scale = 1.0;
                c.canvas.queue_draw();
            });
        }
        canvas.add_controller(zoom_gesture);
    }

    // Wire Ctrl + scroll wheel / touchpad scroll zoom & double-click zoom on canvas
    {
        let scroll_ctrl = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let c = ctx.clone();
        let scroll_accum = Cell::new(0.0_f64);
        scroll_ctrl.connect_scroll(move |ctrl, _dx, dy| {
            let mods = ctrl.current_event_state();
            if !mods.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            let acc = scroll_accum.get() + dy;
            if acc <= -0.75 {
                scroll_accum.set(0.0);
                c.zoom_by(1);
            } else if acc >= 0.75 {
                scroll_accum.set(0.0);
                c.zoom_by(-1);
            } else {
                scroll_accum.set(acc);
            }
            glib::Propagation::Stop
        });
        canvas.add_controller(scroll_ctrl);
    }
    {
        let dbl_click = gtk::GestureClick::new();
        let c = ctx.clone();
        dbl_click.connect_pressed(move |_, n_press, _, _| {
            if n_press == 2 {
                c.zoom_by(1);
            }
        });
        canvas.add_controller(dbl_click);
    }

    // The slider picks a frame; playback moves it smoothly.
    {
        let c = ctx.clone();
        frame_slider.connect_value_changed(move |s| {
            if c.syncing.get() {
                return;
            }
            {
                let mut st = c.state.borrow_mut();
                st.pos = s.value().round().max(0.0);
                st.hold = 0.0;
            }
            c.update_timeline();
            c.canvas.queue_draw();
        });
    }

    // Wire Play/Pause button
    {
        let c = ctx.clone();
        let play_btn2 = play_btn.clone();
        play_btn.connect_clicked(move |_| {
            let now_playing = {
                let mut st = c.state.borrow_mut();
                st.playing = !st.playing;
                // From the newest frame, start the loop over.
                if st.playing && st.pos >= st.frames.len().saturating_sub(1) as f64 {
                    st.pos = st.ready_start() as f64;
                    st.hold = 0.0;
                }
                st.playing
            };
            set_play_icon(&play_btn2, now_playing);
            if now_playing {
                c.ensure_ticking();
            }
        });
    }

    // RainViewer lists its frames; once that's in, a place outside the US loads them.
    {
        let c = ctx.clone();
        cmd::background(backend::fetch_radar_manifest, move |res| {
            if let Ok(m) = res {
                c.state.borrow_mut().manifest = Some(m);
                if !c.loc.is_us() && c.state.borrow().layer == RadarLayerKind::PrecipitationRadar {
                    c.state.borrow_mut().scene = None;
                    c.load_scene();
                }
            }
        });
    }

    // 4. Regional telemetry, in the details popover
    if let Some(s) = snap {
        details.append(&widgets::label(&s.location.name, "reading-details-title"));
        let coords = format!("{:.4}°, {:.4}°", s.location.latitude, s.location.longitude);
        details.append(&widgets::row(
            "Target coordinates",
            "The centre of the map.",
            Some(widgets::label(&coords, "tnum").upcast_ref()),
        ));
        let wind_vec = format!(
            "{} {} ({}°){}",
            units.wind_speed.format(s.current.wind_speed_kmh),
            cardinal_direction(s.current.wind_direction_deg),
            s.current.wind_direction_deg,
            s.current
                .wind_gusts_kmh
                .map(|g| format!(" · gusts {}", units.wind_speed.format(g)))
                .unwrap_or_default()
        );
        details.append(&widgets::row(
            "Surface wind vector",
            "At the location itself; the Wind layer shows it across the map.",
            Some(widgets::label(&wind_vec, "tnum").upcast_ref()),
        ));
        let precip_now = format!(
            "{} · {}% cloud cover",
            units.precipitation.format(s.current.precipitation_mm),
            s.current.cloud_cover_pct
        );
        details.append(&widgets::row(
            "Surface precipitation & cloud deck",
            "Current liquid-equivalent precipitation rate and total cloud fraction.",
            Some(widgets::label(&precip_now, "tnum").upcast_ref()),
        ));
    }
}

/// The map's smallest height; past this the window, not the map, stops shrinking.
const MAP_MIN_HEIGHT: i32 = 200;

/// The space between the floating play bar and the map's edges (style.css's `.radar-play`).
const PLAY_BAR_MARGIN: i32 = 14;

/// The space between the play bar's top and the credits drawn over the map above it.
const CREDIT_GAP: f64 = 6.0;

fn set_play_icon(b: &gtk::Button, playing: bool) {
    b.set_icon_name(if playing { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" });
    b.set_tooltip_text(Some(if playing { "Pause the radar loop" } else { "Play the radar loop" }));
}
