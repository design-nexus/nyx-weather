//! Waybar custom module output (`nyx-weather --waybar`), a summary for widgets and the bar
//! (`nyx-weather --json`), and headless cache refresh.
//!
//! Reuses the SQLite forecast cache directly without starting GTK, refreshing
//! only when the cached entry is missing or older than `refresh_interval_mins`.

use crate::{backend, prefs};
use nyx_weather_core::{ForecastSnapshot, cardinal_direction};
use serde_json::json;

/// The active location's forecast from the cache, fetched first when it's stale.
fn active_snapshot(p: &prefs::Prefs) -> Option<(ForecastSnapshot, bool)> {
    let loc = p.active_location()?;
    let max_age = (p.refresh_interval_mins.max(5) as i64) * 60;
    let lock = std::fs::File::create(crate::paths::cache_db().with_extension("waybar.lock")).ok();
    let _held = lock.as_ref().map(|f| f.lock());
    let outcome = backend::load_or_fetch(&loc, &p.open_meteo_config(), max_age, false);
    outcome.snapshot.map(|s| (s, outcome.stale_fallback))
}

/// Emit one line of JSON for widgets: now, the next twelve hours and five days, every value
/// already formatted in the user's units. Flat objects, so a shell script can read it.
pub fn print_summary_json() {
    let p = prefs::read_disk();
    match active_snapshot(&p) {
        Some((mut snap, stale)) => {
            snap.advance_to(backend::now_epoch());
            println!("{}", format_summary(&snap, &p, stale));
        }
        None => println!("{}", json!({ "error": "No location or cached forecast yet" })),
    }
}

pub fn format_summary(snap: &ForecastSnapshot, p: &prefs::Prefs, stale_fallback: bool) -> serde_json::Value {
    let units = p.units();
    let cur = &snap.current;
    let (hi, lo) = snap.today_high_low_c();
    let hours: Vec<serde_json::Value> = snap
        .upcoming_hours(13)
        .into_iter()
        .skip(1)
        .map(|h| {
            json!({
                "time": crate::sections::format_clock_label(&h.time, units.use_24h_time),
                "temperature": units.temperature.format(h.temperature_c),
                "icon": h.condition.icon_name(h.is_day),
                "precipitation": h.precipitation_probability_pct,
            })
        })
        .collect();
    // Today and the next four days; `low_c`/`high_c` put each day's range on one scale.
    let days: Vec<serde_json::Value> = snap
        .daily
        .iter()
        .take(5)
        .enumerate()
        .map(|(i, d)| {
            let name = match i {
                0 => "Today".to_string(),
                _ => chrono::NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").map(|n| n.format("%a").to_string()).unwrap_or_default(),
            };
            json!({
                "day": name,
                "icon": d.condition.icon_name(true),
                "condition": d.condition.label(),
                "high": units.temperature.format(d.temp_max_c),
                "low": units.temperature.format(d.temp_min_c),
                "high_c": d.temp_max_c,
                "low_c": d.temp_min_c,
                "precipitation": d.precipitation_probability_max_pct,
            })
        })
        .collect();
    let today = snap.daily.first();
    let clock = |t: &Option<String>| t.as_deref().map(|t| crate::sections::format_clock_label(t, units.use_24h_time));
    json!({
        "location": snap.location.name,
        "place": snap.location.display_name(),
        "condition": cur.condition.label(),
        "icon": cur.condition.icon_name(cur.is_day),
        "temperature": units.temperature.format(cur.temperature_c),
        "feels_like": units.temperature.format(cur.apparent_temperature_c),
        "high": units.temperature.format(hi),
        "low": units.temperature.format(lo),
        "wind": format!("{} {}", units.wind_speed.format(cur.wind_speed_kmh), cardinal_direction(cur.wind_direction_deg)),
        "humidity": cur.humidity_pct,
        "rain": snap.rain_nowcast(),
        "alert": snap.alerts.first().map(|a| format!("{}: {}", a.severity.label(), a.event)),
        "updated": backend::human_age(snap.fetched_at),
        "offline": stale_fallback,
        "hours": hours,
        "days": days,
        "uv": cur.uv_index.map(|u| u.round() as i64),
        "sunrise": today.and_then(|d| clock(&d.sunrise)),
        "sunset": today.and_then(|d| clock(&d.sunset)),
    })
}

