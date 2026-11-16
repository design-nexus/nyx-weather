//! Hourly & Graphs section: 48-hour Cairo charts for temperature, precipitation,
//! wind/gusts, humidity/pressure, and a full 48-hour meteorological table.

use crate::charts::{self, ChartSeries};
use crate::prefs;
use crate::sections::format_clock_label;
use crate::widgets::{self, Page};
use gtk::prelude::*;
use nyx_weather_core::{ForecastSnapshot, cardinal_direction};

pub fn build(page: &Page, snap: Option<&ForecastSnapshot>, _stale_fallback: bool, last_error: Option<&str>) {
    let Some(snap) = snap else {
        super::no_forecast(
            page,
            "Hourly & graphs",
            "view-continuous-symbolic",
            "Loading hourly telemetry…",
            "48-hour forecast curves will appear as soon as forecast data arrives.",
            last_error,
        );
        return;
    };

    let p = prefs::get();
    let units = p.units();
    let hours = snap.upcoming_hours(48);
    if hours.is_empty() {
        return;
    }

    let x_labels: Vec<String> = hours
        .iter()
        .enumerate()
        .map(|(i, h)| if i == 0 { "Now".into() } else { format_clock_label(&h.time, units.use_24h_time) })
        .collect();

    // 1. Temperature, Feels-Like & Precipitation Probability Chart
    let g_temp = page.group("Temperature & precipitation (48 hours)");
    g_temp.note("Solid line: 2m air temperature · Dashed line: apparent (feels-like) temperature · Shaded bars: precipitation probability (%).");
    let temp_vals: Vec<f64> = hours.iter().map(|h| units.temperature.convert(h.temperature_c)).collect();
    let app_vals: Vec<f64> = hours.iter().map(|h| units.temperature.convert(h.apparent_temperature_c)).collect();
    let pop_bars: Vec<f64> = hours.iter().map(|h| h.precipitation_probability_pct as f64).collect();

    let temp_chart = charts::line_chart(
        vec![
            ChartSeries {
                label: format!("Temperature ({})", units.temperature.symbol()),
                values: temp_vals,
                dashed: false,
                tone: 0,
            },
            ChartSeries { label: "Feels like".into(), values: app_vals, dashed: true, tone: 1 },
        ],
        x_labels.clone(),
        "°".into(),
        None,
        Some(pop_bars),
        200,
    );
    g_temp.add(&temp_chart);

    // 2. Wind Speed & Wind Gusts Chart
    let g_wind = page.group("Wind speed & gusts (48 hours)");
    let wind_vals: Vec<f64> = hours.iter().map(|h| units.wind_speed.convert(h.wind_speed_kmh)).collect();
    let gust_vals: Vec<f64> =
        hours.iter().map(|h| units.wind_speed.convert(h.wind_gusts_kmh.unwrap_or(h.wind_speed_kmh))).collect();
    let wind_chart = charts::line_chart(
        vec![
            ChartSeries {
                label: format!("Sustained ({})", units.wind_speed.symbol()),
                values: wind_vals,
                dashed: false,
                tone: 3,
            },
            ChartSeries { label: "Gusts".into(), values: gust_vals, dashed: true, tone: 5 },
        ],
        x_labels.clone(),
        format!(" {}", units.wind_speed.symbol()),
        None,
        None,
        180,
    );
    g_wind.add(&wind_chart);

    // 3. Humidity & Cloud Cover Chart
    let g_atm = page.group("Humidity & cloud cover (48 hours)");
    let hum_vals: Vec<f64> = hours.iter().map(|h| h.humidity_pct.unwrap_or(0) as f64).collect();
    let cloud_vals: Vec<f64> = hours.iter().map(|h| h.cloud_cover_pct.unwrap_or(0) as f64).collect();
    let atm_chart = charts::line_chart(
        vec![
            ChartSeries { label: "Humidity (%)".into(), values: hum_vals, dashed: false, tone: 1 },
            ChartSeries { label: "Cloud cover (%)".into(), values: cloud_vals, dashed: true, tone: 4 },
        ],
        x_labels.clone(),
        "%".into(),
        Some((0.0, 100.0)),
        None,
        170,
    );
    g_atm.add(&atm_chart);

    // 4. Surface Pressure Chart
    let pres_vals: Vec<f64> =
        hours.iter().map(|h| h.pressure_hpa.map(|p| units.pressure.convert(p)).unwrap_or(f64::NAN)).collect();
    if pres_vals.iter().any(|v| v.is_finite()) {
        let g_pres = page.group("Surface pressure (48 hours)");
        g_pres.add(&charts::line_chart(
            vec![ChartSeries {
                label: format!("Pressure ({})", units.pressure.symbol()),
                values: pres_vals,
                dashed: false,
                tone: 2,
            }],
            x_labels,
            String::new(),
            None,
            None,
            160,
        ));
    }

    // 5. Detailed 48-hour table
    let g_table = page.group("Hour-by-hour breakdown");
    g_table
        .note("Hourly for 12 hours, then every 2 hours: conditions, temperature (feels like), precipitation, wind and pressure.");
    // A floating 460 px window drops the condition name and pressure columns.
    let narrow = crate::window::is_narrow();
    for (idx, h) in hours.iter().enumerate() {
        if idx > 12 && idx % 2 != 0 {
            continue;
        }
        let row = widgets::hbox(10);
        row.add_css_class("daily-row");

        let time_lbl =
            widgets::label(&if idx == 0 { "Now".into() } else { format_clock_label(&h.time, units.use_24h_time) }, "daily-day");
        time_lbl.set_width_chars(8);
        row.append(&time_lbl);

        let icon = gtk::Image::from_icon_name(h.condition.icon_name(h.is_day));
        icon.set_pixel_size(18);
        icon.add_css_class(crate::theme::condition_color_class(h.condition, h.is_day));
        row.append(&icon);

        if !narrow {
            let cond = widgets::label(h.condition.label(), "");
            cond.set_width_chars(14);
            cond.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&cond);
        }

        let temp = widgets::label(
            &format!("{} ({})", units.temperature.format(h.temperature_c), units.temperature.format(h.apparent_temperature_c)),
            "tnum",
        );
        temp.set_width_chars(10);
        row.append(&temp);

        let pop = widgets::label(
            &if h.precipitation_probability_pct > 0 {
                format!("{}% · {}", h.precipitation_probability_pct, units.precipitation.format(h.precipitation_mm))
            } else {
                "0%".into()
            },
            "daily-pop",
        );
        pop.set_width_chars(if narrow { 9 } else { 12 });
        row.append(&pop);

        let wdir = h.wind_direction_deg.map(cardinal_direction).unwrap_or("–");
        let wind = widgets::label(&format!("{} {wdir}", units.wind_speed.format(h.wind_speed_kmh)), "dim");
        wind.set_hexpand(true);
        wind.set_xalign(1.0);
        row.append(&wind);

        if !narrow {
            let pres_txt = h.pressure_hpa.map(|p| units.pressure.format(p)).unwrap_or_else(|| "–".into());
            let pres = widgets::label(&pres_txt, "dim");
            pres.set_width_chars(10);
            pres.set_xalign(1.0);
            row.append(&pres);
        }

        g_table.add(&row);
    }
}
