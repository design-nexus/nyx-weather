//! Map pictures, built off the GTK thread: the tiles of a [`TileView`] decoded,
//! recoloured (the basemap in the theme's colours, radar on the classic NWS scale,
//! satellite in the theme's cloud colours) and laid out at their native size on the
//! canvas. A worker makes a [`Raster`] of plain pixels; the GTK thread turns it into a
//! surface to paint.

use crate::theme::{self, Rgb};
use gtk::prelude::*;
use gtk::{cairo, gdk_pixbuf};
use nyx_weather_core::{FieldGrid, RadarPalette, TileView, reflectivity, tile_xy};

/// The map on the canvas: the location at the centre of a `w × h` px canvas at zoom `z`.
#[derive(Debug, Clone, Copy)]
pub struct MapGeom {
    pub z: u8,
    pub w: f64,
    pub h: f64,
    /// The location, in tiles at `z`.
    center: (f64, f64),
    /// A pinch in progress scales the map around its centre.
    pub pinch: f64,
}

impl MapGeom {
    pub fn new(lat: f64, lon: f64, z: u8, w: f64, h: f64, pinch: f64) -> Self {
        Self { z, w, h, center: tile_xy(lat, lon, z), pinch }
    }

    /// Canvas position of (`lat`, `lon`).
    pub fn project(&self, lat: f64, lon: f64) -> (f64, f64) {
        let (x, y) = tile_xy(lat, lon, self.z);
        let k = 256.0 * self.pinch;
        (self.w / 2.0 + (x - self.center.0) * k, self.h / 2.0 + (y - self.center.1) * k)
    }

    /// Canvas position of grid position (`gx`, `gy`) of `f`.
    pub fn project_grid(&self, f: &FieldGrid, gx: f64, gy: f64) -> (f64, f64) {
        let max = (f.size - 1).max(1) as f64;
        let b = f.bounds;
        self.project(b.north + (b.south - b.north) * gy / max, b.west + (b.east - b.west) * gx / max)
    }

    /// Where tile (`col`, `row`) of `view` sits on the canvas: its top-left corner and size.
    fn tile_rect(&self, view: &TileView, col: i32, row: i32) -> (f64, f64, f64) {
        let k = 2f64.powi(self.z as i32 - view.z as i32);
        let x = self.w / 2.0 + ((view.x0 + col) as f64 * k - self.center.0) * 256.0;
        let y = self.h / 2.0 + ((view.y0 + row) as f64 * k - self.center.1) * 256.0;
        (x, y, view.tile_px)
    }
}

/// The theme's map colours (0–255 RGB), read on the GTK thread for a worker.
#[derive(Debug, Clone, Copy)]
pub struct BasemapColors {
    land: [f64; 3],
    minor_road: [f64; 3],
    road: [f64; 3],
    line: [f64; 3],
    label: [f64; 3],
    water: [f64; 3],
    park: [f64; 3],
}

fn rgb255(c: Rgb) -> [f64; 3] {
    [c.0 * 255.0, c.1 * 255.0, c.2 * 255.0]
}

impl BasemapColors {
    /// Calm colours from the theme: neutral land and greys for roads and borders, a
    /// slightly cool sea, barely tinted parks, labels in the dim text colour. Nothing
    /// in the accent, so precipitation carries the map.
    pub fn current() -> Self {
        let p = theme::palette();
        let wc = theme::weather_colors_for(&p);
        let (bg, surface, text, dim) = (Rgb::hex(&p.bg), Rgb::hex(&p.surface), Rgb::hex(&p.text), Rgb::hex(&p.dim_text));
        if p.light {
            let land = bg.mix(dim, 0.07);
            Self {
                land: rgb255(land),
                minor_road: rgb255(bg),
                road: rgb255(bg.mix(dim, 0.22)),
                line: rgb255(bg.mix(dim, 0.40)),
                label: rgb255(dim.mix(text, 0.35)),
                water: rgb255(bg.mix(wc.cold, 0.16)),
                park: rgb255(land.mix(wc.wind, 0.08)),
            }
        } else {
            let land = surface.mix(bg, 0.35);
            Self {
                land: rgb255(land),
                minor_road: rgb255(land.mix(dim, 0.16)),
                road: rgb255(land.mix(dim, 0.30)),
                line: rgb255(land.mix(dim, 0.45)),
                label: rgb255(dim.mix(text, 0.25)),
                water: rgb255(bg.mix(wc.cold, 0.16)),
                park: rgb255(land.mix(wc.wind, 0.06)),
            }
        }
    }

    /// An OpenStreetMap pixel in these colours: coloured features by kind, the rest by
    /// lightness (dark labels and borders, mid lines, light land, white streets).
    fn tint(&self, [r, g, b]: [f64; 3]) -> [f64; 3] {
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        if max - min > 28.0 {
            return if b > r + 14.0 && b + 6.0 >= g {
                self.water
            } else if g > r + 8.0 && g >= b {
                self.park
            } else {
                self.road
            };
        }
        let luma = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
        let lerp = |a: [f64; 3], b: [f64; 3], t: f64| {
            let t = t.clamp(0.0, 1.0);
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
        };
        if luma < 0.55 {
            lerp(self.label, self.line, luma / 0.55)
        } else if luma < 0.93 {
            lerp(self.line, self.land, (luma - 0.55) / 0.38)
        } else if luma < 0.97 {
            self.land
        } else {
            lerp(self.land, self.minor_road, (luma - 0.97) / 0.03)
        }
    }
}