/// Emit a single line of Waybar JSON for the active saved location.
pub fn print_waybar_json(refresh_if_stale: bool) {
    let p = prefs::read_disk();
    let Some(loc) = p.active_location() else {
        let out = json!({
            "text": "☁ --°",
            "alt": "unknown",
            "tooltip": "Weather — No location configured\nClick to open Weather",
            "class": "unknown"
        });
        println!("{out}");
        return;
    };

    let max_age = (p.refresh_interval_mins.max(5) as i64) * 60;
    let outcome = if refresh_if_stale {
        // Several bars (one per monitor) poll at once; the first fetches while the
        // others wait, then find the cache fresh.
        let lock = std::fs::File::create(crate::paths::cache_db().with_extension("waybar.lock")).ok();
        let _held = lock.as_ref().map(|f| f.lock());
        backend::load_or_fetch(&loc, &p.open_meteo_config(), max_age, false)
    } else {
        let snap = backend::get_cached(&loc.id);
        backend::FetchOutcome {
            snapshot: snap,
            stale_fallback: false,
            error: None,
        }
    };

    match outcome.snapshot {
        Some(snap) => {
            let payload = format_waybar_payload(&snap, &p, outcome.stale_fallback);
            println!("{payload}");
        }
        None => {
            let err_msg = outcome
                .error
                .map(|e| e.to_string())
                .unwrap_or_else(|| "No cached forecast yet".into());
            let out = json!({
                "text": format!("☁ {}", loc.name),
                "alt": "offline",
                "tooltip": format!("{} — {err_msg}\nClick to open Weather", esc(&loc.display_name())),
                "class": "offline"
            });
            println!("{out}");
        }
    }
}

/// Waybar reads `text` and `tooltip` as Pango markup.
fn esc(s: &str) -> String {
    gtk::glib::markup_escape_text(s).to_string()
}

pub fn format_waybar_payload(
    snap: &ForecastSnapshot,
    p: &prefs::Prefs,
    stale_fallback: bool,
) -> serde_json::Value {
    let units = p.units();
    let cur = &snap.current;
    let glyph = cur.condition.waybar_glyph(cur.is_day);
    let temp = units.temperature.format(cur.temperature_c);
    let text = if !snap.alerts.is_empty() {
        format!("⚠ {glyph} {temp}")
    } else {
        format!("{glyph} {temp}")
    };

    let (hi, lo) = snap.today_high_low_c();
    let wind_dir = cardinal_direction(cur.wind_direction_deg);
    let age = backend::human_age(snap.fetched_at);
    let status = if stale_fallback {
        format!("Offline cache ({age})")
    } else {
        format!("Updated {age}")
    };

    let mut tooltip = format!(
        "{} — {}\nTemperature: {} (Feels like {})\nHigh: {} · Low: {}\nWind: {} {wind_dir} · Humidity: {}%",
        esc(&snap.location.display_name()),
        cur.condition.label(),
        units.temperature.format_with_unit(cur.temperature_c),
        units.temperature.format(cur.apparent_temperature_c),
        units.temperature.format(hi),
        units.temperature.format(lo),
        units.wind_speed.format(cur.wind_speed_kmh),
        cur.humidity_pct
    );

    if let Some(rain) = snap.rain_nowcast() {
        tooltip.push_str(&format!("\n☂ {rain}"));
    }
    if let Some(alert) = snap.alerts.first() {
        tooltip.push_str(&format!("\n⚠ {}: {}", alert.severity.label(), esc(&alert.event)));
    } else if snap.alerts_unavailable {
        tooltip.push_str("\nAlerts unavailable");
    }
    tooltip.push_str(&format!("\n{status} · {}", snap.attribution.provider_name));

    let class = if !snap.alerts.is_empty() {
        "alert"
    } else {
        cur.condition.css_class(cur.is_day)
    };

    json!({
        "text": text,
        "alt": cur.condition.css_class(cur.is_day),
        "tooltip": tooltip,
        "class": class
    })
}

