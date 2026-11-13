//! Today: alerts in a calm banner with Details, the conditions and the next two hours
//! of rain in one card, a 24-hour strip with a temperature line, six readings as
//! tiles (each opens its details), the sun and moon, and all ten days beside them.

use crate::sections::{format_clock_label, format_duration_secs, format_sun_time};
use crate::theme::{self, Rgb};
use crate::widgets::{self, Page};
use crate::{backend, prefs, window};
use chrono::NaiveDate;
use gtk::prelude::*;
use gtk::{cairo, glib};
use nyx_weather_core::{
    AlertPeriod, ForecastSnapshot, UnitPrefs, WeatherAlert, beaufort_scale, calculate_moon_phase, cardinal_direction, pollen_category,
    us_aqi_category, uv_category,
};
use std::cell::Cell;
use std::rc::Rc;

pub fn build(page: &Page, snap: Option<&ForecastSnapshot>, stale_fallback: bool, last_error: Option<&str>) {
    let p = prefs::get();
    let units = p.units();

    let Some(snap) = snap else {
        super::no_forecast(
            page,
            "Today",
            "weather-overcast-symbolic",
            "Loading the forecast…",
            "Fetching the conditions, the next hours and ten days from Open-Meteo.",
            last_error,
        );
        return;
    };
    let narrow = window::is_narrow();
    page.body.add_css_class("today");
    // The two columns are wider than a narrow window; without this their width would
    // hold the window above the narrow threshold, and the one-column layout never show.
    page.root.set_hscrollbar_policy(gtk::PolicyType::External);

    for alert in &snap.alerts {
        page.body.append(&alert_banner(alert, units.use_24h_time));
    }
    if snap.alerts_unavailable {
        let b = widgets::banner("Severe-weather alerts are unavailable right now; NOAA didn't answer.", false);
        b.add_css_class("today-banner");
        page.body.append(&b);
    }
    if stale_fallback {
        let msg =
            format!("{} — showing the forecast from {}.", last_error.unwrap_or("Offline"), backend::human_age(snap.fetched_at));
        let b = super::retry_banner(&msg);
        b.add_css_class("today-banner");
        page.body.append(&b);
    }

    let cols = gtk::Box::new(if narrow { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal }, 16);
    let left = widgets::vbox(0);
    left.set_hexpand(true);
    left.append(&hero(snap, &units, window::is_small()));
    left.append(&sechead("Next 24 hours", Some(("Hourly", "hourly", "48 hours, hour by hour (Ctrl+2)"))));
    left.append(&hour_strip(snap, &units));
    left.append(&sechead("Right now", None));
    left.append(&readings(snap, &units, narrow));
    left.append(&sky_row(snap, &units, narrow));
    cols.append(&left);

    let right = widgets::vbox(0);
    right.add_css_class("today-days");
    if !narrow {
        right.set_width_request(360);
        right.set_hexpand(false);
    }
    right.append(&sechead("10 days", Some(("Outlook", "daily", "Each day in full (Ctrl+3)"))));
    right.append(&ten_days(snap, &units));
    if let Some(text) = week_summary(snap, &units) {
        let card = widgets::vbox(2);
        card.add_css_class("week-card");
        card.append(&widgets::label("This week", "week-title"));
        let body = widgets::label(&text, "week-text");
        body.set_wrap(true);
        body.set_max_width_chars(36);
        body.set_xalign(0.0);
        card.append(&body);
        right.append(&card);
    }
    cols.append(&right);
    page.body.append(&cols);
}

