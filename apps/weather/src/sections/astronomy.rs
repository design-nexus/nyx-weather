//! Sun, Moon & Models section: solar elevation arc, lunar phase disc & cycle,
//! and multi-model ensemble comparison (Open-Meteo Best Match, ECMWF IFS, NOAA GFS, DWD ICON).

use crate::charts::{self, ChartSeries};
use crate::sections::{format_day_name, format_duration_secs, format_sun_time};
use crate::widgets::{self, Page};
use crate::{backend, prefs};
use gtk::prelude::*;
use nyx_weather_core::{ForecastSnapshot, calculate_moon_phase};

pub fn build(page: &Page, snap: Option<&ForecastSnapshot>, _stale_fallback: bool, last_error: Option<&str>) {
    let Some(snap) = snap else {
        super::no_forecast(
            page,
            "Sun, moon & models",
            "weather-clear-night-symbolic",
            "Loading astronomy & model ensemble…",
            "Solar trajectory, lunar phase and multi-model comparisons will appear once loaded.",
            last_error,
        );
        return;
    };

    let p = prefs::get();
    let units = p.units();
    let today = snap.daily.first();

    // 1. Solar Elevation & Daylight Trajectory
    let g_sun = page.group("Sun & daylight trajectory");
    let arc = charts::solar_arc_canvas(
        today.and_then(|d| d.sunrise.clone()),
        today.and_then(|d| d.sunset.clone()),
        snap.current.time.clone(),
        snap.current.is_day,
    );
    g_sun.add(&arc);

    let sr = format_sun_time(today.and_then(|d| d.sunrise.as_deref()), units.use_24h_time);
    let ss = format_sun_time(today.and_then(|d| d.sunset.as_deref()), units.use_24h_time);
    g_sun.add(&widgets::row(
        "Sunrise & sunset",
        &format!("Local timezone: {}", snap.location.timezone),
        Some(widgets::label(&format!("↑ {sr}   ·   ↓ {ss}"), "tnum").upcast_ref()),
    ));
    g_sun.add(&widgets::row(
        "Daylight & sunshine duration",
        "Total time between sunrise and sunset vs. clear-sky direct sunshine.",
        Some(
            widgets::label(
                &format!(
                    "{} daylight  ·  {} sunshine",
                    format_duration_secs(today.and_then(|d| d.daylight_duration_secs)),
                    format_duration_secs(today.and_then(|d| d.sunshine_duration_secs))
                ),
                "tnum",
            )
            .upcast_ref(),
        ),
    ));

    if snap.daily.len() >= 2
        && let (Some(d0), Some(d1)) = (snap.daily[0].daylight_duration_secs, snap.daily[1].daylight_duration_secs)
    {
        let diff_mins = (d1 - d0) / 60.0;
        let sign = if diff_mins >= 0.0 { "+" } else { "−" };
        g_sun.add(&widgets::row(
            "Daily daylight change",
            "Difference in daylight length tomorrow compared to today.",
            Some(widgets::label(&format!("{sign}{:.1} min / day", diff_mins.abs()), "tnum").upcast_ref()),
        ));
    }

    // 2. Lunar Phase & Synodic Cycle
    let moon = calculate_moon_phase(backend::now_epoch());
    let g_moon = page.group("Moon phase & lunar cycle");
    let moon_card = widgets::hbox(18);
    moon_card.add_css_class("moon-hero-row");
    moon_card.append(&charts::moon_phase_canvas(moon));

    let moon_meta = widgets::vbox(6);
    moon_meta.set_valign(gtk::Align::Center);
    moon_meta.set_hexpand(true);
    moon_meta.append(&widgets::label(&format!("{}  {}", moon.glyph, moon.phase_name), "hero-condition"));
    moon_meta.append(&widgets::label(
        &format!("{}% illuminated  ·  Day {:.1} of 29.5-day synodic cycle", moon.illumination_pct, moon.age_days),
        "dim",
    ));
    moon_meta.append(&widgets::label(
        &format!("Next Full Moon in {:.1} days  ·  Next New Moon in {:.1} days", moon.days_to_full, moon.days_to_new),
        "tnum",
    ));
    moon_card.append(&moon_meta);
    g_moon.add(&moon_card);

    // 3. Multi-Model Forecast Comparison (Open-Meteo, ECMWF IFS, NOAA GFS, DWD ICON)
    if snap.models.len() > 1 {
        let g_models = page.group("Multi-model forecast comparison");
        g_models.note(
            "Daily highs from each weather model Open-Meteo serves for this place, beside its blended forecast. Gaps are days past a model's range.",
        );

        let x_labels: Vec<String> = snap.daily.iter().enumerate().map(|(i, d)| format_day_name(&d.date, i)).collect();

        let series: Vec<ChartSeries> = snap
            .models
            .iter()
            .enumerate()
            .map(|(idx, m)| ChartSeries {
                label: m.model_name.clone(),
                values: m.daily_max_c.iter().map(|&c| units.temperature.convert(c)).collect(),
                dashed: idx % 2 == 1,
                tone: idx,
            })
            .collect();

        let model_chart = charts::line_chart(series, x_labels, "°".into(), None, None, 195);
        g_models.add(&model_chart);

        for m in &snap.models {
            let first = |v: &[f64]| v.first().copied().filter(|x| x.is_finite());
            let hi0 = first(&m.daily_max_c).map(|v| units.temperature.format(v)).unwrap_or_else(|| "–".into());
            let lo0 = first(&m.daily_min_c).map(|v| units.temperature.format(v)).unwrap_or_else(|| "–".into());
            let p0 = first(&m.daily_precip_mm).map(|v| units.precipitation.format(v)).unwrap_or_else(|| "–".into());
            let summary = format!("Today H: {hi0}  L: {lo0}  ·  Precip {p0}");
            g_models.add(&widgets::row(
                &m.model_name,
                &format!("Spatial resolution: {}", m.resolution),
                Some(widgets::label(&summary, "tnum").upcast_ref()),
            ));
        }
    }
}
