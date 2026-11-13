//! All sidebar sections in Weather, plus shared date/time formatting helpers.

pub mod air_quality;
pub mod astronomy;
pub mod daily;
pub mod hourly;
pub mod overview;
pub mod radar;
pub mod settings;

use crate::widgets::Page;
use chrono::{Datelike, NaiveDate, NaiveDateTime};
use nyx_weather_core::ForecastSnapshot;

pub struct Section {
    pub id: &'static str,
    pub group: &'static str,
    pub title: &'static str,
    pub icon: &'static str,
    pub description: &'static str,
    /// Quiet text at the end of its sidebar row.
    pub readout: &'static str,
    pub nav: bool,
    pub build: fn(&Page, Option<&ForecastSnapshot>, bool, Option<&str>),
}

pub fn all() -> Vec<Section> {
    vec![
        Section {
            id: "overview",
            group: "Forecast",
            title: "Today",
            icon: "weather-few-clouds-symbolic",
            description: "Alerts, conditions, the next 24 hours, readings and ten days (Ctrl+1)",
            readout: "",
            nav: true,
            build: overview::build,
        },
        Section {
            id: "hourly",
            group: "Forecast",
            title: "Hourly",
            icon: "nyx-weather-graph-symbolic",
            description: "48 hours of temperature, rain, wind, humidity and pressure (Ctrl+2)",
            readout: "48 h",
            nav: true,
            build: hourly::build,
        },
        Section {
            id: "daily",
            group: "Forecast",
            title: "10 days",
            icon: "x-office-calendar-symbolic",
            description: "Ten days with wind, UV, daylight and rain totals (Ctrl+3)",
            readout: "",
            nav: true,
            build: daily::build,
        },
        Section {
            id: "radar",
            group: "Maps",
            title: "Radar",
            icon: "nyx-weather-radar-symbolic",
            description: "Radar, infrared satellite, and wind and temperature maps (Ctrl+4)",
            readout: "",
            nav: true,
            build: radar::build,
        },
        Section {
            id: "air-quality",
            group: "Outdoors",
            title: "Air and pollen",
            icon: "nyx-weather-leaf-symbolic",
            description: "US and European AQI, particles and gases, UV and pollen (Ctrl+5)",
            readout: "",
            nav: true,
            build: air_quality::build,
        },
        Section {
            id: "astronomy",
            group: "Outdoors",
            title: "Sun and moon",
            icon: "weather-clear-night-symbolic",
            description: "The sun's path, the moon's phase, and the forecast models compared (Ctrl+6)",
            readout: "",
            nav: true,
            build: astronomy::build,
        },
    ]
}

/// A sidebar group's section colour: the theme's `[sections.weather]` where it names
/// one (Maps is `maps-radar`, Outdoors `environment`), a swatch otherwise.
pub fn group_section(group: &str) -> &'static str {
    match group {
        "Maps" => "maps-radar",
        "Outdoors" => "environment",
        _ => "swatch-violet",
    }
}

/// A warning banner with one action, Retry.
pub fn retry_banner(text: &str) -> gtk::Box {
    use gtk::prelude::*;
    let banner = crate::widgets::banner(&gtk::glib::markup_escape_text(text), true);
    let retry = gtk::Button::with_label("Retry");
    retry.set_valign(gtk::Align::Center);
    retry.connect_clicked(|_| crate::window::refresh_active(true));
    banner.append(&retry);
    banner
}

/// A page with no forecast to show: still loading, or (after `last_error`) nothing
/// cached and the service unreachable.
pub fn no_forecast(page: &Page, group: &str, icon: &str, loading: &str, desc: &str, last_error: Option<&str>) {
    use gtk::prelude::*;
    let g = page.group(group);
    match last_error {
        Some(err) => {
            page.body.prepend(&retry_banner(err));
            g.add(&crate::widgets::empty_state(
                icon,
                "No forecast yet",
                "Weather couldn't reach the forecast service and has nothing cached for this place.",
                None,
            ));
        }
        None => g.add(&crate::widgets::empty_state(icon, loading, desc, None)),
    }
}