/// A heading over a part of the page, with a link to its full page at the right.
fn sechead(title: &str, more: Option<(&str, &'static str, &str)>) -> gtk::Box {
    let row = widgets::hbox(8);
    row.add_css_class("sechead");
    let l = widgets::label(title, "sechead-title");
    l.set_hexpand(true);
    l.set_xalign(0.0);
    row.append(&l);
    if let Some((label, section, tip)) = more {
        row.append(&link(label, section, tip));
    }
    row
}

/// A quiet accent link to another page.
fn link(label: &str, section: &'static str, tip: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("flat");
    b.add_css_class("more-link");
    b.set_valign(gtk::Align::Center);
    b.set_tooltip_text(Some(tip));
    b.connect_clicked(move |_| window::navigate(section));
    b
}

// ---------- The alert ----------

/// One period: "Until Thu 8:00 AM", or "Fri 1:00 AM to 9:00 AM" when it hasn't started.
fn period_when(p: &AlertPeriod, now: i64, use_24h: bool) -> Option<String> {
    let parse = |iso: &Option<String>| chrono::DateTime::parse_from_rfc3339(iso.as_deref()?).ok();
    let (day, time) = if use_24h { ("%a %H:%M", "%H:%M") } else { ("%a %-I:%M %p", "%-I:%M %p") };
    match (parse(&p.onset), parse(&p.ends)) {
        (Some(from), Some(to)) if from.timestamp() > now => {
            let to_fmt = if from.date_naive() == to.date_naive() { time } else { day };
            Some(format!("{} to {}", from.format(day), to.format(to_fmt)))
        }
        (_, Some(to)) => Some(format!("Until {}", to.format(day))),
        (Some(from), None) => Some(format!("From {}", from.format(day))),
        _ => None,
    }
}

/// When an alert is in force, each period in turn: "Until Sat 8:00 AM, then Sun 12:00 AM
/// to 8:00 AM".
fn alert_when(alert: &WeatherAlert, use_24h: bool) -> Option<String> {
    let now = backend::now_epoch();
    let parts: Vec<String> = alert.in_force().iter().filter_map(|p| period_when(p, now, use_24h)).collect();
    (!parts.is_empty()).then(|| parts.join(", then "))
}

/// A severe-weather alert: its event and when, calmly, with Details for the rest.
fn alert_banner(alert: &WeatherAlert, use_24h: bool) -> gtk::Box {
    let banner = widgets::hbox(12);
    banner.add_css_class("alert-banner");
    if alert.severity >= nyx_weather_core::AlertSeverity::Extreme {
        banner.add_css_class("extreme");
    }
    let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
    icon.add_css_class("alert-icon");
    banner.append(&icon);

    let col = widgets::vbox(1);
    col.set_hexpand(true);
    col.set_valign(gtk::Align::Center);
    let title = widgets::label(&alert.event, "alert-title");
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    col.append(&title);
    let sub: Vec<String> = [alert_when(alert, use_24h), alert.sender.clone()].into_iter().flatten().collect();
    if !sub.is_empty() {
        let s = widgets::label(&sub.join(" · "), "alert-sub");
        s.set_xalign(0.0);
        s.set_ellipsize(gtk::pango::EllipsizeMode::End);
        col.append(&s);
    }
    banner.set_tooltip_text(Some(&alert.headline));
    banner.append(&col);

    let details = gtk::Button::with_label("Details");
    details.add_css_class("alert-details");
    details.set_valign(gtk::Align::Center);
    let alert = alert.clone();
    details.connect_clicked(move |_| alert_dialog(&alert, use_24h));
    banner.append(&details);
    banner
}

fn alert_dialog(alert: &WeatherAlert, use_24h: bool) {
    let (dialog, body) = widgets::dialog(&alert.event, 560);
    if let Some(w) = window::window() {
        dialog.set_transient_for(Some(&w));
    }
    let col = widgets::vbox(10);
    let head = widgets::label(&alert.headline, "alert-headline");
    head.set_wrap(true);
    head.set_xalign(0.0);
    col.append(&head);
    let mut meta = vec![alert.severity.label().to_string()];
    meta.extend(alert_when(alert, use_24h));
    meta.extend(alert.sender.clone());
    let m = widgets::label(&meta.join(" · "), "dim");
    m.set_wrap(true);
    m.set_xalign(0.0);
    col.append(&m);
    if !alert.area.is_empty() {
        let area = widgets::label(&alert.area, "dim");
        area.set_wrap(true);
        area.set_xalign(0.0);
        col.append(&area);
    }
    for text in [Some(alert.description.as_str()), alert.instruction.as_deref()].into_iter().flatten() {
        if text.trim().is_empty() {
            continue;
        }
        let l = widgets::label(text.trim(), "alert-body");
        l.set_wrap(true);
        l.set_xalign(0.0);
        l.set_selectable(true);
        col.append(&l);
    }
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .propagate_natural_height(true)
        .max_content_height(480)
        .child(&col)
        .build();
    body.append(&scroll);
    let actions = widgets::hbox(8);
    actions.set_halign(gtk::Align::End);
    let close = gtk::Button::with_label("Close");
    let d2 = dialog.clone();
    close.connect_clicked(move |_| d2.close());
    actions.append(&close);
    body.append(&actions);
    dialog.present();
}

// ---------- The conditions and the next two hours ----------

fn hero(snap: &ForecastSnapshot, units: &UnitPrefs, stacked: bool) -> gtk::Box {
    let cur = &snap.current;
    let (hi_c, lo_c) = snap.today_high_low_c();
    let hero = gtk::Box::new(if stacked { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal }, 18);
    hero.add_css_class("today-hero");

    let main = widgets::vbox(8);
    main.set_hexpand(true);
    let place = widgets::hbox(8);
    let name = widgets::label(&snap.location.name, "hero-place");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    place.append(&name);
    let local = nyx_weather_core::local_time(backend::now_epoch(), snap.utc_offset_seconds);
    let clock = local.format(if units.use_24h_time { "%H:%M" } else { "%-I:%M %p" }).to_string();
    let region = snap.location.admin1.clone().or_else(|| snap.location.country.clone());
    let sub = [region, Some(clock)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let sub_lbl = widgets::label(&sub, "hero-place-sub");
    sub_lbl.set_hexpand(true);
    sub_lbl.set_xalign(0.0);
    sub_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
    sub_lbl.set_tooltip_text(Some(&snap.location.display_name()));
    place.append(&sub_lbl);
    let p = prefs::get();
    if !p.saved_locations.iter().any(|l| l.id == snap.location.id) {
        let save = widgets::labeled_button("list-add-symbolic", "Save");
        save.set_tooltip_text(Some("Save this place"));
        let loc = snap.location.clone();
        save.connect_clicked(move |_| window::save_and_select_location(loc.clone()));
        place.append(&save);
    }
    main.append(&place);

    let now = widgets::hbox(12);
    let icon = gtk::Image::from_icon_name(cur.condition.icon_name(cur.is_day));
    icon.set_pixel_size(44);
    icon.add_css_class(theme::condition_color_class(cur.condition, cur.is_day));
    now.append(&icon);
    now.append(&widgets::label(&units.temperature.format(cur.temperature_c), "hero-temp"));
    let cond = widgets::vbox(2);
    cond.set_valign(gtk::Align::Center);
    cond.set_hexpand(true);
    let c = widgets::label(cur.condition.label(), "hero-condition");
    c.set_xalign(0.0);
    c.set_ellipsize(gtk::pango::EllipsizeMode::End);
    cond.append(&c);
    let meta = widgets::label(
        &format!(
            "Feels like {} · H {} L {}",
            units.temperature.format(cur.apparent_temperature_c),
            units.temperature.format(hi_c),
            units.temperature.format(lo_c)
        ),
        "hero-meta",
    );
    meta.set_xalign(0.0);
    meta.set_wrap(true);
    cond.append(&meta);
    now.append(&cond);
    main.append(&now);
    hero.append(&main);

    if !snap.minutely_precip.is_empty() {
        let rain = widgets::vbox(4);
        rain.add_css_class("hero-rain");
        rain.set_hexpand(stacked);
        if stacked {
            rain.add_css_class("stacked");
        } else {
            rain.set_width_request(if window::is_narrow() { 190 } else { 210 });
        }
        let key = widgets::hbox(6);
        key.add_css_class("hero-rain-key");
        key.append(&gtk::Image::from_icon_name("nyx-weather-umbrella-symbolic"));
        key.append(&widgets::label("Next two hours", ""));
        rain.append(&key);
        let summary = snap.rain_nowcast().unwrap_or_else(|| "No rain expected".into());
        let s = widgets::label(&summary, "hero-rain-summary");
        s.set_xalign(0.0);
        s.set_ellipsize(gtk::pango::EllipsizeMode::End);
        rain.append(&s);
        let steps: Vec<f64> = snap.minutely_precip.iter().take(8).map(|(_, mm)| *mm).collect();
        rain.append(&rain_bars(steps));
        let axis = gtk::CenterBox::new();
        axis.add_css_class("hero-rain-axis");
        axis.set_start_widget(Some(&widgets::label("Now", "")));
        axis.set_center_widget(Some(&widgets::label("+1 h", "")));
        axis.set_end_widget(Some(&widgets::label("+2 h", "")));
        rain.append(&axis);
        hero.append(&rain);
    }
    hero
}

/// The nowcast's 15-minute steps as bars: a hairline when dry, up to full height at
/// 2 mm in 15 minutes (heavy rain).
fn rain_bars(steps: Vec<f64>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_height(34);
    area.set_hexpand(true);
    area.set_margin_top(4);
    let a = area.downgrade();
    theme::subscribe(&area, move || {
        if let Some(a) = a.upgrade() {
            a.queue_draw();
        }
    });
    area.set_draw_func(move |_, cr, w, h| {
        let p = theme::palette();
        let rain = theme::swatch("blue").unwrap_or_else(|| theme::weather_colors_for(&p).cold);
        let (w, h) = (w as f64, h as f64);
        let n = steps.len().max(1) as f64;
        let gap = 3.0;
        let bw = ((w - gap * (n - 1.0)) / n).max(1.0);
        for (i, mm) in steps.iter().enumerate() {
            let frac = (mm / 2.0).clamp(0.0, 1.0);
            let bh = (frac * h).max(3.0);
            let x = i as f64 * (bw + gap);
            cr.set_source_rgba(rain.0, rain.1, rain.2, if *mm >= 0.1 { 0.85 } else { 0.35 });
            rounded(cr, x, h - bh, bw, bh, 1.0_f64.min(bh / 2.0));
            let _ = cr.fill();
        }
    });
    area
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

// ---------- The next 24 hours ----------

/// Each hour's time, sky and chance of rain, a line through the temperatures, and the
/// temperatures. As many hours as the width holds show, each as wide as the others.
fn hour_strip(snap: &ForecastSnapshot, units: &UnitPrefs) -> gtk::Box {
    let hours = snap.upcoming_hours(24);
    let strip = widgets::vbox(0);
    strip.add_css_class("hour-strip");
    let cells = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    cells.set_homogeneous(true);
    let values = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    values.set_homogeneous(true);
    let mut cell_widgets = Vec::new();
    let mut value_widgets = Vec::new();
    for (i, h) in hours.iter().enumerate() {
        let cell = widgets::vbox(5);
        cell.add_css_class("hour-cell");
        if i == 0 {
            cell.add_css_class("now");
        }
        let t = if i == 0 { "Now".to_string() } else { format_clock_label(&h.time, units.use_24h_time) };
        cell.append(&centred(&t, "hour-time"));
        let icon = gtk::Image::from_icon_name(h.condition.icon_name(h.is_day));
        icon.set_pixel_size(16);
        icon.add_css_class(theme::condition_color_class(h.condition, h.is_day));
        cell.append(&icon);
        let pop =
            if h.precipitation_probability_pct >= 10 { format!("{}%", h.precipitation_probability_pct) } else { String::new() };
        let pop = centred(&pop, "hour-pop");
        pop.add_css_class("section-swatch-blue");
        cell.append(&pop);
        cell.set_tooltip_text(Some(h.condition.label()));
        cells.append(&cell);
        cell_widgets.push(cell);
        let v = centred(&units.temperature.format(h.temperature_c), "hour-temp");
        values.append(&v);
        value_widgets.push(v);
    }
    strip.append(&cells);

    let temps: Vec<f64> = hours.iter().map(|h| h.temperature_c).collect();
    let shown = Rc::new(Cell::new(temps.len().min(10)));
    let line = gtk::DrawingArea::new();
    line.add_css_class("hour-line");
    line.set_content_height(34);
    line.set_hexpand(true);
    {
        let shown = shown.clone();
        let temps = temps.clone();
        line.set_draw_func(move |_, cr, w, h| {
            let n = shown.get().min(temps.len());
            if n < 2 {
                return;
            }
            let t = &temps[..n];
            let lo = t.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = t.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let span = (hi - lo).max(4.0);
            let mid = (hi + lo) / 2.0;
            let (w, h) = (w as f64, h as f64);
            let y = |v: f64| h / 2.0 - (v - mid) / span * (h - 8.0);
            let p = theme::palette();
            let c: Rgb = theme::swatch("amber").unwrap_or_else(|| theme::weather_colors_for(&p).sun);
            cr.set_source_rgba(c.0, c.1, c.2, 1.0);
            cr.set_line_width(2.0);
            cr.set_line_join(cairo::LineJoin::Round);
            cr.set_line_cap(cairo::LineCap::Round);
            for (i, v) in t.iter().enumerate() {
                let x = (i as f64 + 0.5) / n as f64 * w;
                if i == 0 {
                    cr.move_to(x, y(*v));
                } else {
                    cr.line_to(x, y(*v));
                }
            }
            let _ = cr.stroke();
        });
    }
    let a = line.downgrade();
    theme::subscribe(&line, move || {
        if let Some(a) = a.upgrade() {
            a.queue_draw();
        }
    });
    // The line spans the strip, so its width says how many hours fit.
    line.connect_resize(move |area, w, _| {
        let n = ((w as f64 / 60.0).floor() as usize).clamp(4, cell_widgets.len().max(4)).min(cell_widgets.len());
        if n == shown.get() && cell_widgets.iter().filter(|c| c.is_visible()).count() == n {
            return;
        }
        shown.set(n);
        let (cells, values, area) = (cell_widgets.clone(), value_widgets.clone(), area.clone());
        glib::idle_add_local_once(move || {
            for (i, (c, v)) in cells.iter().zip(&values).enumerate() {
                c.set_visible(i < n);
                v.set_visible(i < n);
            }
            area.queue_draw();
        });
    });
    strip.append(&line);
    strip.append(&values);
    strip
}

fn centred(text: &str, class: &str) -> gtk::Label {
    let l = widgets::label(text, class);
    l.set_xalign(0.5);
    l.set_halign(gtk::Align::Center);
    l
}

// ---------- Right now: six readings ----------

/// "NW" as words: "north-west".
fn direction_words(cardinal: &str) -> String {
    cardinal
        .chars()
        .map(|c| match c {
            'N' => "north",
            'E' => "east",
            'S' => "south",
            _ => "west",
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn dash() -> String {
    "–".into()
}

struct Reading {
    icon: &'static str,
    swatch: &'static str,
    key: &'static str,
    value: String,
    sub: String,
    /// The AQI's place on its scale, 0 to 1, drawn as a marker on the scale's colours.
    scale: Option<f64>,
    details: Vec<(&'static str, String)>,
    more: Option<(&'static str, &'static str)>,
}

fn readings(snap: &ForecastSnapshot, units: &UnitPrefs, narrow: bool) -> gtk::Grid {
    let cur = &snap.current;
    let today = snap.daily.first();
    let fmt_t = |c: f64| units.temperature.format(c);

    let dir = cardinal_direction(cur.wind_direction_deg);
    let gusts = cur.wind_gusts_kmh.map(|g| units.wind_speed.format(g));
    let (bft, bft_label) = beaufort_scale(cur.wind_speed_kmh);
    let wind = Reading {
        icon: "weather-windy-symbolic",
        swatch: "teal",
        key: "Wind",
        value: units.wind_speed.format(cur.wind_speed_kmh),
        sub: match &gusts {
            Some(g) => format!("From the {} · gusts {g}", direction_words(dir)),
            None => format!("From the {}", direction_words(dir)),
        },
        scale: None,
        details: vec![
            ("Sustained wind", units.wind_speed.format(cur.wind_speed_kmh)),
            ("Gusts", gusts.clone().unwrap_or_else(dash)),
            ("Direction", format!("{}° ({dir})", cur.wind_direction_deg)),
            ("Beaufort scale", format!("{bft} — {bft_label}")),
            ("Today's strongest", today.map(|d| units.wind_speed.format(d.wind_speed_max_kmh)).unwrap_or_else(dash)),
        ],
        more: None,
    };

    let dew = cur.dew_point_c.map(fmt_t);
    let humidity = Reading {
        icon: "nyx-weather-drop-symbolic",
        swatch: "blue",
        key: "Humidity",
        value: format!("{}%", cur.humidity_pct),
        sub: dew.as_ref().map(|d| format!("Dew point {d}")).unwrap_or_default(),
        scale: None,
        details: vec![
            ("Relative humidity", format!("{}%", cur.humidity_pct)),
            ("Dew point", cur.dew_point_c.map(|d| units.temperature.format_with_unit(d)).unwrap_or_else(dash)),
            ("Cloud cover", format!("{}%", cur.cloud_cover_pct)),
            ("Visibility", cur.visibility_m.map(|v| units.distance.format_m(v)).unwrap_or_else(dash)),
        ],
        more: None,
    };

    let later = snap.hourly.get(3).and_then(|h| h.pressure_hpa);
    let trend = match (cur.pressure_hpa, later) {
        (Some(now), Some(then)) => {
            let d = then - now;
            Some(match d {
                d if d >= 1.5 => "Rising",
                d if d >= 0.5 => "Rising slowly",
                d if d <= -1.5 => "Falling",
                d if d <= -0.5 => "Falling slowly",
                _ => "Steady",
            })
        }
        _ => None,
    };
    let pressure = Reading {
        icon: "nyx-weather-gauge-symbolic",
        swatch: "violet",
        key: "Pressure",
        value: cur.pressure_hpa.map(|p| units.pressure.format(p)).unwrap_or_else(dash),
        sub: trend.unwrap_or("").to_string(),
        scale: None,
        details: vec![
            ("Surface pressure", cur.pressure_hpa.map(|p| units.pressure.format(p)).unwrap_or_else(dash)),
            ("In three hours", later.map(|p| units.pressure.format(p)).unwrap_or_else(dash)),
        ],
        more: Some(("Hourly", "hourly")),
    };

    let uv_advice = match cur.uv_index {
        None => "No reading",
        Some(x) if x < 3.0 => "No protection needed",
        Some(x) if x < 6.0 => "Shade and SPF 30 at midday",
        Some(x) if x < 8.0 => "Cover up around midday",
        Some(_) => "Avoid the midday sun",
    };
    let uv = Reading {
        icon: "weather-clear-symbolic",
        swatch: "amber",
        key: "UV",
        value: cur.uv_index.map(|u| format!("{u:.0} · {}", uv_category(u))).unwrap_or_else(dash),
        sub: uv_advice.to_string(),
        scale: None,
        details: vec![
            ("Now", cur.uv_index.map(|u| format!("{u:.1} ({})", uv_category(u))).unwrap_or_else(dash)),
            (
                "Today's highest",
                today.and_then(|d| d.uv_index_max).map(|u| format!("{u:.1} ({})", uv_category(u))).unwrap_or_else(dash),
            ),
            ("Guidance", uv_advice.to_string()),
        ],
        more: None,
    };

    let aq = snap.air_quality.clone().unwrap_or_default();
    let pollen = [aq.grass_pollen, aq.birch_pollen, aq.alder_pollen, aq.ragweed_pollen, aq.olive_pollen, aq.mugwort_pollen]
        .into_iter()
        .flatten()
        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))));
    let ug = |v: Option<f64>| v.map(|v| format!("{v:.1} µg/m³")).unwrap_or_else(dash);
    let air = Reading {
        icon: "nyx-weather-leaf-symbolic",
        swatch: "green",
        key: "Air quality",
        value: aq.us_aqi.map(|v| format!("{v} · {}", us_aqi_category(v))).unwrap_or_else(|| "Unavailable".into()),
        sub: match (pollen, aq.pm2_5) {
            (Some(g), _) => format!("Pollen {}", pollen_category(g).to_lowercase()),
            (None, Some(pm)) => format!("PM2.5 {pm:.0} µg/m³"),
            _ => String::new(),
        },
        scale: aq.us_aqi.map(|v| (v as f64 / 300.0).clamp(0.0, 1.0)),
        details: vec![
            ("US AQI", aq.us_aqi.map(|v| format!("{v} ({})", us_aqi_category(v))).unwrap_or_else(dash)),
            ("European AQI", aq.european_aqi.map(|v| v.to_string()).unwrap_or_else(dash)),
            ("PM2.5", ug(aq.pm2_5)),
            ("PM10", ug(aq.pm10)),
            ("Ozone", ug(aq.ozone)),
            ("Nitrogen dioxide", ug(aq.nitrogen_dioxide)),
        ],
        more: Some(("Air and pollen", "air-quality")),
    };

    let sr = format_sun_time(today.and_then(|d| d.sunrise.as_deref()), units.use_24h_time);
    let ss = format_sun_time(today.and_then(|d| d.sunset.as_deref()), units.use_24h_time);
    let daylight = today.and_then(|d| d.daylight_duration_secs);
    let sunset = Reading {
        icon: "daytime-sunset-symbolic",
        swatch: "coral",
        key: "Sunset",
        value: ss.clone(),
        sub: match daylight {
            Some(_) => format!("Sunrise {sr} · {}", format_duration_secs(daylight)),
            None => format!("Sunrise {sr}"),
        },
        scale: None,
        details: vec![
            ("Sunrise", sr),
            ("Sunset", ss),
            ("Daylight", format_duration_secs(daylight)),
            ("Sunshine", format_duration_secs(today.and_then(|d| d.sunshine_duration_secs))),
            ("Time zone", snap.location.timezone.clone()),
        ],
        more: Some(("Sun and moon", "astronomy")),
    };

    let grid = gtk::Grid::new();
    grid.add_css_class("readings");
    grid.set_column_homogeneous(true);
    grid.set_column_spacing(10);
    grid.set_row_spacing(10);
    let cols = if narrow { 2 } else { 3 };
    for (i, r) in [wind, humidity, pressure, uv, air, sunset].into_iter().enumerate() {
        grid.attach(&reading_tile(r), (i % cols) as i32, (i / cols) as i32, 1, 1);
    }
    grid
}

/// A reading as a tile; clicking it shows the reading's details.
fn reading_tile(r: Reading) -> gtk::MenuButton {
    let col = widgets::vbox(6);
    let key = widgets::hbox(6);
    key.add_css_class("reading-key");
    let icon = gtk::Image::from_icon_name(r.icon);
    icon.add_css_class(&format!("section-swatch-{}", r.swatch));
    key.append(&icon);
    key.append(&widgets::label(r.key, ""));
    col.append(&key);
    let v = widgets::label(&r.value, "reading-value");
    v.set_xalign(0.0);
    v.set_ellipsize(gtk::pango::EllipsizeMode::End);
    col.append(&v);
    if let Some(at) = r.scale {
        col.append(&aqi_scale(at));
    }
    let s = widgets::label(&r.sub, "reading-sub");
    s.set_xalign(0.0);
    s.set_ellipsize(gtk::pango::EllipsizeMode::End);
    s.set_tooltip_text(Some(&r.sub));
    col.append(&s);

    let details = widgets::vbox(6);
    details.add_css_class("reading-details");
    details.append(&widgets::label(r.key, "reading-details-title"));
    for (k, val) in &r.details {
        let row = widgets::hbox(16);
        let kl = widgets::label(k, "dim");
        kl.set_hexpand(true);
        kl.set_xalign(0.0);
        row.append(&kl);
        let vl = widgets::label(val, "tnum");
        vl.set_xalign(1.0);
        vl.set_wrap(true);
        vl.set_max_width_chars(28);
        row.append(&vl);
        details.append(&row);
    }
    let pop = gtk::Popover::new();
    pop.set_child(Some(&details));
    if let Some((label, section)) = r.more {
        let b = gtk::Button::with_label(label);
        b.add_css_class("flat");
        b.add_css_class("more-link");
        b.set_halign(gtk::Align::End);
        let p2 = pop.downgrade();
        b.connect_clicked(move |_| {
            if let Some(p) = p2.upgrade() {
                p.popdown();
            }
            window::navigate(section);
        });
        details.append(&b);
    }

    let tile = gtk::MenuButton::new();
    tile.add_css_class("reading");
    tile.set_child(Some(&col));
    tile.set_popover(Some(&pop));
    tile.set_always_show_arrow(false);
    tile.set_tooltip_text(Some(&format!("{} in detail", r.key)));
    tile
}

/// The US AQI's colours, good to hazardous, with a marker where the air is.
fn aqi_scale(at: f64) -> gtk::DrawingArea {
    const STOPS: [&str; 6] = ["#59d45b", "#eeb43f", "#f5875a", "#f2655f", "#c95fc6", "#8b3a62"];
    let area = gtk::DrawingArea::new();
    area.set_content_height(11);
    area.set_hexpand(true);
    let a = area.downgrade();
    theme::subscribe(&area, move || {
        if let Some(a) = a.upgrade() {
            a.queue_draw();
        }
    });
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        let grad = cairo::LinearGradient::new(0.0, 0.0, w, 0.0);
        for (i, c) in STOPS.iter().enumerate() {
            let c = Rgb::hex(c);
            grad.add_color_stop_rgb(i as f64 / (STOPS.len() - 1) as f64, c.0, c.1, c.2);
        }
        let y = (h - 5.0) / 2.0;
        rounded(cr, 0.0, y, w, 5.0, 2.5);
        let _ = cr.set_source(&grad);
        let _ = cr.fill();
        let p = theme::palette();
        let x = (at * w).clamp(2.0, w - 2.0);
        let card = Rgb::hex(&p.surface);
        cr.set_source_rgba(card.0, card.1, card.2, 1.0);
        rounded(cr, x - 3.5, 0.0, 7.0, h, 2.5);
        let _ = cr.fill();
        let text = Rgb::hex(&p.text);
        cr.set_source_rgba(text.0, text.1, text.2, 1.0);
        rounded(cr, x - 2.0, 1.5, 4.0, h - 3.0, 2.0);
        let _ = cr.fill();
    });
    area
}

// ---------- The sun and moon ----------

fn sky_row(snap: &ForecastSnapshot, units: &UnitPrefs, narrow: bool) -> gtk::Box {
    let today = snap.daily.first();
    let row = widgets::hbox(18);
    row.add_css_class("sky-row");
    let item = |icon: &str, swatch: &str, text: &str| {
        let b = widgets::hbox(7);
        let i = gtk::Image::from_icon_name(icon);
        i.add_css_class(&format!("section-swatch-{swatch}"));
        b.append(&i);
        let l = widgets::label(text, "");
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        l.set_tooltip_text(Some(text));
        b.append(&l);
        b
    };
    row.append(&item(
        "daytime-sunrise-symbolic",
        "amber",
        &format_sun_time(today.and_then(|d| d.sunrise.as_deref()), units.use_24h_time),
    ));
    row.append(&item(
        "daytime-sunset-symbolic",
        "amber",
        &format_sun_time(today.and_then(|d| d.sunset.as_deref()), units.use_24h_time),
    ));
    let moon = calculate_moon_phase(backend::now_epoch());
    let m = item(
        "weather-clear-night-symbolic",
        "violet",
        &format!("{} · {}%", capitalise(&moon.phase_name.to_lowercase()), moon.illumination_pct),
    );
    m.set_hexpand(true);
    row.append(&m);
    if !narrow {
        row.append(&link("Sun and moon", "astronomy", "The sun's path and the moon's phase (Ctrl+6)"));
    }
    row
}

// ---------- Ten days ----------

fn ten_days(snap: &ForecastSnapshot, units: &UnitPrefs) -> gtk::Box {
    let card = widgets::vbox(0);
    card.add_css_class("card");
    card.add_css_class("ten-days");
    let days: Vec<_> = snap.daily.iter().take(10).collect();
    let lo = days.iter().map(|d| d.temp_min_c).fold(f64::INFINITY, f64::min);
    let hi = days.iter().map(|d| d.temp_max_c).fold(f64::NEG_INFINITY, f64::max);
    let span = (hi - lo).max(1.0);
    for (i, d) in days.iter().enumerate() {
        let row = widgets::hbox(10);
        row.add_css_class("day-row");
        if i == 0 {
            row.add_css_class("now");
        }
        let name = widgets::label(&super::format_day_name(&d.date, i), "day-name");
        name.set_width_chars(5);
        name.set_xalign(0.0);
        row.append(&name);
        let icon = gtk::Image::from_icon_name(d.condition.icon_name(true));
        icon.add_css_class(theme::condition_color_class(d.condition, true));
        row.append(&icon);
        let pop = widgets::hbox(3);
        pop.add_css_class("day-pop");
        pop.add_css_class("section-swatch-blue");
        pop.set_size_request(50, -1);
        if d.precipitation_probability_max_pct >= 10 {
            let drop = gtk::Image::from_icon_name("nyx-weather-drop-symbolic");
            drop.set_pixel_size(11);
            pop.append(&drop);
            pop.append(&widgets::label(&format!("{}%", d.precipitation_probability_max_pct), ""));
        }
        row.append(&pop);
        let min = widgets::label(&units.temperature.format(d.temp_min_c), "day-lo");
        min.set_width_chars(4);
        min.set_xalign(1.0);
        row.append(&min);
        row.append(&range_bar(super::range_fracs(d.temp_min_c, d.temp_max_c, lo, span)));
        let max = widgets::label(&units.temperature.format(d.temp_max_c), "day-hi");
        max.set_width_chars(4);
        max.set_xalign(1.0);
        row.append(&max);
        row.set_tooltip_text(Some(d.condition.label()));
        card.append(&row);
    }
    card
}

/// A day's low to high on the ten days' range, cold to warm.
fn range_bar((start, len): (f64, f64)) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_height(5);
    area.set_content_width(40);
    area.set_hexpand(true);
    area.set_valign(gtk::Align::Center);
    let a = area.downgrade();
    theme::subscribe(&area, move || {
        if let Some(a) = a.upgrade() {
            a.queue_draw();
        }
    });
    area.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        let p = theme::palette();
        let text = Rgb::hex(&p.text);
        cr.set_source_rgba(text.0, text.1, text.2, 0.1);
        rounded(cr, 0.0, 0.0, w, h, h / 2.0);
        let _ = cr.fill();
        let wc = theme::weather_colors_for(&p);
        let cold = theme::swatch("blue").unwrap_or(wc.cold);
        let warm = theme::swatch("amber").unwrap_or(wc.sun);
        let (x0, x1) = (start * w, ((start + len) * w).min(w));
        let grad = cairo::LinearGradient::new(x0, 0.0, x1, 0.0);
        grad.add_color_stop_rgb(0.0, cold.0, cold.1, cold.2);
        grad.add_color_stop_rgb(1.0, warm.0, warm.1, warm.2);
        rounded(cr, x0, 0.0, (x1 - x0).max(h), h, h / 2.0);
        let _ = cr.set_source(&grad);
        let _ = cr.fill();
    });
    area
}

