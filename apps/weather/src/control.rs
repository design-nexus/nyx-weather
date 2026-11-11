//! D-Bus control interface (`io.github.design_nexus.nyx.Control`) for Nyx Assistant,
//! Nyx Glimpse, and Nyx Calendar integration.

use crate::{backend, cmd, prefs, window};
use nyx_core::control::{self, Reply};
use nyx_weather_core::{evaluate_query, parse_query};
use serde_json::{Value, json};

pub fn tools() -> Value {
    let location = json!({"type": "string", "description": "A city or place; the active saved location when left out"});
    let only_location = json!({"type": "object", "properties": {"location": location}, "additionalProperties": false});
    let none = json!({"type": "object", "properties": {}, "additionalProperties": false});
    json!([
        {"name": "weather.current", "description": "The weather now and today's high and low for the active or named location.",
         "inputSchema": only_location, "annotations": {"title": "Weather now", "readOnlyHint": true},
         "x-nyx": {"icon": "weather-few-clouds-symbolic", "suggest": {"location": "weather.saved"}, "risk": "read_only", "aliases": ["now"], "positional": ["location"], "result": "object"}},
        {"name": "weather.forecast", "description": "The next 24 hours and 7 days for the active or named location (Calendar reuses it).",
         "inputSchema": only_location, "annotations": {"title": "Forecast", "readOnlyHint": true},
         "x-nyx": {"icon": "weather-overcast-symbolic", "suggest": {"location": "weather.saved"}, "risk": "read_only", "aliases": ["week"], "positional": ["location"], "result": "object"}},
        {"name": "weather.query", "description": "Answer a weather question such as 'tomorrow', 'rain Friday' or 'London'.",
         "inputSchema": {"type": "object", "properties": {"query": {"type": "string", "minLength": 1}}, "required": ["query"], "additionalProperties": false},
         "annotations": {"title": "Ask about the weather", "readOnlyHint": true},
         "x-nyx": {"risk": "read_only", "aliases": ["ask"], "positional": ["query"], "result": "object"}},
        {"name": "weather.locations", "description": "The saved locations and which one is active.",
         "inputSchema": none, "annotations": {"title": "Saved locations", "readOnlyHint": true},
         "x-nyx": {"risk": "read_only", "aliases": ["places"], "result": "object"}},
        {"name": "weather.select_location", "description": "Make a saved location active, by name or ID.",
         "inputSchema": {"type": "object", "properties": {"location": {"type": "string", "minLength": 1}}, "required": ["location"], "additionalProperties": false},
         "annotations": {"title": "Switch location"},
         "x-nyx": {"icon": "mark-location-symbolic", "suggest": {"location": "weather.saved"}, "risk": "writes", "confirmation": "never", "aliases": ["use"], "positional": ["location"], "result": "object"}},
        {"name": "weather.saved", "description": "Saved locations whose name starts with a prefix, for completing a location.",
         "inputSchema": {"type": "object", "properties": {"prefix": {"type": "string"}, "limit": {"type": "integer", "minimum": 1, "maximum": 50}}, "additionalProperties": false},
         "annotations": {"title": "Saved locations by name", "readOnlyHint": true},
         "x-nyx": {"icon": "find-location-symbolic", "risk": "read_only", "result": "object"}},
        {"name": "weather.refresh", "description": "Fetch the active location's forecast again.",
         "inputSchema": none, "annotations": {"title": "Refresh"},
         "x-nyx": {"risk": "writes", "confirmation": "never", "result": "object"}},
        {"name": "weather.open", "description": "Open Weather's window, at a saved location when given.",
         "inputSchema": only_location, "annotations": {"title": "Open Weather"},
         "x-nyx": {"suggest": {"location": "weather.saved"}, "risk": "writes", "confirmation": "never", "positional": ["location"], "result": "none"}},
    ])
}

/// API 2: the tools and the app. Weather answers without its window (D-Bus starts
/// it as a service), and Glimpse's Weather panel watches `now`.
pub fn description(app_id: &str) -> Value {
    json!({
        "tools": tools(),
        "nyx": {"api": 2, "app": app_id, "name": "Weather", "icon": crate::APP_ID, "namespaces": ["weather"], "aliases": ["forecast"],
                "bus_name": app_id, "activation": "dbus", "default": "weather.query", "open": {"action": "weather.open"},
                "topics": [{"name": "now", "sensitive": true}],
                "panel": {"id": "weather", "topics": ["now"], "size": {"min": [420, 300], "preferred": [500, 380]}}}
    })
}

