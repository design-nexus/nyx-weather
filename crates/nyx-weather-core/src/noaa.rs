//! NOAA / National Weather Service (NWS) active severe-weather alert adapter for US locations.

use crate::alerts::merge_alerts;
use crate::model::{AlertPeriod, AlertSeverity, WeatherAlert};
use crate::provider::{AlertProvider, ProviderError};
use reqwest::{
    blocking::Client,
    header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT},
    redirect,
};
use serde_json::Value;
use std::{io::Read, time::Duration};

const MAX_BODY: u64 = 2 * 1024 * 1024;

pub struct NoaaAlertProvider {
    base_url: String,
}

impl Default for NoaaAlertProvider {
    fn default() -> Self {
        Self { base_url: "https://api.weather.gov".into() }
    }
}

impl NoaaAlertProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self { base_url: base_url.into() }
    }

    fn http() -> Result<Client, ProviderError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/geo+json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("NyxWeather/0.1 (https://github.com/design-nexus/nyx)"),
        );
        Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(10))
            .redirect(redirect::Policy::none())
            .build()
            .map_err(|_| ProviderError::Offline)
    }

    pub fn parse_alerts_json(value: &Value) -> Result<Vec<WeatherAlert>, ProviderError> {
        let features = value["features"].as_array().ok_or(ProviderError::InvalidResponse)?;
        let mut alerts = Vec::new();
        for feature in features {
            let props = &feature["properties"];
            let Some(event) = props["event"].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            let id = props["id"]
                .as_str()
                .or_else(|| feature["id"].as_str())
                .unwrap_or(event)
                .to_string();
            let headline = props["headline"].as_str().unwrap_or(event).to_string();
            let description = props["description"].as_str().unwrap_or("").trim().to_string();
            let instruction = props["instruction"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let severity = match props["severity"].as_str().unwrap_or("Minor") {
                "Extreme" => AlertSeverity::Extreme,
                "Severe" => AlertSeverity::Severe,
                "Moderate" => AlertSeverity::Moderate,
                _ => AlertSeverity::Minor,
            };
            let area = props["areaDesc"].as_str().unwrap_or("").to_string();
            let text = |key: &str| props[key].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            let effective = text("effective");
            let expires = text("expires");
            let sender = text("senderName");
            let references = props["references"]
                .as_array()
                .map(|refs| refs.iter().filter_map(|r| r["identifier"].as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let vtec: Vec<(String, String)> = props["parameters"]["VTEC"]
                .as_array()
                .map(|v| v.iter().filter_map(|s| s.as_str().and_then(parse_vtec)).collect())
                .unwrap_or_default();
            if props["status"].as_str().is_some_and(|s| s != "Actual")
                || props["messageType"].as_str() == Some("Cancel")
                || (!vtec.is_empty() && vtec.iter().all(|(action, _)| action == "CAN"))
            {
                continue;
            }
            let period = AlertPeriod {
                onset: text("onset").or_else(|| effective.clone()),
                ends: text("ends").or_else(|| expires.clone()),
            };
            alerts.push(WeatherAlert {
                merged_ids: vec![id.clone()],
                id,
                event: event.to_string(),
                headline,
                description,
                instruction,
                severity,
                area,
                effective,
                expires,
                sender,
                sent: text("sent"),
                references,
                vtec: vtec.into_iter().map(|(_, key)| key).collect(),
                periods: vec![period],
            });
        }
        Ok(merge_alerts(alerts))
    }
}

/// A VTEC string's action and event key: "/O.CON.KBMX.FZ.W.0003.261115T0600Z-…/" is
/// ("CON", "KBMX.FZ.W.0003"), the same key across every update of that warning.
fn parse_vtec(s: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = s.trim_matches('/').split('.').collect();
    let [_, action, office, phenomenon, significance, number, ..] = parts.as_slice() else {
        return None;
    };
    Some((action.to_string(), format!("{office}.{phenomenon}.{significance}.{number}")))
}

impl AlertProvider for NoaaAlertProvider {
    fn id(&self) -> &'static str {
        "noaa-nws"
    }

    fn fetch_alerts(&self, latitude: f64, longitude: f64) -> Result<Vec<WeatherAlert>, ProviderError> {
        if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
            return Err(ProviderError::InvalidLocation);
        }
        let url = format!(
            "{}/alerts/active?point={:.4},{:.4}",
            self.base_url.trim_end_matches('/'),
            latitude,
            longitude
        );
        let resp = Self::http()?.get(&url).send().map_err(|_| ProviderError::Offline)?;
        match resp.status().as_u16() {
            200..=299 => {}
            429 => return Err(ProviderError::RateLimited { retry_after_secs: None }),
            _ => return Err(ProviderError::Failed("Could not fetch NOAA weather alerts".into())),
        }
        let mut bytes = Vec::new();
        resp.take(MAX_BODY + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ProviderError::Offline)?;
        if bytes.len() as u64 > MAX_BODY {
            return Err(ProviderError::InvalidResponse);
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidResponse)?;
        Self::parse_alerts_json(&v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_noaa_geojson_alerts() {
        let sample = serde_json::json!({
            "type": "FeatureCollection",
            "features": [
                {
                    "id": "urn:oid:2.49.0.1.840.0.1",
                    "properties": {
                        "id": "NWS-1",
                        "event": "Severe Thunderstorm Warning",
                        "headline": "Severe Thunderstorm Warning issued October 18 at 8:00PM CDT",
                        "description": "60 mph wind gusts and quarter size hail.",
                        "instruction": "Move to an interior room on the lowest floor.",
                        "severity": "Severe",
                        "areaDesc": "Travis, TX",
                        "effective": "2026-10-18T20:00:00-05:00",
                        "expires": "2026-10-18T21:00:00-05:00",
                        "senderName": "NWS Austin/San Antonio TX"
                    }
                }
            ]
        });
        let alerts = NoaaAlertProvider::parse_alerts_json(&sample).unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].event, "Severe Thunderstorm Warning");
        assert_eq!(alerts[0].severity, AlertSeverity::Severe);
        assert_eq!(alerts[0].area, "Travis, TX");
        assert_eq!(alerts[0].periods[0].ends.as_deref(), Some("2026-10-18T21:00:00-05:00"));
    }

    #[test]
    fn skips_cancellations_and_tests() {
        let feature = |id: &str, message: &str, status: &str, vtec: &str| {
            serde_json::json!({ "properties": {
                "id": id, "event": "Tornado Warning", "severity": "Extreme", "messageType": message,
                "status": status, "parameters": { "VTEC": [vtec] }
            }})
        };
        let sample = serde_json::json!({ "features": [
            feature("cancel", "Cancel", "Actual", "/O.CAN.KBMX.TO.W.0042.000000T0000Z-261113T2000Z/"),
            feature("can-update", "Update", "Actual", "/O.CAN.KBMX.TO.W.0043.000000T0000Z-261113T2000Z/"),
            feature("test", "Alert", "Test", "/T.NEW.KBMX.TO.W.0044.261113T1900Z-261113T2000Z/"),
            feature("live", "Alert", "Actual", "/O.NEW.KBMX.TO.W.0045.261113T1900Z-261113T2000Z/"),
        ]});
        let alerts = NoaaAlertProvider::parse_alerts_json(&sample).unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].id, "live");
        assert_eq!(alerts[0].vtec, ["KBMX.TO.W.0045"]);
    }
}