/// "Showers on Saturday and rain on Thursday. Friday morning is the coldest, at
/// 38°." for the next seven days.
fn week_summary(snap: &ForecastSnapshot, units: &UnitPrefs) -> Option<String> {
    let week: Vec<_> = snap.daily.iter().take(7).collect();
    if week.len() < 3 {
        return None;
    }
    let day_name = |i: usize, date: &str| match i {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        _ => NaiveDate::parse_from_str(date, "%Y-%m-%d").map(|d| d.format("%A").to_string()).unwrap_or_default(),
    };
    let wet: Vec<String> = week
        .iter()
        .enumerate()
        .filter(|(_, d)| d.precipitation_probability_max_pct >= 50 && d.condition.is_wet())
        .take(3)
        .enumerate()
        .map(|(k, (i, d))| {
            let what = d.condition.label().to_lowercase();
            let what = if k == 0 { capitalise(&what) } else { what };
            format!("{what} {}", on_day(&day_name(i, &d.date)))
        })
        .collect();
    let mut text = match wet.as_slice() {
        [] => "No rain expected this week.".to_string(),
        [one] => format!("{one}."),
        [first @ .., last] => format!("{} and {last}.", first.join(", ")),
    };
    if let Some((i, d)) = week.iter().enumerate().skip(1).min_by(|a, b| a.1.temp_min_c.total_cmp(&b.1.temp_min_c)) {
        text.push_str(&format!(
            " {} morning is the coldest, at {}.",
            capitalise(&day_name(i, &d.date)),
            units.temperature.format(d.temp_min_c)
        ));
    }
    Some(text)
}

