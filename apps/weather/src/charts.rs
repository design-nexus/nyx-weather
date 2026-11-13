//! Cairo charts, astronomy diagrams, and interactive radar/overlay map rendering.
//! Every color is derived from `theme::palette()` and `theme::weather_colors()`
//! so live Omarchy theme changes apply immediately to all graphs and maps.

use crate::maprender::MapGeom;
use crate::theme::{self, Rgb};
use gtk::prelude::*;
use gtk::cairo;
use nyx_weather_core::{
    Echo, FieldGrid, ForecastSnapshot, MoonPhaseInfo, RadarLayerKind, UnitPrefs,
};
use nyx_weather_core::reflectivity::nws_color;
use std::f64::consts::PI;

/// Chart and map labels are in the theme's interface font: the first family of
/// its `[font] ui` list (Cairo takes one name), else the system sans.
fn ui_font() -> String {
    nyx_theme::look()
        .font
        .ui
        .as_deref()
        .and_then(|list| list.split(',').map(str::trim).find(|f| !f.is_empty()))
        .unwrap_or("sans-serif")
        .to_string()
}

fn set_rgb(cr: &cairo::Context, c: Rgb, a: f64) {
    cr.set_source_rgba(c.0, c.1, c.2, a);
}

#[derive(Clone)]
pub struct ChartSeries {
    pub label: String,
    pub values: Vec<f64>,
    pub dashed: bool,
    /// Color index:
    /// 0 = warm/accent, 1 = rain/cyan, 2 = cold/blue, 3 = wind/green, 4 = uv/magenta, 5 = sun/yellow
    pub tone: usize,
}

fn series_color(tone: usize) -> Rgb {
    let p = theme::palette();
    let wc = theme::weather_colors_for(&p);
    let accent = Rgb::hex(&p.accent);
    match tone {
        0 => wc.warm.mix(accent, 0.35),
        1 => wc.rain,
        2 => wc.cold,
        3 => wc.wind,
        4 => wc.uv,
        _ => wc.sun,
    }
}

