//! 10-Day Outlook section: extended 10-day temperature & precipitation graph
//! and expandable daily cards with full solar, wind, and precipitation metrics.

use crate::charts::{self, ChartSeries};
use crate::prefs;
use crate::sections::{format_day_full, format_day_name, format_duration_secs, format_sun_time};
use crate::widgets::{self, Page};
use gtk::prelude::*;
use nyx_weather_core::{ForecastSnapshot, cardinal_direction, uv_category};

pub fn build(page: &Page, snap: Option<&ForecastSnapshot>, _stale_fallback: bool, last_error: Option<&str>) {
    let Some(snap) = snap else {
        super::no_forecast(
            page,
            "10-day outlook",
            "x-office-calendar-symbolic",
            "Loading 10-day outlook…",
            "Extended daily forecasts will appear once the forecast loads.",
            last_error,
        );
        return;
    };

    let p = prefs::get();
    let units = p.units();
    if snap.daily.is_empty() {
        return;
    }

    // 1. 10-Day High / Low & Precipitation Trend Graph
    let g_chart = page.group("10-day temperature & rain trend");
    let x_labels: Vec<String> = snap.daily.iter().enumerate().map(|(i, d)| format_day_name(&d.date, i)).collect();
    let max_vals: Vec<f64> = snap.daily.iter().map(|d| units.temperature.convert(d.temp_max_c)).collect();
    let min_vals: Vec<f64> = snap.daily.iter().map(|d| units.temperature.convert(d.temp_min_c)).collect();
    let pop_bars: Vec<f64> = snap.daily.iter().map(|d| d.precipitation_probability_max_pct as f64).collect();

    let chart = charts::line_chart(
        vec![
            ChartSeries {
                label: format!("Daily High ({})", units.temperature.symbol()),
                values: max_vals,
                dashed: false,
                tone: 0,
            },
            ChartSeries { label: format!("Daily Low ({})", units.temperature.symbol()), values: min_vals, dashed: true, tone: 2 },
        ],
        x_labels,
        "°".into(),
        None,
        Some(pop_bars),
        190,
    );
    g_chart.add(&chart);

    // 2. Expandable 10-Day Cards
    let g_days = page.group("Daily breakdown (10 days)");
    g_days.note("Click any day to inspect apparent high/low, precipitation hours, wind gusts, UV max and daylight duration.");

    let all_min = snap.daily.iter().map(|d| d.temp_min_c).fold(f64::INFINITY, f64::min);
    let all_max = snap.daily.iter().map(|d| d.temp_max_c).fold(f64::NEG_INFINITY, f64::max);
    let all_span = (all_max - all_min).max(1.0);

    for (idx, d) in snap.daily.iter().enumerate() {
        let outer = widgets::vbox(0);
        outer.add_css_class("detail-expand-row");

        let header = widgets::hbox(10);

        let day_lbl = widgets::label(&format_day_full(&d.date, idx), "detail-title");
        // Shrinks (ellipsized) in a floating 460 px window.
        day_lbl.set_width_chars(10);
        day_lbl.set_max_width_chars(19);
        day_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        header.append(&day_lbl);

        let icon = gtk::Image::from_icon_name(d.condition.icon_name(true));
        icon.set_pixel_size(18);
        icon.add_css_class(crate::theme::condition_color_class(d.condition, true));
        header.append(&icon);

        let cond_lbl = widgets::label(d.condition.label(), "dim");
        cond_lbl.set_hexpand(true);
        cond_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        header.append(&cond_lbl);

        let pop_txt = if d.precipitation_probability_max_pct >= 10 {
            format!("{}%", d.precipitation_probability_max_pct)
        } else {
            String::new()
        };
        let pop_lbl = widgets::label(&pop_txt, "daily-pop");
        pop_lbl.set_width_chars(4);
        pop_lbl.set_xalign(1.0);
        header.append(&pop_lbl);

        let min_lbl = widgets::label(&units.temperature.format(d.temp_min_c), "daily-min");
        min_lbl.set_width_chars(4);
        min_lbl.set_xalign(1.0);
        header.append(&min_lbl);

        let bar_track = super::range_bar(d.temp_min_c, d.temp_max_c, all_min, all_span, 96);
        header.append(&bar_track);

        let max_lbl = widgets::label(&units.temperature.format(d.temp_max_c), "daily-max");
        max_lbl.set_width_chars(4);
        max_lbl.set_xalign(1.0);
        header.append(&max_lbl);

        let chevron = gtk::Image::from_icon_name(if idx == 0 { "pan-up-symbolic" } else { "pan-down-symbolic" });
        chevron.add_css_class("dim");
        header.append(&chevron);
        let toggle = gtk::Button::builder().child(&header).css_classes(["flat", "detail-expand-header"]).build();
        outer.append(&toggle);

        let body = widgets::vbox(6);
        body.add_css_class("detail-expand-body");
        body.set_visible(idx == 0);

        let app_range = match (d.apparent_max_c, d.apparent_min_c) {
            (Some(hi), Some(lo)) => {
                format!("{} / {}", units.temperature.format_with_unit(hi), units.temperature.format_with_unit(lo))
            }
            _ => "–".into(),
        };
        let prec_detail = format!(
            "{} ({}% chance{})",
            units.precipitation.format(d.precipitation_sum_mm),
            d.precipitation_probability_max_pct,
            d.precipitation_hours.map(|h| format!(", {h:.0}h duration")).unwrap_or_default()
        );
        let wdir = d.wind_direction_dominant_deg.map(cardinal_direction).unwrap_or("–");
        let wind_detail = format!(
            "{} {wdir}{}",
            units.wind_speed.format(d.wind_speed_max_kmh),
            d.wind_gusts_max_kmh.map(|g| format!(" (gusts {})", units.wind_speed.format(g))).unwrap_or_default()
        );
        let uv_detail = d.uv_index_max.map(|u| format!("{u:.1} ({})", uv_category(u))).unwrap_or_else(|| "–".into());
        let sr = format_sun_time(d.sunrise.as_deref(), units.use_24h_time);
        let ss = format_sun_time(d.sunset.as_deref(), units.use_24h_time);
        let solar_energy = d.shortwave_radiation_sum_mj.map(|mj| format!("{mj:.1} MJ/m²")).unwrap_or_else(|| "–".into());

        for (k, v) in [
            ("Feels-like high / low", app_range),
            ("Precipitation total", prec_detail),
            ("Maximum wind & gusts", wind_detail),
            ("Maximum UV index", uv_detail),
            ("Sunrise & sunset", format!("↑ {sr}  ·  ↓ {ss}")),
            (
                "Daylight / sunshine",
                format!(
                    "{} daylight · {} sunshine",
                    format_duration_secs(d.daylight_duration_secs),
                    format_duration_secs(d.sunshine_duration_secs)
                ),
            ),
            ("Solar radiation sum", solar_energy),
        ] {
            let r = widgets::hbox(8);
            let kl = widgets::label(k, "dim");
            kl.set_hexpand(true);
            let vl = widgets::label(&v, "tnum");
            r.append(&kl);
            r.append(&vl);
            body.append(&r);
        }
        outer.append(&body);

        let b2 = body.clone();
        toggle.connect_clicked(move |_| {
            let next = !b2.is_visible();
            b2.set_visible(next);
            chevron.set_icon_name(Some(if next { "pan-up-symbolic" } else { "pan-down-symbolic" }));
        });

        g_days.add(&outer);
    }
}