thread_local! {
    static SERVICE: std::cell::RefCell<Option<control::Service>> = const { std::cell::RefCell::new(None) };
    static TIMER: std::cell::RefCell<Option<gtk::glib::SourceId>> = const { std::cell::RefCell::new(None) };
}

/// Register the interface (from `connect_startup`); `now` is read again every
/// five minutes (from the forecast cache when it's fresh) only while watched.
pub fn serve(app: &gtk::Application) {
    use gtk::prelude::*;
    let id = app.application_id().map(|s| s.to_string()).unwrap_or_else(|| crate::APP_ID.to_string());
    let window_app = app.clone();
    let Some(service) = control::serve_with(app, description(&id), move |request, answer| {
        if request.action == "weather.open" {
            window::present(&window_app, None, request.arguments["location"].as_str());
            return answer(Ok(Value::Null));
        }
        let changes = matches!(request.action.as_str(), "weather.select_location" | "weather.refresh");
        handle(&request.action, request.arguments, Box::new(move |r| {
            answer(r.map_err(|e| nyx_actions::Error::new(nyx_actions::ErrorKind::Failed, e)));
            if changes {
                publish();
            }
        }));
    }) else {
        return;
    };
    service.on_watch(|_, on| {
        TIMER.with(|t| {
            if let Some(id) = t.borrow_mut().take() {
                id.remove();
            }
            if on {
                publish();
                *t.borrow_mut() = Some(gtk::glib::timeout_add_seconds_local(300, || {
                    publish();
                    gtk::glib::ControlFlow::Continue
                }));
            }
        });
    });
    SERVICE.with(|s| *s.borrow_mut() = Some(service));
}

fn publish() {
    let Some(service) = SERVICE.with(|s| s.borrow().clone()) else { return };
    if !service.watched("now") {
        return;
    }
    handle("weather.current", json!({}), Box::new(move |r| {
        service.publish("now", r.unwrap_or_else(|e| json!({"error": e})));
    }));
}

