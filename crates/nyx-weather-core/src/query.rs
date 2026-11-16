//! Natural-language weather query parser and evaluator for Glimpse, CLI, and D-Bus.
//!
//! Handles queries such as:
//! - `weather`
//! - `weather tomorrow`
//! - `rain Friday`
//! - `weather London`
//! - `rain tomorrow in Seattle`

use crate::model::{ForecastSnapshot, UnitPrefs};
use chrono::{Datelike, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetDay {
    Today,
    Tomorrow,
    Weekday(u32), // 0 = Mon .. 6 = Sun
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryIntent {
    pub rain_focus: bool,
    pub day: TargetDay,
    pub location_query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryResponse {
    pub title: String,
    pub subtitle: String,
    pub details: String,
    pub location_name: String,
}

/// Parse a user query into a structured [`QueryIntent`].
pub fn parse_query(raw: &str) -> Option<QueryIntent> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let first_lower = words[0].to_lowercase();
    let is_weather_prefix =
        matches!(first_lower.as_str(), "weather" | "forecast" | "rain" | "snow" | "temp" | "temperature" | "umbrella");
    if !is_weather_prefix {
        return None;
    }

    let rain_focus = matches!(first_lower.as_str(), "rain" | "snow" | "umbrella")
        || words.iter().any(|w| matches!(w.to_lowercase().as_str(), "rain" | "raining" | "precipitation" | "umbrella"));

    let mut day = TargetDay::Today;
    let mut loc_tokens = Vec::new();

    for &w in words.iter().skip(1) {
        let lower = w.to_lowercase();
        let clean = lower.trim_matches(|c: char| !c.is_alphanumeric());
        match clean {
            "today" | "now" | "tonight" => day = TargetDay::Today,
            "tomorrow" | "tmrw" => day = TargetDay::Tomorrow,
            "monday" | "mon" => day = TargetDay::Weekday(0),
            "tuesday" | "tue" | "tues" => day = TargetDay::Weekday(1),
            "wednesday" | "wed" => day = TargetDay::Weekday(2),
            "thursday" | "thu" | "thur" | "thurs" => day = TargetDay::Weekday(3),
            "friday" | "fri" => day = TargetDay::Weekday(4),
            "saturday" | "sat" => day = TargetDay::Weekday(5),
            "sunday" | "sun" => day = TargetDay::Weekday(6),
            "in" | "for" | "at" | "on" | "weather" | "forecast" | "rain" => {}
            _ => loc_tokens.push(w),
        }
    }

    let location_query = if loc_tokens.is_empty() { None } else { Some(loc_tokens.join(" ")) };

    Some(QueryIntent { rain_focus, day, location_query })
}

/// Evaluate a [`QueryIntent`] against a [`ForecastSnapshot`] and user unit preferences.
pub fn evaluate_query(intent: &QueryIntent, snapshot: &ForecastSnapshot, units: UnitPrefs) -> QueryResponse {
    let day_index = match intent.day {
        TargetDay::Today => Some(0),
        TargetDay::Tomorrow => Some(1),
        TargetDay::Weekday(target_num) => snapshot.daily.iter().position(|d| {
            NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")
                .ok()
                .is_some_and(|dt| dt.weekday().num_days_from_monday() == target_num)
        }),
    }
    .filter(|&i| i < snapshot.daily.len());

    let loc = &snapshot.location.name;
    let Some(day_index) = day_index else {
        let day_label = match intent.day {
            TargetDay::Weekday(w) => weekday_name(w),
            _ => "Tomorrow",
        };
        let title = format!("{loc}: {day_label} isn't in the forecast yet");
        return QueryResponse {
            details: title.clone(),
            title,
            subtitle: format!("The forecast covers {} days", snapshot.daily.len()),
            location_name: loc.clone(),
        };
    };
    let nowcast = (day_index == 0).then(|| snapshot.rain_nowcast()).flatten();
    if day_index == 0 && !intent.rain_focus && matches!(intent.day, TargetDay::Today) {
        let (hi, lo) = snapshot.today_high_low_c();
        let title = format!(
            "{}: {} · {}",
            loc,
            units.temperature.format_with_unit(snapshot.current.temperature_c),
            snapshot.current.condition.label()
        );
        let subtitle = format!(
            "{}Feels like {} · High {} / Low {} · Wind {}",
            nowcast.as_ref().map(|n| format!("{n} · ")).unwrap_or_default(),
            units.temperature.format(snapshot.current.apparent_temperature_c),
            units.temperature.format(hi),
            units.temperature.format(lo),
            units.wind_speed.format(snapshot.current.wind_speed_kmh)
        );
        let details = format!(
            "{}\n{}\nHumidity: {}% · Precipitation: {}",
            title,
            subtitle,
            snapshot.current.humidity_pct,
            units.precipitation.format(snapshot.current.precipitation_mm)
        );
        return QueryResponse { title, subtitle, details, location_name: loc.clone() };
    }

    if let Some(day) = snapshot.daily.get(day_index) {
        let day_label = match intent.day {
            TargetDay::Today => "Today".to_string(),
            TargetDay::Tomorrow => "Tomorrow".to_string(),
            TargetDay::Weekday(w) => weekday_name(w).to_string(),
        };
        let title = if intent.rain_focus {
            format!(
                "{} ({day_label}): {}% chance of precipitation ({})",
                loc,
                day.precipitation_probability_max_pct,
                units.precipitation.format(day.precipitation_sum_mm)
            )
        } else {
            format!(
                "{} ({day_label}): {} · {} / {}",
                loc,
                day.condition.label(),
                units.temperature.format_with_unit(day.temp_max_c),
                units.temperature.format(day.temp_min_c)
            )
        };
        let subtitle = format!(
            "{}{} · Precip {}% ({}) · Max wind {}",
            nowcast.map(|n| format!("{n} · ")).unwrap_or_default(),
            day.condition.label(),
            day.precipitation_probability_max_pct,
            units.precipitation.format(day.precipitation_sum_mm),
            units.wind_speed.format(day.wind_speed_max_kmh)
        );
        let details = format!("{title}\n{subtitle}");
        QueryResponse { title, subtitle, details, location_name: loc.clone() }
    } else {
        QueryResponse {
            title: format!("{loc}: Forecast unavailable"),
            subtitle: String::new(),
            details: String::new(),
            location_name: loc.clone(),
        }
    }
}

fn weekday_name(num_from_mon: u32) -> &'static str {
    match Weekday::try_from(num_from_mon as u8).unwrap_or(Weekday::Mon) {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_when_a_day_is_past_the_forecast() {
        let v: serde_json::Value = serde_json::json!({
            "current": {"time": "2026-10-18T14:00", "temperature_2m": 20.0},
            "hourly": {"time": ["2026-10-18T14:00"], "temperature_2m": [20.0]},
            "daily": {"time": ["2026-10-18"], "temperature_2m_max": [22.0], "temperature_2m_min": [12.0]}
        });
        let loc = crate::Location::new("Austin", None, None, None, 30.0, -97.0, None, None);
        let snap = crate::OpenMeteoProvider::parse_forecast_json(
            &loc,
            &v,
            None,
            0,
            crate::WeatherProvider::attribution(&crate::OpenMeteoProvider::default()),
        )
        .unwrap();
        let ask = |q: &str| evaluate_query(&parse_query(q).unwrap(), &snap, UnitPrefs::default()).title;
        assert_eq!(ask("weather tomorrow"), "Austin: Tomorrow isn't in the forecast yet");
        // 2026-10-18 is a Sunday; Monday isn't in a one-day forecast.
        assert_eq!(ask("rain monday"), "Austin: Monday isn't in the forecast yet");
        assert!(ask("rain sunday").starts_with("Austin (Sunday)"));
    }

    #[test]
    fn parses_glimpse_queries() {
        assert_eq!(parse_query("weather"), Some(QueryIntent { rain_focus: false, day: TargetDay::Today, location_query: None }));
        assert_eq!(
            parse_query("weather tomorrow"),
            Some(QueryIntent { rain_focus: false, day: TargetDay::Tomorrow, location_query: None })
        );
        assert_eq!(
            parse_query("rain Friday"),
            Some(QueryIntent { rain_focus: true, day: TargetDay::Weekday(4), location_query: None })
        );
        assert_eq!(
            parse_query("weather London"),
            Some(QueryIntent { rain_focus: false, day: TargetDay::Today, location_query: Some("London".into()) })
        );
    }
}
