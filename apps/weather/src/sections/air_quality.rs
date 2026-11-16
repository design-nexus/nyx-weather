//! Air Quality, Pollen & UV section: US EPA AQI, European EAQI, 24-hour AQI/PM2.5 curve,
//! particulate/gas pollutant breakdown, 6 pollen species, and UV irradiance.

use crate::charts::{self, ChartSeries};
use crate::widgets::{self, Page};
use gtk::prelude::*;
use nyx_weather_core::{ForecastSnapshot, pollen_category, us_aqi_category, uv_category};

pub fn build(page: &Page, snap: Option<&ForecastSnapshot>, _stale_fallback: bool, last_error: Option<&str>) {
    let Some(snap) = snap else {
        super::no_forecast(
            page,
            "Air quality & pollen",
            "weather-fog-symbolic",
            "Loading air quality & pollen…",
            "Atmospheric composition, particulates and pollen counts will appear once loaded.",
            last_error,
        );
        return;
    };

    let Some(aq) = snap.air_quality.clone() else {
        let g = page.group("Air quality & pollen");
        g.add(&widgets::empty_state(
            "weather-fog-symbolic",
            "Air quality unavailable",
            "The air-quality service didn't answer. It's asked again on the next refresh.",
            None,
        ));
        return;
    };

    // 1. Air Quality Summary Hero & Health Guidance
    let g_summary = page.group("Air quality index (AQI)");
    let advice = match aq.us_aqi {
        None => "No current reading.",
        Some(0..=50) => "Air quality is satisfactory, and air pollution poses little or no risk.",
        Some(51..=100) => {
            "Air quality is acceptable. Unusually sensitive individuals should consider limiting prolonged outdoor exertion."
        }
        Some(101..=150) => {
            "Members of sensitive groups may experience health effects. The general public is less likely to be affected."
        }
        Some(151..=200) => {
            "Everyone may begin to experience health effects; sensitive groups may experience more serious effects."
        }
        Some(_) => "Health alert: outdoor exertion should be avoided.",
    };
    g_summary.note(advice);

    let aqi_right = widgets::hbox(8);
    if let Some(us_aqi) = aq.us_aqi {
        let aqi_tag_class = match us_aqi {
            0..=50 => "good",
            51..=100 => "moderate",
            101..=150 => "elevated",
            _ => "danger",
        };
        aqi_right.append(&widgets::tag(us_aqi_category(us_aqi), aqi_tag_class));
    }
    aqi_right.append(&widgets::label(&aq.us_aqi.map(|v| format!("{v} AQI")).unwrap_or_else(|| "Unavailable".into()), "tnum"));

    g_summary.add(&widgets::row(
        "US EPA Air Quality Index",
        "United States Environmental Protection Agency 0–500 scale.",
        Some(aqi_right.upcast_ref()),
    ));
    g_summary.add(&widgets::row(
        "European Air Quality Index (EAQI)",
        "European Environment Agency 0–100+ scale.",
        Some(
            widgets::label(&aq.european_aqi.map(|v| format!("{v} EAQI")).unwrap_or_else(|| "Unavailable".into()), "tnum")
                .upcast_ref(),
        ),
    ));

    // 2. 24-Hour US AQI & PM2.5 Cairo Trend Graph
    if aq.hourly_us_aqi.iter().flatten().count() >= 4 {
        let g_trend = page.group("24-hour air quality trend");
        let gap = |v: Option<f64>| v.unwrap_or(f64::NAN);
        let aqi_f: Vec<f64> = aq.hourly_us_aqi.iter().map(|v| gap(v.map(f64::from))).collect();
        let pm25_f: Vec<f64> = aq.hourly_pm2_5.iter().map(|&v| gap(v)).collect();
        let start = aq.hourly_start.as_deref().and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").ok());
        let use_24h = crate::prefs::get().use_24h_time;
        let labels: Vec<String> = (0..aqi_f.len())
            .map(|i| match start {
                Some(t) => {
                    let t = t + chrono::Duration::hours(i as i64);
                    if use_24h { t.format("%H:%M").to_string() } else { t.format("%-I %p").to_string() }
                }
                None => format!("+{i}h"),
            })
            .collect();
        let chart = charts::line_chart(
            vec![
                ChartSeries { label: "US AQI".into(), values: aqi_f, dashed: false, tone: 3 },
                ChartSeries { label: "PM2.5 (μg/m³)".into(), values: pm25_f, dashed: true, tone: 4 },
            ],
            labels,
            "".into(),
            None,
            None,
            175,
        );
        g_trend.add(&chart);
    }

    // 3. Particulate Matter & Trace Gases Breakdown
    let g_pollutants = page.group("Particulates & trace gases");
    g_pollutants.note("Concentrations at surface level in micrograms per cubic meter (μg/m³).");

    for (name, desc, val_opt, unit) in [
        ("PM2.5 (Fine particulate matter)", "Inhalable particles ≤ 2.5 μm (WHO 24h guideline: 15 μg/m³).", aq.pm2_5, "μg/m³"),
        ("PM10 (Coarse particulate matter)", "Inhalable particles ≤ 10 μm (WHO 24h guideline: 45 μg/m³).", aq.pm10, "μg/m³"),
        ("Ozone (O₃)", "Ground-level tropospheric ozone.", aq.ozone, "μg/m³"),
        ("Nitrogen dioxide (NO₂)", "Combustion and traffic trace gas.", aq.nitrogen_dioxide, "μg/m³"),
        ("Sulphur dioxide (SO₂)", "Industrial and volcanic trace gas.", aq.sulphur_dioxide, "μg/m³"),
        ("Carbon monoxide (CO)", "Incomplete combustion gas concentration.", aq.carbon_monoxide, "μg/m³"),
        ("Desert & mineral dust", "Airborne mineral dust concentration.", aq.dust, "μg/m³"),
        (
            "Aerosol optical depth (AOD)",
            "Columnar atmospheric haze and aerosol extinction at 550 nm.",
            aq.aerosol_optical_depth,
            "",
        ),
    ] {
        let txt = val_opt
            .map(|v| if unit.is_empty() { format!("{v:.2}") } else { format!("{v:.1} {unit}") })
            .unwrap_or_else(|| "–".into());
        g_pollutants.add(&widgets::row(name, desc, Some(widgets::label(&txt, "tnum").upcast_ref())));
    }

    // 4. Pollen & Allergens
    let g_pollen = page.group("Pollen & allergens");
    g_pollen.note("Airborne pollen in grains/m³. Open-Meteo forecasts pollen for Europe only, during each species' season.");
    for (species, kind_desc, val_opt) in [
        ("Grass pollen", "Poaceae meadow and turf grasses", aq.grass_pollen),
        ("Birch pollen", "Betula tree pollen", aq.birch_pollen),
        ("Alder pollen", "Alnus tree pollen", aq.alder_pollen),
        ("Olive pollen", "Olea tree pollen", aq.olive_pollen),
        ("Ragweed pollen", "Ambrosia weed pollen", aq.ragweed_pollen),
        ("Mugwort pollen", "Artemisia weed pollen", aq.mugwort_pollen),
    ] {
        let right = widgets::hbox(8);
        if let Some(v) = val_opt {
            let p_class = if v < 10.0 {
                "good"
            } else if v < 50.0 {
                "moderate"
            } else {
                "elevated"
            };
            right.append(&widgets::tag(pollen_category(v), p_class));
            right.append(&widgets::label(&format!("{v:.0} grains/m³"), "tnum"));
        } else {
            right.append(&widgets::label("Not forecast here", "dim"));
        }
        g_pollen.add(&widgets::row(species, kind_desc, Some(right.upcast_ref())));
    }

    // 5. UV Index & Clear-Sky Irradiance
    let g_uv = page.group("UV radiation & solar exposure");
    let max_uv = snap.daily.first().and_then(|d| d.uv_index_max);

    let uv_tag_class = |u: f64| {
        if u < 3.0 {
            "good"
        } else if u < 6.0 {
            "moderate"
        } else if u < 8.0 {
            "elevated"
        } else {
            "uv"
        }
    };

    let uv_value = |uv: Option<f64>| -> gtk::Widget {
        let right = widgets::hbox(8);
        match uv {
            Some(u) => {
                right.append(&widgets::tag(uv_category(u), uv_tag_class(u)));
                right.append(&widgets::label(&format!("{u:.1}"), "tnum"));
            }
            None => right.append(&widgets::label("Unavailable", "tnum")),
        }
        right.upcast()
    };
    g_uv.add(&widgets::row("Current UV index", "Live surface ultraviolet index.", Some(&uv_value(snap.current.uv_index))));
    g_uv.add(&widgets::row(
        "Clear-sky UV potential",
        "Unattenuated UV index without cloud cover.",
        Some(&uv_value(aq.uv_index_clear_sky)),
    ));
    g_uv.add(&widgets::row("Today's peak UV index", "Forecasted solar noon maximum.", Some(&uv_value(max_uv))));
}