/// Tools by name; the names from before `weather.` (`current`, `forecast`, …) still work.
pub fn handle(name: &str, args: Value, reply: Reply) {
    let name = name.strip_prefix("weather.").unwrap_or(name);
    match name {
        "saved" => {
            let prefix = args["prefix"].as_str().unwrap_or("").to_lowercase();
            let limit = args["limit"].as_u64().unwrap_or(20).min(50) as usize;
            let found: Vec<Value> = prefs::get()
                .saved_locations
                .iter()
                .filter(|l| l.name.to_lowercase().starts_with(&prefix))
                .take(limit)
                .map(|l| json!({"value": l.name, "label": l.name, "detail": l.display_name()}))
                .collect();
            reply(Ok(Value::Array(found)));
        }
        "locations" => {
            let p = prefs::get();
            reply(Ok(json!({
                "active_location_id": p.active_location_id,
                "saved_locations": p.saved_locations,
            })));
        }
        "select_location" => {
            let Ok(target) = control::text(&args, "location") else {
                reply(Err("location is required".into()));
                return;
            };
            let q = target.trim().to_lowercase();
            let p = prefs::get();
            if let Some(found) = p
                .saved_locations
                .iter()
                .find(|l| l.id == target || l.name.to_lowercase().contains(&q))
                .cloned()
            {
                window::select_location(&found.id);
                reply(Ok(json!({ "selected": found })));
            } else {
                reply(Err(format!("No saved location matches '{target}'")));
            }
        }
        "refresh" => {
            window::refresh_active(true);
            reply(Ok(json!({ "refreshing": true })));
        }
        "current" | "forecast" => {
            let loc_arg = args["location"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
            let p = prefs::get();
            let cfg = p.open_meteo_config();
            let max_age = (p.refresh_interval_mins.max(5) as i64) * 60;
            let want_full = name == "forecast";
            let units = p.units();

            cmd::background(
                move || -> Result<Value, String> {
                    let target_loc = if let Some(q) = loc_arg {
                        let q_low = q.to_lowercase();
                        if let Some(saved) = p.saved_locations.iter().find(|l| l.name.to_lowercase().contains(&q_low)) {
                            saved.clone()
                        } else {
                            let found = backend::search_locations(&q, &cfg).map_err(|e| e.to_string())?;
                            found.into_iter().next().ok_or_else(|| format!("Location '{q}' not found"))?
                        }
                    } else {
                        p.active_location().ok_or_else(|| "No active location configured".to_string())?
                    };

                    let outcome = backend::load_or_fetch(&target_loc, &cfg, max_age, false);
                    let Some(snap) = outcome.snapshot else {
                        return Err(outcome.error.map(|e| e.to_string()).unwrap_or_else(|| "Forecast unavailable".into()));
                    };

                    if want_full {
                        Ok(json!({
                            "location": snap.location,
                            "fetched_at": snap.fetched_at,
                            "stale_fallback": outcome.stale_fallback,
                            "current": snap.current,
                            "hourly": snap.upcoming_hours(24),
                            "daily": snap.daily,
                            "air_quality": snap.air_quality,
                            "alerts": snap.alerts,
                            "attribution": snap.attribution,
                        }))
                    } else {
                        let (hi, lo) = snap.today_high_low_c();
                        Ok(json!({
                            "location": snap.location.display_name(),
                            "condition": snap.current.condition.label(),
                            "temperature": units.temperature.format_with_unit(snap.current.temperature_c),
                            "feels_like": units.temperature.format_with_unit(snap.current.apparent_temperature_c),
                            "high": units.temperature.format_with_unit(hi),
                            "low": units.temperature.format_with_unit(lo),
                            "wind": units.wind_speed.format(snap.current.wind_speed_kmh),
                            "humidity_pct": snap.current.humidity_pct,
                            "icon": snap.current.condition.icon_name(snap.current.is_day),
                            "days": snap.daily.iter().take(5).map(|d| json!({
                                "date": d.date,
                                "condition": d.condition.label(),
                                "icon": d.condition.icon_name(true),
                                "high": units.temperature.format_with_unit(d.temp_max_c),
                                "low": units.temperature.format_with_unit(d.temp_min_c),
                                "rain_pct": d.precipitation_probability_max_pct,
                            })).collect::<Vec<_>>(),
                            "alerts": snap.alerts,
                            "attribution": snap.attribution.attribution_text,
                        }))
                    }
                },
                reply,
            );
        }
        "query" => {
            let Ok(raw) = control::text(&args, "query") else {
                reply(Err("query is required".into()));
                return;
            };
            // A bare place name ("Seattle") is a question about its weather.
            let intent = parse_query(&raw).unwrap_or_else(|| {
                parse_query(&format!("weather {raw}")).unwrap_or(nyx_weather_core::QueryIntent {
                    rain_focus: false,
                    day: nyx_weather_core::query::TargetDay::Today,
                    location_query: None,
                })
            });
            let p = prefs::get();
            let cfg = p.open_meteo_config();
            let units = p.units();
            let max_age = (p.refresh_interval_mins.max(5) as i64) * 60;

            cmd::background(
                move || -> Result<Value, String> {
                    let target_loc = if let Some(q) = &intent.location_query {
                        let q_low = q.to_lowercase();
                        if let Some(saved) = p.saved_locations.iter().find(|l| l.name.to_lowercase().contains(&q_low)) {
                            saved.clone()
                        } else {
                            let found = backend::search_locations(q, &cfg).map_err(|e| e.to_string())?;
                            found.into_iter().next().ok_or_else(|| format!("Location '{q}' not found"))?
                        }
                    } else {
                        p.active_location().ok_or_else(|| "No active location configured".to_string())?
                    };

                    let outcome = backend::load_or_fetch(&target_loc, &cfg, max_age, false);
                    let Some(snap) = outcome.snapshot else {
                        return Err(outcome.error.map(|e| e.to_string()).unwrap_or_else(|| "Forecast unavailable".into()));
                    };

                    let answer = evaluate_query(&intent, &snap, units);
                    Ok(serde_json::to_value(answer).unwrap_or_else(|_| json!({})))
                },
                reply,
            );
        }
        _ => reply(Err(format!("unknown tool: {name}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_description_is_api_2() {
        let d = nyx_actions::Description::from_value(&description(crate::APP_ID), None).unwrap();
        assert_eq!(d.actions.len(), 8);
        assert_eq!(d.action("weather.current").unwrap().suggest.get("location").map(String::as_str), Some("weather.saved"));
        assert!(d.actions.iter().all(|a| !a.asks()));
        assert_eq!(d.app.panel.as_ref().map(|p| p.id.as_str()), Some("weather"));
    }
}