/// How a layer's tiles are recoloured.
#[derive(Debug, Clone, Copy)]
pub enum Recolor {
    Basemap(BasemapColors),
    Radar(RadarPalette),
    /// Infrared satellite: clear ground transparent, cloud tops from `low` to `high`.
    Infrared {
        low: Rgb,
        high: Rgb,
    },
}

impl Recolor {
    /// The satellite layer's colours, from the theme (GTK thread).
    pub fn infrared() -> Self {
        let wc = theme::weather_colors_for(&theme::palette());
        Self::Infrared { low: wc.rain, high: wc.uv.mix(wc.sun, 0.4) }
    }

    /// The straight RGBA to draw a straight-RGBA tile pixel in.
    fn apply(&self, px: [u8; 4]) -> [u8; 4] {
        match self {
            Self::Radar(palette) => reflectivity::decode(*palette, px).map(reflectivity::nws_color).unwrap_or([0; 4]),
            Self::Basemap(colors) => {
                let [r, g, b] = colors.tint([px[0] as f64, px[1] as f64, px[2] as f64]);
                [r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8, px[3]]
            }
            Self::Infrared { low, high } => {
                let luma = (0.299 * px[0] as f64 + 0.587 * px[1] as f64 + 0.114 * px[2] as f64) / 255.0;
                if luma < 0.36 || px[3] == 0 {
                    return [0; 4];
                }
                let norm = ((luma - 0.36) / 0.64).clamp(0.0, 1.0);
                let c = low.mix(*high, norm);
                [(c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8, (norm * 225.0).clamp(30.0, 230.0) as u8]
            }
        }
    }
}

/// Plain pixels (cairo ARGB32, premultiplied), made on a worker.
pub struct Raster {
    data: Vec<u8>,
    width: i32,
    height: i32,
    stride: i32,
    /// Device pixels per canvas pixel.
    scale: f64,
}

impl Raster {
    /// The picture as a surface that paints at canvas size (GTK thread).
    pub fn into_surface(self) -> Option<cairo::ImageSurface> {
        let s =
            cairo::ImageSurface::create_for_data(self.data, cairo::Format::ARgb32, self.width, self.height, self.stride).ok()?;
        s.set_device_scale(self.scale, self.scale);
        Some(s)
    }
}

/// Decode a PNG tile and recolour it into a premultiplied ARGB32 surface.
fn tile_surface(bytes: &[u8], recolor: &Recolor) -> Option<cairo::ImageSurface> {
    let loader = gdk_pixbuf::PixbufLoader::new();
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    let pb = loader.pixbuf()?;
    let (w, h) = (pb.width() as usize, pb.height() as usize);
    let (n, rowstride) = (pb.n_channels() as usize, pb.rowstride() as usize);
    if n < 3 {
        return None;
    }
    let src = pb.read_pixel_bytes();
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = y * rowstride + x * n;
            if i + n > src.len() {
                continue;
            }
            let px = [src[i], src[i + 1], src[i + 2], if n == 4 { src[i + 3] } else { 255 }];
            let [r, g, b, a] = recolor.apply(px);
            if a == 0 {
                continue;
            }
            let pre = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
            let o = (y * w + x) * 4;
            // ARGB32 is a native-endian u32: blue, green, red, alpha in memory here.
            out[o..o + 4].copy_from_slice(&u32::from_be_bytes([a, pre(r), pre(g), pre(b)]).to_ne_bytes());
        }
    }
    cairo::ImageSurface::create_for_data(out, cairo::Format::ARgb32, w as i32, h as i32, (w * 4) as i32).ok()
}

/// Lay `tiles` (PNG bytes, row-major over `view`) out on `geom`'s canvas at `scale`
/// device pixels per canvas pixel, recoloured by `recolor`. `None` when no tile could
/// be read. Runs on a worker: everything it touches is its own.
pub fn compose(view: &TileView, tiles: &[Option<Vec<u8>>], geom: &MapGeom, scale: f64, recolor: &Recolor) -> Option<Raster> {
    let (width, height) = ((geom.w * scale).ceil().max(1.0) as i32, (geom.h * scale).ceil().max(1.0) as i32);
    let target = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).ok()?;
    let mut any = false;
    {
        let cr = cairo::Context::new(&target).ok()?;
        cr.scale(scale, scale);
        for ((col, row), bytes) in view.cells().zip(tiles) {
            let Some(bytes) = bytes else { continue };
            let (x, y, size) = geom.tile_rect(view, col, row);
            if x > geom.w || y > geom.h || x + size < 0.0 || y + size < 0.0 {
                continue;
            }
            let Some(tile) = tile_surface(bytes, recolor) else { continue };
            let k = size / tile.width().max(1) as f64;
            cr.save().ok();
            cr.translate(x, y);
            cr.scale(k, k);
            if cr.set_source_surface(&tile, 0.0, 0.0).is_ok() {
                let src = cr.source();
                src.set_filter(if k * scale < 1.0 { cairo::Filter::Good } else { cairo::Filter::Bilinear });
                // Pad, so scaled tiles meet without a seam.
                src.set_extend(cairo::Extend::Pad);
                cr.rectangle(0.0, 0.0, tile.width() as f64, tile.height() as f64);
                let _ = cr.fill();
                any = true;
            }
            cr.restore().ok();
        }
    }
    target.flush();
    let stride = target.stride();
    let data = target.take_data().ok()?.to_vec();
    any.then_some(Raster { data, width, height, stride, scale })
}