/// Build a responsive, theme-reactive Cairo multi-series line & area chart with optional background bars.
pub fn line_chart(
    series: Vec<ChartSeries>,
    x_labels: Vec<String>,
    unit_suffix: String,
    fixed_range: Option<(f64, f64)>,
    bars_0_to_100: Option<Vec<f64>>,
    height: i32,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_height(height);
    area.set_hexpand(true);

    let a_weak = area.downgrade();
    theme::subscribe(&area, move || {
        if let Some(a) = a_weak.upgrade() {
            a.queue_draw();
        }
    });

    // The pointer's x while it is over the chart: the nearest point is read out.
    let hover = std::rc::Rc::new(std::cell::Cell::new(None::<f64>));
    let motion = gtk::EventControllerMotion::new();
    {
        let (h, a) = (hover.clone(), area.downgrade());
        motion.connect_motion(move |_, x, _| {
            h.set(Some(x));
            if let Some(a) = a.upgrade() {
                a.queue_draw();
            }
        });
        let (h, a) = (hover.clone(), area.downgrade());
        motion.connect_leave(move |_| {
            h.set(None);
            if let Some(a) = a.upgrade() {
                a.queue_draw();
            }
        });
    }
    area.add_controller(motion);

    area.set_draw_func(move |_, cr, w, h| {
        let w = w as f64;
        let h = h as f64;
        let p = theme::palette();
        let wc = theme::weather_colors_for(&p);
        let text = Rgb::hex(&p.text);
        let dim = Rgb::hex(&p.dim_text);
        let border = Rgb::hex(&p.border);
        let surface = Rgb::hex(&p.surface);

        let pad_l = 48.0;
        let pad_r = 18.0;
        let pad_t = 26.0;
        let pad_b = 26.0;
        let cw = (w - pad_l - pad_r).max(40.0);
        let ch = (h - pad_t - pad_b).max(40.0);

        let (mut min_v, mut max_v) = if let Some((lo, hi)) = fixed_range {
            (lo, hi)
        } else {
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for s in &series {
                for &v in &s.values {
                    if v.is_finite() {
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                }
            }
            if !lo.is_finite() || !hi.is_finite() {
                (0.0, 10.0)
            } else {
                let span = (hi - lo).max(2.0);
                (lo - span * 0.12, hi + span * 0.15)
            }
        };
        if (max_v - min_v).abs() < 1e-6 {
            max_v += 1.0;
            min_v -= 1.0;
        }

        // Decimals enough to tell the labels apart: a pressure in inHg moves by tenths.
        let span = max_v - min_v;
        let digits = if span < 2.0 { 2 } else if span < 20.0 { 1 } else { 0 };

        // Horizontal grid lines + Y-axis labels
        cr.select_font_face(&ui_font(), cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(10.5);
        for i in 0..=4 {
            let frac = i as f64 / 4.0;
            let y = pad_t + ch * (1.0 - frac);
            set_rgb(cr, border, 0.35);
            cr.set_line_width(1.0);
            cr.move_to(pad_l, y);
            cr.line_to(pad_l + cw, y);
            let _ = cr.stroke();

            let val = min_v + (max_v - min_v) * frac;
            let lbl = format!("{val:.digits$}{unit_suffix}");
            set_rgb(cr, dim, 0.88);
            cr.move_to(6.0, y + 3.5);
            let _ = cr.show_text(&lbl);
        }

        // Optional background bars (e.g. precipitation probability 0..100%)
        if let Some(bars) = &bars_0_to_100
            && !bars.is_empty()
        {
            let n = bars.len().max(1);
            let step = cw / n as f64;
            let bar_w = (step * 0.58).clamp(2.5, 16.0);
            for (i, &pct) in bars.iter().enumerate() {
                if pct <= 1.0 {
                    continue;
                }
                let bh = (pct.clamp(0.0, 100.0) / 100.0) * (ch * 0.68);
                let x = pad_l + (i as f64 + 0.5) * step - bar_w * 0.5;
                let y = pad_t + ch - bh;
                let grad = cairo::LinearGradient::new(0.0, y, 0.0, pad_t + ch);
                grad.add_color_stop_rgba(0.0, wc.rain.0, wc.rain.1, wc.rain.2, 0.46);
                grad.add_color_stop_rgba(1.0, wc.rain.0, wc.rain.1, wc.rain.2, 0.10);
                let _ = cr.set_source(&grad);
                cr.rectangle(x, y, bar_w, bh);
                let _ = cr.fill();
            }
        }

        // X-axis labels
        if !x_labels.is_empty() {
            let n = x_labels.len();
            let stride = (n / 7).max(1);
            set_rgb(cr, dim, 0.88);
            for (i, lbl) in x_labels.iter().enumerate() {
                if i % stride != 0 && i + 1 != n {
                    continue;
                }
                let x = if n > 1 {
                    pad_l + (i as f64 / (n - 1) as f64) * cw
                } else {
                    pad_l + cw * 0.5
                };
                cr.move_to((x - 14.0).clamp(pad_l, pad_l + cw - 30.0), h - 7.0);
                let _ = cr.show_text(lbl);
            }
        }

        // Draw series lines + gradient fill under primary series
        for (s_idx, s) in series.iter().enumerate() {
            let n = s.values.len();
            if n < 2 {
                continue;
            }
            let col = series_color(s.tone);
            let pt = |i: usize| -> (f64, f64) {
                let x = pad_l + (i as f64 / (n - 1) as f64) * cw;
                let v = s.values[i].clamp(min_v, max_v);
                let y = pad_t + ch * (1.0 - (v - min_v) / (max_v - min_v));
                (x, y)
            };

            // Runs of finite values; a missing value (NaN) leaves a gap.
            let mut runs: Vec<(usize, usize)> = Vec::new();
            let mut i = 0;
            while i < n {
                if !s.values[i].is_finite() {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < n && s.values[i].is_finite() {
                    i += 1;
                }
                runs.push((start, i));
            }

            if s_idx == 0 && !s.dashed {
                for &(a, b) in runs.iter().filter(|(a, b)| b - a > 1) {
                    cr.new_path();
                    let (x0, y0) = pt(a);
                    cr.move_to(x0, pad_t + ch);
                    cr.line_to(x0, y0);
                    for i in a + 1..b {
                        let (x, y) = pt(i);
                        cr.line_to(x, y);
                    }
                    let (xn, _) = pt(b - 1);
                    cr.line_to(xn, pad_t + ch);
                    cr.close_path();

                    let grad = cairo::LinearGradient::new(0.0, pad_t, 0.0, pad_t + ch);
                    grad.add_color_stop_rgba(0.0, col.0, col.1, col.2, 0.34);
                    grad.add_color_stop_rgba(1.0, col.0, col.1, col.2, 0.02);
                    let _ = cr.set_source(&grad);
                    let _ = cr.fill();
                }
            }

            cr.new_path();
            for &(a, b) in &runs {
                let (x0, y0) = pt(a);
                cr.move_to(x0, y0);
                for i in a + 1..b {
                    let (x, y) = pt(i);
                    cr.line_to(x, y);
                }
            }
            set_rgb(cr, col, if s.dashed { 0.88 } else { 0.98 });
            cr.set_line_width(if s.dashed { 1.8 } else { 2.5 });
            if s.dashed {
                cr.set_dash(&[5.0, 4.0], 0.0);
            } else {
                cr.set_dash(&[], 0.0);
            }
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);

            // Data point dots & labels at sparse intervals
            let dot_stride = (n / 8).max(1);
            for i in (0..n).step_by(dot_stride).filter(|&i| s.values[i].is_finite()) {
                let (x, y) = pt(i);
                set_rgb(cr, col, 1.0);
                cr.arc(x, y, 3.2, 0.0, 2.0 * PI);
                let _ = cr.fill();
                if s_idx == 0 {
                    set_rgb(cr, text, 0.92);
                    cr.set_font_size(9.5);
                    cr.move_to(x - 8.0, (y - 7.0).max(12.0));
                    let _ = cr.show_text(&format!("{:.digits$}", s.values[i], digits = digits.saturating_sub(1)));
                }
            }
        }

        // Top-right legend
        let mut lx = pad_l + 6.0;
        cr.set_font_size(10.5);
        for s in &series {
            let col = series_color(s.tone);
            set_rgb(cr, col, 1.0);
            cr.arc(lx + 4.0, 11.0, 4.2, 0.0, 2.0 * PI);
            let _ = cr.fill();
            set_rgb(cr, text, 0.92);
            cr.move_to(lx + 12.0, 14.5);
            let _ = cr.show_text(&s.label);
            lx += (s.label.chars().count() as f64 * 6.5) + 28.0;
        }

        // Hover readout: a line at the nearest point and its values.
        let n = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
        if let Some(hx) = hover.get()
            && n > 1
        {
            let i = (((hx - pad_l) / cw) * (n - 1) as f64).round().clamp(0.0, (n - 1) as f64) as usize;
            let x = pad_l + (i as f64 / (n - 1) as f64) * cw;
            set_rgb(cr, dim, 0.7);
            cr.set_line_width(1.0);
            cr.move_to(x, pad_t);
            cr.line_to(x, pad_t + ch);
            let _ = cr.stroke();

            let mut lines: Vec<(Option<Rgb>, String)> = Vec::new();
            if let Some(l) = x_labels.get(i) {
                lines.push((None, l.clone()));
            }
            for s in &series {
                if let Some(v) = s.values.get(i).filter(|v| v.is_finite()) {
                    lines.push((Some(series_color(s.tone)), format!("{}: {v:.prec$}{unit_suffix}", s.label, prec = digits.max(1))));
                }
            }
            if let Some(pct) = bars_0_to_100.as_ref().and_then(|b| b.get(i)) {
                lines.push((Some(wc.rain), format!("Chance of rain: {pct:.0}%")));
            }
            cr.set_font_size(10.5);
            let widest = lines
                .iter()
                .filter_map(|(_, t)| cr.text_extents(t).ok().map(|e| e.x_advance()))
                .fold(0.0, f64::max);
            let (bw, bh) = (widest + 20.0, lines.len() as f64 * 15.0 + 8.0);
            let bx = if x + 10.0 + bw < w { x + 10.0 } else { (x - 10.0 - bw).max(0.0) };
            let by = pad_t;
            set_rgb(cr, surface, 0.96);
            cr.rectangle(bx, by, bw, bh);
            let _ = cr.fill();
            set_rgb(cr, border, 0.8);
            cr.rectangle(bx + 0.5, by + 0.5, bw - 1.0, bh - 1.0);
            let _ = cr.stroke();
            for (k, (c, t)) in lines.iter().enumerate() {
                let ty = by + 15.0 + k as f64 * 15.0;
                if let Some(c) = c {
                    set_rgb(cr, *c, 1.0);
                    cr.arc(bx + 8.0, ty - 3.5, 3.0, 0.0, 2.0 * PI);
                    let _ = cr.fill();
                }
                set_rgb(cr, text, 0.95);
                cr.move_to(bx + 14.0, ty);
                let _ = cr.show_text(t);
            }
        }
    });

    area
}

/// Solar trajectory & horizon arc diagram for the Astronomy section.
pub fn solar_arc_canvas(
    sunrise_iso: Option<String>,
    sunset_iso: Option<String>,
    current_iso: String,
    is_day: bool,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_height(155);
    area.set_hexpand(true);

    let a_weak = area.downgrade();
    theme::subscribe(&area, move || {
        if let Some(a) = a_weak.upgrade() {
            a.queue_draw();
        }
    });

    area.set_draw_func(move |_, cr, w, h| {
        let w = w as f64;
        let h = h as f64;
        let p = theme::palette();
        let wc = theme::weather_colors_for(&p);
        let text = Rgb::hex(&p.text);
        let dim = Rgb::hex(&p.dim_text);
        let border = Rgb::hex(&p.border);

        let parse_mins = |iso: &str| -> Option<f64> {
            let t = iso.split('T').nth(1)?;
            let mut parts = t.split(':');
            let hh: f64 = parts.next()?.parse().ok()?;
            let mm: f64 = parts.next()?.parse().ok()?;
            Some(hh * 60.0 + mm)
        };

        let sr_m = sunrise_iso.as_deref().and_then(parse_mins).unwrap_or(390.0);
        let ss_m = sunset_iso.as_deref().and_then(parse_mins).unwrap_or(1110.0);
        let cur_m = parse_mins(&current_iso).unwrap_or(750.0);

        let pad_x = 36.0;
        let horizon_y = h * 0.73;
        let span_w = (w - pad_x * 2.0).max(80.0);
        let amp = (h * 0.50).max(32.0);

        // Daylight warm gradient fill above horizon
        cr.new_path();
        let steps = 60;
        for i in 0..=steps {
            let frac = i as f64 / steps as f64;
            let x = pad_x + frac * span_w;
            let angle = (frac * 2.0 - 0.5) * PI;
            let y = horizon_y - angle.sin() * amp * 0.72;
            if i == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        cr.line_to(pad_x + span_w, horizon_y);
        cr.line_to(pad_x, horizon_y);
        cr.close_path();
        let sky_grad = cairo::LinearGradient::new(0.0, horizon_y - amp, 0.0, horizon_y);
        sky_grad.add_color_stop_rgba(0.0, wc.sun.0, wc.sun.1, wc.sun.2, 0.24);
        sky_grad.add_color_stop_rgba(0.6, wc.warm.0, wc.warm.1, wc.warm.2, 0.10);
        sky_grad.add_color_stop_rgba(1.0, wc.warm.0, wc.warm.1, wc.warm.2, 0.0);
        let _ = cr.set_source(&sky_grad);
        let _ = cr.fill();

        // Horizon line
        set_rgb(cr, border, 0.65);
        cr.set_line_width(1.2);
        cr.set_dash(&[4.0, 4.0], 0.0);
        cr.move_to(pad_x, horizon_y);
        cr.line_to(pad_x + span_w, horizon_y);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);

        // Sinusoidal solar curve across 24h
        cr.new_path();
        for i in 0..=steps {
            let frac = i as f64 / steps as f64;
            let x = pad_x + frac * span_w;
            let angle = (frac * 2.0 - 0.5) * PI;
            let y = horizon_y - angle.sin() * amp * 0.72;
            if i == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        set_rgb(cr, wc.sun, 0.92);
        cr.set_line_width(2.4);
        let _ = cr.stroke();

        // Current sun/moon marker on curve
        let day_span = (ss_m - sr_m).max(60.0);
        let frac = if cur_m >= sr_m && cur_m <= ss_m {
            0.25 + 0.5 * ((cur_m - sr_m) / day_span)
        } else if cur_m < sr_m {
            0.25 * (cur_m / sr_m.max(1.0))
        } else {
            0.75 + 0.25 * ((cur_m - ss_m) / (1440.0 - ss_m).max(1.0))
        }
        .clamp(0.02, 0.98);

        let sx = pad_x + frac * span_w;
        let sy = horizon_y - ((frac * 2.0 - 0.5) * PI).sin() * amp * 0.72;

        let orb_col = if is_day { wc.sun } else { wc.uv };
        set_rgb(cr, orb_col, 0.26);
        cr.arc(sx, sy, 14.0, 0.0, 2.0 * PI);
        let _ = cr.fill();
        set_rgb(cr, orb_col, 1.0);
        cr.arc(sx, sy, 6.2, 0.0, 2.0 * PI);
        let _ = cr.fill();

        // Labels for Sunrise, Solar Noon, Sunset
        cr.select_font_face(&ui_font(), cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(10.5);
        set_rgb(cr, dim, 0.92);
        cr.move_to(pad_x + span_w * 0.20, horizon_y + 18.0);
        let _ = cr.show_text("Sunrise");
        set_rgb(cr, wc.sun, 0.95);
        cr.move_to(pad_x + span_w * 0.45, horizon_y - amp * 0.76 - 8.0);
        let _ = cr.show_text("Solar Noon");
        set_rgb(cr, dim, 0.92);
        cr.move_to(pad_x + span_w * 0.70, horizon_y + 18.0);
        let _ = cr.show_text("Sunset");

        set_rgb(cr, text, 0.92);
        cr.move_to(12.0, 18.0);
        let _ = cr.show_text(if is_day {
            "☀ Sun above horizon"
        } else {
            "☾ Sun below horizon (Night)"
        });
    });

    area
}

/// Draw a realistic illuminated lunar phase disc using Cairo.
pub fn moon_phase_canvas(info: MoonPhaseInfo) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(110);
    area.set_content_height(110);

    let a_weak = area.downgrade();
    theme::subscribe(&area, move || {
        if let Some(a) = a_weak.upgrade() {
            a.queue_draw();
        }
    });

    area.set_draw_func(move |_, cr, w, h| {
        let cx = w as f64 * 0.5;
        let cy = h as f64 * 0.5;
        let r = (w.min(h) as f64 * 0.38).max(20.0);
        let p = theme::palette();
        let wc = theme::weather_colors_for(&p);
        let surface = Rgb::hex(&p.surface);
        let border = Rgb::hex(&p.border);
        let lit_col = wc.sun.mix(Rgb::hex(&p.text), 0.35);

        // Outer ambient lunar glow
        set_rgb(cr, wc.uv, 0.14);
        cr.arc(cx, cy, r + 8.0, 0.0, 2.0 * PI);
        let _ = cr.fill();

        // Dark lunar shadow disc
        set_rgb(cr, surface, 0.95);
        cr.arc(cx, cy, r, 0.0, 2.0 * PI);
        let _ = cr.fill();

        // Lit portion of the moon
        let phase = info.phase_fraction.clamp(0.0, 1.0);
        let waxing = phase <= 0.5;
        let k = (phase * 2.0 * PI).cos(); // +1 at new, -1 at full

        cr.save().ok();
        cr.arc(cx, cy, r, 0.0, 2.0 * PI);
        cr.clip();

        set_rgb(cr, lit_col, 0.92);
        let steps = 48;
        cr.new_path();
        for i in 0..=steps {
            let theta = -PI * 0.5 + (i as f64 / steps as f64) * PI;
            let y = cy + r * theta.sin();
            let x_limb = r * theta.cos();
            let x = if waxing { cx + x_limb } else { cx - x_limb };
            if i == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        for i in (0..=steps).rev() {
            let theta = -PI * 0.5 + (i as f64 / steps as f64) * PI;
            let y = cy + r * theta.sin();
            let x_term = r * theta.cos() * k;
            let x = if waxing { cx + x_term } else { cx - x_term };
            cr.line_to(x, y);
        }
        cr.close_path();
        let _ = cr.fill();
        cr.restore().ok();

        // Outer ring
        set_rgb(cr, border, 0.7);
        cr.set_line_width(1.5);
        cr.arc(cx, cy, r, 0.0, 2.0 * PI);
        let _ = cr.stroke();
    });

    area
}

/// What the map canvas shows: the composed basemap, the radar or satellite frames being
/// cross-faded, or a field drawn from grid points, and what goes over them.
pub struct MapScene<'a> {
    pub layer: RadarLayerKind,
    /// Where the map is; `None` until there's a place to show.
    pub geom: Option<MapGeom>,
    pub basemap: Option<&'a cairo::ImageSurface>,
    /// The frame showing, the next one, and how far into it the fade is (0–1).
    pub frames: Option<(&'a cairo::ImageSurface, Option<&'a cairo::ImageSurface>, f64)>,
    pub field: Option<&'a FieldGrid>,
    pub snap: Option<&'a ForecastSnapshot>,
    pub units: UnitPrefs,
    pub opacity: f64,
    pub anim_phase: f64,
    pub notice: Option<&'a str>,
    /// The data credits the services ask for.
    pub credit: &'a str,
    /// How much of the map's foot the controls over it cover.
    pub inset_bottom: f64,
}

/// Paint `s` (composed for a canvas of its own size) centred on a `w × h` canvas.
fn paint_centered(cr: &cairo::Context, s: &cairo::ImageSurface, w: f64, h: f64, alpha: f64) {
    let (sx, sy) = s.device_scale();
    let (sw, sh) = (s.width() as f64 / sx, s.height() as f64 / sy);
    if cr.set_source_surface(s, ((w - sw) / 2.0).round(), ((h - sh) / 2.0).round()).is_ok() {
        cr.source().set_filter(cairo::Filter::Bilinear);
        let _ = cr.paint_with_alpha(alpha);
    }
}

/// Interactive Radar, Satellite, Wind Streamlines, and Temperature/Isobar map canvas.
pub fn draw_radar_map(cr: &cairo::Context, w: f64, h: f64, scene: &MapScene) {
    let p = theme::palette();
    let wc = theme::weather_colors_for(&p);
    let bg = Rgb::hex(&p.bg);
    let surface = Rgb::hex(&p.surface);
    let text = Rgb::hex(&p.text);
    let dim = Rgb::hex(&p.dim_text);
    let layer = scene.layer;

    // Base background
    set_rgb(cr, surface.mix(bg, 0.35), 1.0);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();

    let cx = w * 0.5;
    let cy = h * 0.5;
    let pinch = scene.geom.map_or(1.0, |g| g.pinch);

    // 1. The basemap, then the frames cross-faded: each at its weight, added in a group
    //    (premultiplied, so that's an exact blend), the group at the layer's opacity.
    cr.save().ok();
    cr.translate(cx, cy);
    cr.scale(pinch, pinch);
    cr.translate(-cx, -cy);
    if let Some(b) = scene.basemap {
        paint_centered(cr, b, w, h, 1.0);
    } else {
        // Subtle cartographic graticule while tiles are loading
        set_rgb(cr, dim, 0.18);
        cr.set_line_width(1.0);
        for x in (0..(w as i32)).step_by(64) {
            cr.move_to(x as f64, 0.0);
            cr.line_to(x as f64, h);
        }
        for y in (0..(h as i32)).step_by(64) {
            cr.move_to(0.0, y as f64);
            cr.line_to(w, y as f64);
        }
        let _ = cr.stroke();
    }
    if let Some((a, b, f)) = scene.frames {
        cr.push_group();
        cr.set_operator(cairo::Operator::Add);
        let f = if b.is_some() { f.clamp(0.0, 1.0) } else { 0.0 };
        paint_centered(cr, a, w, h, 1.0 - f);
        if let Some(b) = b {
            paint_centered(cr, b, w, h, f);
        }
        let _ = cr.pop_group_to_source();
        cr.set_operator(cairo::Operator::Over);
        let _ = cr.paint_with_alpha(scene.opacity.clamp(0.15, 1.0));
    }
    cr.restore().ok();

    // 2. Fields drawn from the 7×7 Open-Meteo grid: wind arrows, or temperature shading
    //    with isobars.
    if let (Some(f), Some(l)) = (scene.field, scene.geom) {
        match layer {
            RadarLayerKind::WindStreamlines => draw_wind_field(cr, f, l, wc, scene.units, scene.opacity, scene.anim_phase),
            RadarLayerKind::TemperatureContour => draw_temp_field(cr, f, l, wc, text, scene.units, scene.opacity),
            _ => {}
        }
    }
    if layer == RadarLayerKind::PrecipitationRadar {
        reflectivity_legend(cr);
    }
    // 3. Center location target crosshair & live badge
    set_rgb(cr, wc.sun, 0.30);
    cr.arc(cx, cy, 14.0, 0.0, 2.0 * PI);
    let _ = cr.fill();
    set_rgb(cr, wc.sun, 1.0);
    cr.arc(cx, cy, 5.2, 0.0, 2.0 * PI);
    let _ = cr.fill();
    set_rgb(cr, bg, 1.0);
    cr.set_line_width(1.8);
    cr.arc(cx, cy, 5.2, 0.0, 2.0 * PI);
    let _ = cr.stroke();

    // The place's name beside its mark.
    if let Some(s) = scene.snap {
        cr.select_font_face(&ui_font(), cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        cr.set_font_size(12.0);
        let (x, y) = (cx + 18.0, cy + 4.5);
        set_rgb(cr, bg, 0.75);
        cr.set_line_width(3.5);
        cr.set_line_join(cairo::LineJoin::Round);
        cr.move_to(x, y);
        cr.text_path(&s.location.name);
        let _ = cr.stroke();
        set_rgb(cr, text, 1.0);
        cr.move_to(x, y);
        let _ = cr.show_text(&s.location.name);
    }

    // A note when the layer has nothing to show here (no radar coverage, field loading).
    if let Some(n) = scene.notice {
        cr.select_font_face(&ui_font(), cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(11.0);
        let tw = cr.text_extents(n).map(|e| e.x_advance()).unwrap_or(0.0);
        let (bx, by) = ((cx - tw / 2.0 - 10.0).max(8.0), h - scene.inset_bottom - 36.0);
        set_rgb(cr, bg, 0.88);
        cr.rectangle(bx, by, tw + 20.0, 26.0);
        let _ = cr.fill();
        set_rgb(cr, text, 0.92);
        cr.move_to(bx + 10.0, by + 17.0);
        let _ = cr.show_text(n);
    }

    // Map data credits, which the tile services ask for.
    let credit = scene.credit;
    cr.select_font_face(&ui_font(), cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(9.5);
    let tw = cr.text_extents(credit).map(|e| e.x_advance()).unwrap_or(0.0);
    let foot = h - scene.inset_bottom;
    set_rgb(cr, bg, 0.78);
    cr.rectangle(w - tw - 28.0, foot - 18.0, tw + 14.0, 18.0);
    let _ = cr.fill();
    set_rgb(cr, dim, 1.0);
    cr.move_to(w - tw - 21.0, foot - 5.5);
    let _ = cr.show_text(credit);
}

/// Wind arrows over the map, from the grid interpolated as vectors: each points where
/// the air is going, longer and warmer-coloured the stronger it blows, and drifts along
/// its direction with `anim_phase`.
fn draw_wind_field(
    cr: &cairo::Context,
    f: &FieldGrid,
    l: MapGeom,
    wc: theme::WeatherColors,
    units: UnitPrefs,
    opacity: f64,
    anim_phase: f64,
) {
    let max_speed = f.range(|p| p.wind_speed_kmh).map(|r| r.1).unwrap_or(0.0).max(1.0);
    let steps = 16;
    cr.set_line_width(2.0);
    cr.set_line_cap(cairo::LineCap::Round);
    for r in 0..=steps {
        for c in 0..=steps {
            let max = (f.size - 1) as f64;
            let (gx, gy) = (c as f64 / steps as f64 * max, r as f64 / steps as f64 * max);
            let (u, v) = f.wind(gx, gy);
            let speed = u.hypot(v);
            if !speed.is_finite() || speed < 0.5 {
                continue;
            }
            // Canvas y grows southwards.
            let (dx, dy) = (u / speed, -v / speed);
            let len = 10.0 + 22.0 * (speed / max_speed);
            let (x, y) = l.project_grid(f, gx, gy);
            let drift = (anim_phase + (c * 7 + r * 3) as f64 * 0.13).fract() * 10.0 - 5.0;
            let (sx, sy) = (x + dx * drift - dx * len / 2.0, y + dy * drift - dy * len / 2.0);
            let (ex, ey) = (sx + dx * len, sy + dy * len);
            let col = wc.wind.mix(wc.warm, (speed / max_speed).clamp(0.0, 1.0) * 0.7);
            set_rgb(cr, col, 0.85 * opacity);
            cr.move_to(sx, sy);
            cr.line_to(ex, ey);
            let ang = dy.atan2(dx);
            for side in [2.6, -2.6] {
                cr.move_to(ex, ey);
                cr.line_to(ex + (ang + side).cos() * 6.0, ey + (ang + side).sin() * 6.0);
            }
            let _ = cr.stroke();
        }
    }
    cr.set_line_cap(cairo::LineCap::Butt);
    legend(cr, &format!("Wind 10 m · up to {}", units.wind_speed.format(max_speed)), wc.wind);
}

/// Temperature shading (cold to warm across the map's own range) with sea-level
/// isobars every 2 hPa, labelled.
fn draw_temp_field(
    cr: &cairo::Context,
    f: &FieldGrid,
    l: MapGeom,
    wc: theme::WeatherColors,
    text: Rgb,
    units: UnitPrefs,
    opacity: f64,
) {
    let max = (f.size - 1) as f64;
    if let Some((lo, hi)) = f.range(|p| p.temperature_c) {
        let span = (hi - lo).max(1.0);
        let cells = 24;
        for r in 0..cells {
            for c in 0..cells {
                let (gx0, gy0) = (c as f64 / cells as f64 * max, r as f64 / cells as f64 * max);
                let (gx1, gy1) = ((c + 1) as f64 / cells as f64 * max, (r + 1) as f64 / cells as f64 * max);
                let t = f.sample((gx0 + gx1) / 2.0, (gy0 + gy1) / 2.0, |p| p.temperature_c);
                if !t.is_finite() {
                    continue;
                }
                let frac = ((t - lo) / span).clamp(0.0, 1.0);
                let col = if frac < 0.5 { wc.cold.mix(wc.sun, frac * 2.0) } else { wc.sun.mix(wc.danger, (frac - 0.5) * 2.0) };
                let (x0, y0) = l.project_grid(f, gx0, gy0);
                let (x1, y1) = l.project_grid(f, gx1, gy1);
                set_rgb(cr, col, 0.30 * opacity);
                cr.rectangle(x0, y0, x1 - x0 + 0.5, y1 - y0 + 0.5);
                let _ = cr.fill();
            }
        }
        legend(
            cr,
            &format!("2 m temperature {} – {}", units.temperature.format(lo), units.temperature.format(hi)),
            wc.sun,
        );
    }

    let Some((plo, phi)) = f.range(|p| p.pressure_hpa) else { return };
    cr.select_font_face(&ui_font(), cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(10.0);
    let mut level = (plo / 2.0).ceil() * 2.0;
    while level <= phi {
        let segs = f.contour(level, |p| p.pressure_hpa);
        set_rgb(cr, text, 0.55 * opacity);
        cr.set_line_width(1.3);
        for [a, b] in &segs {
            let (x0, y0) = l.project_grid(f, a.0, a.1);
            let (x1, y1) = l.project_grid(f, b.0, b.1);
            cr.move_to(x0, y0);
            cr.line_to(x1, y1);
        }
        let _ = cr.stroke();
        // Label the isobar once, near its middle.
        if let Some([a, _]) = segs.get(segs.len() / 2) {
            let (x, y) = l.project_grid(f, a.0, a.1);
            set_rgb(cr, text, 0.9);
            cr.move_to(x + 4.0, y - 4.0);
            let _ = cr.show_text(&units.pressure.format(level));
        }
        level += 2.0;
    }
}

/// A one-line key in the map's bottom-left corner.
fn legend(cr: &cairo::Context, label: &str, col: Rgb) {
    let p = theme::palette();
    cr.select_font_face(&ui_font(), cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(10.5);
    let tw = cr.text_extents(label).map(|e| e.x_advance()).unwrap_or(0.0);
    let (x, y) = (14.0, 14.0);
    key_pill(cr, &p, x, y, tw + 30.0);
    set_rgb(cr, col, 1.0);
    cr.arc(x + 11.0, y + 11.0, 4.0, 0.0, 2.0 * PI);
    let _ = cr.fill();
    set_rgb(cr, Rgb::hex(&p.text), 0.95);
    cr.move_to(x + 20.0, y + 15.0);
    let _ = cr.show_text(label);
}

/// A key's background on the map: a rounded pill in the popover colour.
fn key_pill(cr: &cairo::Context, p: &theme::Palette, x: f64, y: f64, w: f64) {
    let r = 6.0;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + 22.0 - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + 22.0 - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
    set_rgb(cr, Rgb::hex(&p.bg), 0.9);
    let _ = cr.fill();
}

/// The radar key in the map's top-left corner: the NWS scale from light to heavy.
fn reflectivity_legend(cr: &cairo::Context) {
    let p = theme::palette();
    cr.select_font_face(&ui_font(), cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(10.5);
    let (light, heavy) = ("Light", "Heavy");
    let lw = cr.text_extents(light).map(|e| e.x_advance()).unwrap_or(0.0);
    let hw = cr.text_extents(heavy).map(|e| e.x_advance()).unwrap_or(0.0);
    let bar = 120.0;
    let (x, y) = (14.0, 14.0);
    key_pill(cr, &p, x, y, lw + bar + hw + 36.0);
    set_rgb(cr, Rgb::hex(&p.text), 0.95);
    cr.move_to(x + 10.0, y + 15.0);
    let _ = cr.show_text(light);
    let bx = x + 18.0 + lw;
    for i in 0..bar as i32 {
        let dbz = 15.0 + 60.0 * i as f32 / bar as f32;
        let [r, g, b, _] = nws_color(Echo { dbz, snow: false });
        cr.set_source_rgb(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
        cr.rectangle(bx + i as f64, y + 7.0, 1.0, 8.0);
        let _ = cr.fill();
    }
    set_rgb(cr, Rgb::hex(&p.text), 0.95);
    cr.move_to(bx + bar + 8.0, y + 15.0);
    let _ = cr.show_text(heavy);
}