fn on_day(day: &str) -> String {
    match day {
        "today" | "tomorrow" => day.to_string(),
        _ => format!("on {day}"),
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{AlertPeriod, capitalise, direction_words, on_day, period_when};

    #[test]
    fn says_when_each_period_is() {
        let p = |onset: &str, ends: &str| AlertPeriod { onset: Some(onset.into()), ends: Some(ends.into()) };
        let friday_noon = chrono::DateTime::parse_from_rfc3339("2026-11-13T12:00:00-06:00").unwrap().timestamp();
        let tonight = p("2026-11-14T00:00:00-06:00", "2026-11-14T08:00:00-06:00");
        assert_eq!(period_when(&tonight, friday_noon, false).as_deref(), Some("Sat 12:00 AM to 8:00 AM"));
        assert_eq!(period_when(&tonight, friday_noon + 14 * 3600, true).as_deref(), Some("Until Sat 08:00"));
        let overnight = p("2026-11-13T22:00:00-06:00", "2026-11-14T08:00:00-06:00");
        assert_eq!(period_when(&overnight, friday_noon, false).as_deref(), Some("Fri 10:00 PM to Sat 8:00 AM"));
    }

    #[test]
    fn words_for_the_week() {
        assert_eq!(direction_words("NW"), "north-west");
        assert_eq!(direction_words("S"), "south");
        assert_eq!(capitalise("tomorrow"), "Tomorrow");
        assert_eq!(on_day("Saturday"), "on Saturday");
        assert_eq!(on_day("tomorrow"), "tomorrow");
    }
}