/// Where a day's low-to-high bar starts and how long it is, as fractions of the track,
/// given the period's lowest low and its span. The bar is at least 8% long and never
/// runs past the end.
pub fn range_fracs(lo: f64, hi: f64, period_lo: f64, period_span: f64) -> (f64, f64) {
    let span = period_span.max(f64::EPSILON);
    let start = ((lo - period_lo) / span).clamp(0.0, 1.0);
    let len = ((hi - lo) / span).max(0.08);
    let start = start.min(1.0 - 0.08);
    (start, len.min(1.0 - start))
}

/// A day's temperature range bar, `width` px wide, over the period's `period_lo` and span.
pub fn range_bar(lo: f64, hi: f64, period_lo: f64, period_span: f64, width: i32) -> gtk::Box {
    use gtk::prelude::*;
    let track = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    track.add_css_class("temp-range-track");
    track.set_width_request(width);
    track.set_valign(gtk::Align::Center);
    let (start, len) = range_fracs(lo, hi, period_lo, period_span);
    let w = width as f64;
    let pad = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    pad.set_size_request((start * w).round() as i32, 6);
    let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    fill.add_css_class("temp-range-fill");
    fill.set_size_request((len * w).round().max(8.0) as i32, 6);
    track.append(&pad);
    track.append(&fill);
    track
}

pub fn format_clock_label(iso: &str, use_24h: bool) -> String {
    if let Ok(dt) = NaiveDateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M") {
        if use_24h {
            dt.format("%H:%M").to_string()
        } else {
            dt.format("%-I %p").to_string()
        }
    } else if let Some(hm) = iso.split('T').nth(1) {
        hm.get(..5).unwrap_or(hm).to_string()
    } else {
        iso.to_string()
    }
}

pub fn format_sun_time(iso: Option<&str>, use_24h: bool) -> String {
    let Some(s) = iso else { return "–".into() };
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        if use_24h {
            dt.format("%H:%M").to_string()
        } else {
            dt.format("%-I:%M %p").to_string()
        }
    } else {
        s.to_string()
    }
}

pub fn format_day_name(date_str: &str, idx: usize) -> String {
    if idx == 0 {
        return "Today".into();
    }
    if let Ok(d) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        match d.weekday() {
            chrono::Weekday::Mon => "Mon".into(),
            chrono::Weekday::Tue => "Tue".into(),
            chrono::Weekday::Wed => "Wed".into(),
            chrono::Weekday::Thu => "Thu".into(),
            chrono::Weekday::Fri => "Fri".into(),
            chrono::Weekday::Sat => "Sat".into(),
            chrono::Weekday::Sun => "Sun".into(),
        }
    } else {
        date_str.to_string()
    }
}

pub fn format_day_full(date_str: &str, idx: usize) -> String {
    if let Ok(d) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        let prefix = if idx == 0 {
            "Today · ".to_string()
        } else if idx == 1 {
            "Tomorrow · ".to_string()
        } else {
            String::new()
        };
        format!("{prefix}{}", d.format("%a, %b %-d"))
    } else {
        date_str.to_string()
    }
}

pub fn format_duration_secs(secs: Option<f64>) -> String {
    let Some(s) = secs else { return "–".into() };
    let total_mins = (s / 60.0).round() as i64;
    let h = total_mins / 60;
    let m = total_mins % 60;
    format!("{h}h {m:02}m")
}

#[cfg(test)]
mod tests {
    use super::range_fracs;

    #[test]
    fn range_bars_stay_on_the_track() {
        // A day whose range sits at the top of the week's (it used to panic).
        let (start, len) = range_fracs(19.8, 20.5, 10.0, 10.5);
        assert!(start + len <= 1.0 + 1e-9 && len >= 0.079);
        assert_eq!(range_fracs(10.0, 20.0, 10.0, 10.0), (0.0, 1.0));
        let (start, len) = range_fracs(15.0, 15.0, 10.0, 0.0);
        assert!(start + len <= 1.0 + 1e-9);
    }
}