/// Refresh all saved locations in the SQLite cache without starting GTK.
pub fn refresh_all_cli() -> i32 {
    let p = prefs::read_disk();
    let cfg = p.open_meteo_config();
    let mut failed = false;
    for loc in &p.saved_locations {
        let outcome = backend::load_or_fetch(loc, &cfg, 0, true);
        if let Some(err) = outcome.error {
            eprintln!("nyx-weather: {}: {err}", loc.name);
            failed = true;
        }
    }
    if failed { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyx_weather_core::{
        CurrentWeather, DailyForecast, Location, ProviderAttribution, WeatherCondition,
    };

    #[test]
    fn formats_waybar_json_with_units_and_tooltip() {
        let loc = Location::new(
            "Austin",
            Some("Texas".into()),
            Some("United States".into()),
            Some("US".into()),
            30.2672,
            -97.7431,
            None,
            None,
        );
        let snap = ForecastSnapshot {
            location: loc,
            fetched_at: backend::now_epoch(),
            utc_offset_seconds: -18000,
            current: CurrentWeather {
                time: "2026-10-18T14:00".into(),
                temperature_c: 20.0,
                apparent_temperature_c: 21.0,
                condition: WeatherCondition::Clear,
                is_day: true,
                humidity_pct: 48,
                dew_point_c: Some(9.0),
                wind_speed_kmh: 16.0934,
                wind_gusts_kmh: None,
                wind_direction_deg: 315,
                pressure_hpa: Some(1015.0),
                precipitation_mm: 0.0,
                cloud_cover_pct: 5,
                visibility_m: Some(16000.0),
                uv_index: Some(5.0),
            },
            hourly: vec![],
            daily: vec![DailyForecast {
                date: "2026-10-18".into(),
                condition: WeatherCondition::Clear,
                temp_max_c: 25.0,
                temp_min_c: 15.0,
                apparent_max_c: None,
                apparent_min_c: None,
                sunrise: None,
                sunset: None,
                precipitation_sum_mm: 0.0,
                precipitation_probability_max_pct: 0,
                precipitation_hours: None,
                uv_index_max: Some(6.0),
                wind_speed_max_kmh: 20.0,
                wind_gusts_max_kmh: None,
                wind_direction_dominant_deg: Some(315),
                daylight_duration_secs: None,
                sunshine_duration_secs: None,
                shortwave_radiation_sum_mj: None,
            }],
            air_quality: None,
            alerts: vec![],
            alerts_unavailable: false,
            models: vec![],
            minutely_precip: vec![],
            attribution: ProviderAttribution {
                provider_id: "open-meteo".into(),
                provider_name: "Open-Meteo".into(),
                license_name: "CC BY 4.0".into(),
                attribution_text: "Weather data by Open-Meteo.com".into(),
                attribution_url: "https://open-meteo.com/".into(),
                commercial_use_allowed: false,
            },
        };
        let p = prefs::Prefs {
            temperature_unit: nyx_weather_core::TemperatureUnit::Fahrenheit,
            wind_speed_unit: nyx_weather_core::WindSpeedUnit::Mph,
            ..Default::default()
        };
        let out = format_waybar_payload(&snap, &p, false);
        assert_eq!(out["text"], "☀ 68°");
        assert_eq!(out["class"], "clear-day");
        let tip = out["tooltip"].as_str().unwrap();
        assert!(tip.contains("Austin · Texas, United States — Clear sky"));
        assert!(tip.contains("High: 77° · Low: 59°"));
        assert!(tip.contains("Wind: 10 mph NW"));

        let mut snap = snap;
        snap.hourly = (14..18)
            .map(|h| nyx_weather_core::HourlyForecast {
                time: format!("2026-10-18T{h}:00"),
                temperature_c: 20.0 + (h - 14) as f64,
                apparent_temperature_c: 20.0,
                precipitation_probability_pct: 10,
                precipitation_mm: 0.0,
                condition: WeatherCondition::Clear,
                is_day: true,
                wind_speed_kmh: 0.0,
                wind_gusts_kmh: None,
                wind_direction_deg: None,
                uv_index: None,
                humidity_pct: None,
                dew_point_c: None,
                pressure_hpa: None,
                cloud_cover_pct: None,
                visibility_m: None,
                shortwave_radiation_wm2: None,
            })
            .collect();
        let sum = format_summary(&snap, &p, false);
        assert_eq!(sum["temperature"], "68°");
        assert_eq!(sum["high"], "77°");
        assert_eq!(sum["icon"], "weather-clear-symbolic");
        let hours = sum["hours"].as_array().unwrap();
        assert_eq!(hours.len(), 3, "the hours after this one");
        assert_eq!(hours[0]["time"], "3 PM");
        assert_eq!(hours[0]["temperature"], "70°");
        let days = sum["days"].as_array().unwrap();
        assert_eq!((days[0]["day"].as_str(), days[0]["high"].as_str()), (Some("Today"), Some("77°")));
        assert_eq!(sum["uv"], 5);
    }
}
