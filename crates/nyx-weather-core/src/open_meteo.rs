//! Open-Meteo forecast, air-quality, and geocoding adapter.
//!
//! Includes configurable endpoints, commercial licensing checks, bounded reads,
//! rate limiting, and transient retry handling.

use crate::model::{
    AirQuality, CurrentWeather, DailyForecast, ForecastSnapshot, HourlyForecast, Location,
    ProviderAttribution, WeatherCondition,
};
use crate::field::{Bounds, FieldGrid, FieldPoint};
use crate::noaa::NoaaAlertProvider;
use crate::provider::{AlertProvider, ProviderError, WeatherProvider};
use reqwest::{
    blocking::{Client, Response},
    header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT},
    redirect,
};
use serde_json::Value;
use std::{
    io::Read,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;

const DEFAULT_FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const DEFAULT_AIR_QUALITY_URL: &str = "https://air-quality-api.open-meteo.com/v1/air-quality";
const DEFAULT_GEOCODING_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const MAX_BODY: u64 = 4 * 1024 * 1024;
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct OpenMeteoConfig {
    pub forecast_url: String,
    pub air_quality_url: String,
    pub geocoding_url: String,
    pub api_key: Option<String>,
    pub commercial_use: bool,
    pub fetch_us_noaa_alerts: bool,
}

impl Default for OpenMeteoConfig {
    fn default() -> Self {
        Self {
            forecast_url: DEFAULT_FORECAST_URL.into(),
            air_quality_url: DEFAULT_AIR_QUALITY_URL.into(),
            geocoding_url: DEFAULT_GEOCODING_URL.into(),
            api_key: None,
            commercial_use: false,
            fetch_us_noaa_alerts: true,
        }
    }
}

pub struct OpenMeteoProvider {
    config: OpenMeteoConfig,
    last_request: Mutex<Option<Instant>>,
}

impl Default for OpenMeteoProvider {
    fn default() -> Self {
        Self::new(OpenMeteoConfig::default())
    }
}

impl OpenMeteoProvider {
    pub fn new(config: OpenMeteoConfig) -> Self {
        Self {
            config,
            last_request: Mutex::new(None),
        }
    }

    /// Verify that commercial usage is not attempted against the free hosted Open-Meteo
    /// endpoint without an API key or custom self-hosted URL.
    pub fn check_licensing(&self) -> Result<(), ProviderError> {
        if !self.config.commercial_use {
            return Ok(());
        }
        let has_key = self
            .config
            .api_key
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty());
        let custom_endpoint = self.config.forecast_url != DEFAULT_FORECAST_URL
            && !self.config.forecast_url.contains("//api.open-meteo.com");
        if has_key || custom_endpoint {
            Ok(())
        } else {
            Err(ProviderError::CommercialKeyRequired)
        }
    }

    fn throttle(&self) {
        if let Ok(mut guard) = self.last_request.lock() {
            if let Some(prev) = *guard {
                let elapsed = prev.elapsed();
                if elapsed < MIN_REQUEST_INTERVAL {
                    std::thread::sleep(MIN_REQUEST_INTERVAL - elapsed);
                }
            }
            *guard = Some(Instant::now());
        }
    }

    fn http() -> Result<Client, ProviderError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("NyxWeather/0.1 (https://github.com/design-nexus/nyx)"),
        );
        Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(redirect::Policy::none())
            .build()
            .map_err(|_| ProviderError::Offline)
    }

    fn get_json(&self, base_url: &str, params: &[(&str, String)]) -> Result<Value, ProviderError> {
        self.check_licensing()?;
        self.throttle();

        let mut url = Url::parse(self.endpoint(base_url.trim()).as_ref())
            .map_err(|_| ProviderError::Failed("Invalid Open-Meteo endpoint URL".into()))?;
        {
            let mut qp = url.query_pairs_mut();
            for (k, v) in params {
                qp.append_pair(k, v);
            }
            if let Some(key) = self.config.api_key.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                qp.append_pair("apikey", key);
            }
        }

        let client = Self::http()?;
        let mut last_err = ProviderError::Offline;

        for attempt in 0..2 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(600));
            }
            match client.get(url.as_str()).send() {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if matches!(status, 502..=504) && attempt == 0 {
                        last_err = ProviderError::Offline;
                        continue;
                    }
                    return Self::read_json_response(resp);
                }
                Err(_) => {
                    last_err = ProviderError::Offline;
                }
            }
        }
        Err(last_err)
    }

    fn read_json_response(resp: Response) -> Result<Value, ProviderError> {
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        match status {
            200..=299 => {}
            429 => return Err(ProviderError::RateLimited { retry_after_secs: retry_after }),
            400 => return Err(ProviderError::InvalidLocation),
            401 | 403 => {
                return Err(ProviderError::Failed(
                    "Open-Meteo rejected the API key or endpoint credentials".into(),
                ));
            }
            _ => {
                return Err(ProviderError::Failed(
                    "Weather service could not complete the request".into(),
                ));
            }
        }

        let mut bytes = Vec::new();
        resp.take(MAX_BODY + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ProviderError::Offline)?;
        if bytes.len() as u64 > MAX_BODY {
            return Err(ProviderError::InvalidResponse);
        }
        serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidResponse)
    }

    pub fn parse_geocoding_json(value: &Value, limit: usize) -> Result<Vec<Location>, ProviderError> {
        let Some(results) = value.get("results").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for item in results.iter().take(limit) {
            let Some(name) = item["name"].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            let Some(lat) = item["latitude"].as_f64() else {
                continue;
            };
            let Some(lon) = item["longitude"].as_f64() else {
                continue;
            };
            let admin1 = item["admin1"].as_str().map(str::to_string);
            let country = item["country"].as_str().map(str::to_string);
            let country_code = item["country_code"].as_str().map(str::to_string);
            let timezone = item["timezone"].as_str().map(str::to_string);
            let elevation_m = item["elevation"].as_f64();
            out.push(Location::new(
                name,
                admin1,
                country,
                country_code,
                lat,
                lon,
                timezone,
                elevation_m,
            ));
        }
        Ok(out)
    }

    pub fn parse_forecast_json(
        location: &Location,
        forecast_val: &Value,
        aq_val: Option<&Value>,
        fetched_at: i64,
        attribution: ProviderAttribution,
    ) -> Result<ForecastSnapshot, ProviderError> {
        let utc_offset_seconds = forecast_val["utc_offset_seconds"].as_i64().unwrap_or(0) as i32;
        let mut loc = location.clone();
        if let Some(tz) = forecast_val["timezone"].as_str().filter(|s| !s.is_empty()) {
            loc.timezone = tz.to_string();
        }
        if loc.elevation_m.is_none() {
            loc.elevation_m = forecast_val["elevation"].as_f64();
        }

        let cur = &forecast_val["current"];
        if !cur.is_object() {
            return Err(ProviderError::InvalidResponse);
        }

        let current_time = cur["time"].as_str().unwrap_or("").to_string();
        let temperature_c = cur["temperature_2m"].as_f64().ok_or(ProviderError::InvalidResponse)?;
        let apparent_temperature_c = cur["apparent_temperature"].as_f64().unwrap_or(temperature_c);
        let wmo = cur["weather_code"].as_u64().unwrap_or(0) as u16;
        let is_day = cur["is_day"].as_u64().unwrap_or(1) != 0;
        let humidity_pct = cur["relative_humidity_2m"].as_f64().unwrap_or(0.0).clamp(0.0, 100.0) as u8;
        let dew_point_c = cur["dew_point_2m"].as_f64();
        let wind_speed_kmh = cur["wind_speed_10m"].as_f64().unwrap_or(0.0);
        let wind_gusts_kmh = cur["wind_gusts_10m"].as_f64();
        let wind_direction_deg = cur["wind_direction_10m"].as_f64().unwrap_or(0.0).rem_euclid(360.0) as u16;
        let pressure_hpa = cur["surface_pressure"].as_f64().or_else(|| cur["pressure_msl"].as_f64());
        let precipitation_mm = cur["precipitation"].as_f64().unwrap_or(0.0);
        let cloud_cover_pct = cur["cloud_cover"].as_f64().unwrap_or(0.0).clamp(0.0, 100.0) as u8;
        let visibility_m = cur["visibility"].as_f64();
        let mut current_uv = cur["uv_index"].as_f64();

        let hourly_obj = &forecast_val["hourly"];
        let h_times = hourly_obj["time"].as_array().ok_or(ProviderError::InvalidResponse)?;
        let h_temps = hourly_obj["temperature_2m"].as_array().ok_or(ProviderError::InvalidResponse)?;
        let h_app = hourly_obj["apparent_temperature"].as_array();
        let h_prob = hourly_obj["precipitation_probability"].as_array();
        let h_prec = hourly_obj["precipitation"].as_array();
        let h_code = hourly_obj["weather_code"].as_array();
        let h_day = hourly_obj["is_day"].as_array();
        let h_wind = hourly_obj["wind_speed_10m"].as_array();
        let h_gusts = hourly_obj["wind_gusts_10m"].as_array();
        let h_wdir = hourly_obj["wind_direction_10m"].as_array();
        let h_uv = hourly_obj["uv_index"].as_array();
        let h_hum = hourly_obj["relative_humidity_2m"].as_array();
        let h_dew = hourly_obj["dew_point_2m"].as_array();
        let h_pres = hourly_obj["surface_pressure"].as_array();
        let h_cloud = hourly_obj["cloud_cover"].as_array();
        let h_vis = hourly_obj["visibility"].as_array();
        let h_rad = hourly_obj["shortwave_radiation"].as_array();

        let mut hourly = Vec::with_capacity(h_times.len());
        let mut matched_vis = None;

        for (i, t_val) in h_times.iter().enumerate() {
            let Some(time) = t_val.as_str() else { continue };
            let Some(temp) = h_temps.get(i).and_then(Value::as_f64) else { continue };
            let app = h_app.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(temp);
            let prob = h_prob
                .and_then(|a| a.get(i))
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .clamp(0.0, 100.0) as u8;
            let prec = h_prec.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0);
            let code = h_code.and_then(|a| a.get(i)).and_then(Value::as_u64).unwrap_or(0) as u16;
            let day_flag = h_day.and_then(|a| a.get(i)).and_then(Value::as_u64).unwrap_or(1) != 0;
            let wind = h_wind.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0);
            let gusts = h_gusts.and_then(|a| a.get(i)).and_then(Value::as_f64);
            let wdir = h_wdir
                .and_then(|a| a.get(i))
                .and_then(Value::as_f64)
                .map(|d| d.rem_euclid(360.0) as u16);
            let uv = h_uv.and_then(|a| a.get(i)).and_then(Value::as_f64);
            let hum = h_hum
                .and_then(|a| a.get(i))
                .and_then(Value::as_f64)
                .map(|v| v.clamp(0.0, 100.0) as u8);
            let dew = h_dew.and_then(|a| a.get(i)).and_then(Value::as_f64);
            let pres = h_pres.and_then(|a| a.get(i)).and_then(Value::as_f64);
            let cloud = h_cloud
                .and_then(|a| a.get(i))
                .and_then(Value::as_f64)
                .map(|v| v.clamp(0.0, 100.0) as u8);
            let vis = h_vis.and_then(|a| a.get(i)).and_then(Value::as_f64);
            let rad = h_rad.and_then(|a| a.get(i)).and_then(Value::as_f64);

            if current_time.get(..13) == time.get(..13) {
                if current_uv.is_none() {
                    current_uv = uv;
                }
                if matched_vis.is_none() {
                    matched_vis = vis;
                }
            }

            hourly.push(HourlyForecast {
                time: time.to_string(),
                temperature_c: temp,
                apparent_temperature_c: app,
                precipitation_probability_pct: prob,
                precipitation_mm: prec,
                condition: WeatherCondition::from_wmo(code),
                is_day: day_flag,
                wind_speed_kmh: wind,
                wind_gusts_kmh: gusts,
                wind_direction_deg: wdir,
                uv_index: uv,
                humidity_pct: hum,
                dew_point_c: dew,
                pressure_hpa: pres,
                cloud_cover_pct: cloud,
                visibility_m: vis,
                shortwave_radiation_wm2: rad,
            });
        }

        let daily_obj = &forecast_val["daily"];
        let d_dates = daily_obj["time"].as_array().ok_or(ProviderError::InvalidResponse)?;
        let d_code = daily_obj["weather_code"].as_array();
        let d_max = daily_obj["temperature_2m_max"].as_array().ok_or(ProviderError::InvalidResponse)?;
        let d_min = daily_obj["temperature_2m_min"].as_array().ok_or(ProviderError::InvalidResponse)?;
        let d_app_max = daily_obj["apparent_temperature_max"].as_array();
        let d_app_min = daily_obj["apparent_temperature_min"].as_array();
        let d_sunrise = daily_obj["sunrise"].as_array();
        let d_sunset = daily_obj["sunset"].as_array();
        let d_daylight = daily_obj["daylight_duration"].as_array();
        let d_sunshine = daily_obj["sunshine_duration"].as_array();
        let d_prec_sum = daily_obj["precipitation_sum"].as_array();
        let d_prec_hrs = daily_obj["precipitation_hours"].as_array();
        let d_prec_prob = daily_obj["precipitation_probability_max"].as_array();
        let d_uv_max = daily_obj["uv_index_max"].as_array();
        let d_wind_max = daily_obj["wind_speed_10m_max"].as_array();
        let d_gust_max = daily_obj["wind_gusts_10m_max"].as_array();
        let d_wind_dir = daily_obj["wind_direction_10m_dominant"].as_array();
        let d_rad_sum = daily_obj["shortwave_radiation_sum"].as_array();

        let mut daily = Vec::with_capacity(d_dates.len());
        for (i, dt_val) in d_dates.iter().enumerate() {
            let Some(date) = dt_val.as_str() else { continue };
            let Some(temp_max_c) = d_max.get(i).and_then(Value::as_f64) else { continue };
            let Some(temp_min_c) = d_min.get(i).and_then(Value::as_f64) else { continue };
            let code = d_code.and_then(|a| a.get(i)).and_then(Value::as_u64).unwrap_or(0) as u16;
            daily.push(DailyForecast {
                date: date.to_string(),
                condition: WeatherCondition::from_wmo(code),
                temp_max_c,
                temp_min_c,
                apparent_max_c: d_app_max.and_then(|a| a.get(i)).and_then(Value::as_f64),
                apparent_min_c: d_app_min.and_then(|a| a.get(i)).and_then(Value::as_f64),
                sunrise: d_sunrise.and_then(|a| a.get(i)).and_then(Value::as_str).map(str::to_string),
                sunset: d_sunset.and_then(|a| a.get(i)).and_then(Value::as_str).map(str::to_string),
                precipitation_sum_mm: d_prec_sum.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0),
                precipitation_probability_max_pct: d_prec_prob
                    .and_then(|a| a.get(i))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    .clamp(0.0, 100.0) as u8,
                precipitation_hours: d_prec_hrs.and_then(|a| a.get(i)).and_then(Value::as_f64),
                uv_index_max: d_uv_max.and_then(|a| a.get(i)).and_then(Value::as_f64),
                wind_speed_max_kmh: d_wind_max.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0),
                wind_gusts_max_kmh: d_gust_max.and_then(|a| a.get(i)).and_then(Value::as_f64),
                wind_direction_dominant_deg: d_wind_dir
                    .and_then(|a| a.get(i))
                    .and_then(Value::as_f64)
                    .map(|d| d.rem_euclid(360.0) as u16),
                daylight_duration_secs: d_daylight.and_then(|a| a.get(i)).and_then(Value::as_f64),
                sunshine_duration_secs: d_sunshine.and_then(|a| a.get(i)).and_then(Value::as_f64),
                shortwave_radiation_sum_mj: d_rad_sum.and_then(|a| a.get(i)).and_then(Value::as_f64),
            });
        }

        let air_quality = aq_val.and_then(|v| {
            let c = &v["current"];
            if !c.is_object() {
                return None;
            }
            if current_uv.is_none() {
                current_uv = c["uv_index"].as_f64();
            }
            // Hourly values start at local midnight; keep them from the current hour on,
            // aligned with their times (a null stays a gap).
            let h_aq = &v["hourly"];
            let times = h_aq["time"].as_array().cloned().unwrap_or_default();
            let hour = current_time.get(..13).unwrap_or(&current_time);
            let start = times
                .iter()
                .position(|t| t.as_str().is_some_and(|t| t.get(..13).unwrap_or(t) >= hour))
                .unwrap_or(0);
            let hourly_start = times.get(start).and_then(Value::as_str).map(str::to_string);
            let series = |key: &str| -> Vec<Option<f64>> {
                h_aq[key]
                    .as_array()
                    .map(|arr| arr.iter().skip(start).take(24).map(Value::as_f64).collect())
                    .unwrap_or_default()
            };
            let hourly_us_aqi = series("us_aqi")
                .into_iter()
                .map(|v| v.map(|n| n.round().clamp(0.0, 500.0) as u16))
                .collect();
            let hourly_pm2_5 = series("pm2_5");
            let hourly_uv = series("uv_index");

            Some(AirQuality {
                us_aqi: c["us_aqi"].as_f64().map(|n| n.round() as u16),
                european_aqi: c["european_aqi"].as_f64().map(|n| n.round() as u16),
                pm2_5: c["pm2_5"].as_f64(),
                pm10: c["pm10"].as_f64(),
                ozone: c["ozone"].as_f64(),
                nitrogen_dioxide: c["nitrogen_dioxide"].as_f64(),
                sulphur_dioxide: c["sulphur_dioxide"].as_f64(),
                carbon_monoxide: c["carbon_monoxide"].as_f64(),
                aerosol_optical_depth: c["aerosol_optical_depth"].as_f64(),
                dust: c["dust"].as_f64(),
                uv_index_clear_sky: c["uv_index_clear_sky"].as_f64(),
                alder_pollen: c["alder_pollen"].as_f64(),
                birch_pollen: c["birch_pollen"].as_f64(),
                grass_pollen: c["grass_pollen"].as_f64(),
                mugwort_pollen: c["mugwort_pollen"].as_f64(),
                olive_pollen: c["olive_pollen"].as_f64(),
                ragweed_pollen: c["ragweed_pollen"].as_f64(),
                hourly_start,
                hourly_us_aqi,
                hourly_pm2_5,
                hourly_uv,
            })
        });

        let minutely_precip = Self::parse_minutely(forecast_val, &current_time);
        let current = CurrentWeather {
            time: current_time,
            temperature_c,
            apparent_temperature_c,
            condition: WeatherCondition::from_wmo(wmo),
            is_day,
            humidity_pct,
            dew_point_c,
            wind_speed_kmh,
            wind_gusts_kmh,
            wind_direction_deg,
            pressure_hpa,
            precipitation_mm,
            cloud_cover_pct,
            visibility_m: visibility_m.or(matched_vis),
            uv_index: current_uv,
        };


        Ok(ForecastSnapshot {
            location: loc,
            fetched_at,
            utc_offset_seconds,
            current,
            hourly,
            daily,
            air_quality,
            alerts: Vec::new(),
            alerts_unavailable: false,
            models: Vec::new(),
            minutely_precip,
            attribution,
        })
    }

    /// The next two hours of 15-minute precipitation, from the step containing `current_time`.
    fn parse_minutely(forecast_val: &Value, current_time: &str) -> Vec<(String, f64)> {
        let m = &forecast_val["minutely_15"];
        let (Some(times), Some(precip)) = (m["time"].as_array(), m["precipitation"].as_array()) else {
            return Vec::new();
        };
        let pairs: Vec<(String, f64)> = times
            .iter()
            .zip(precip)
            .filter_map(|(t, p)| Some((t.as_str()?.to_string(), p.as_f64()?)))
            .collect();
        // The step containing now is the last one that starts at or before it.
        let start = pairs.iter().rposition(|(t, _)| t.as_str() <= current_time).unwrap_or(0);
        pairs.into_iter().skip(start).take(9).collect()
    }

    /// Model comparison: the blended forecast already parsed, plus each model in
    /// `models_val` (one request with `models=`, whose keys end in `_<model id>`). A model
    /// the response lacks is left out; a missing value is NaN, not a guess.
    pub fn build_model_series(
        daily: &[DailyForecast],
        hourly: &[HourlyForecast],
        models_val: Option<&Value>,
    ) -> Vec<crate::model::ModelSeries> {
        let mut out = Vec::new();
        if daily.is_empty() {
            return out;
        }
        out.push(crate::model::ModelSeries {
            model_id: "best_match".into(),
            model_name: "Open-Meteo Best Match".into(),
            resolution: "Seamless blend".into(),
            daily_max_c: daily.iter().map(|d| d.temp_max_c).collect(),
            daily_min_c: daily.iter().map(|d| d.temp_min_c).collect(),
            daily_precip_mm: daily.iter().map(|d| d.precipitation_sum_mm).collect(),
            hourly_temp_c: hourly.iter().map(|h| h.temperature_c).collect(),
        });
        let Some(v) = models_val else { return out };
        let values = |obj: &Value, key: &str| -> Option<Vec<f64>> {
            let a = obj[key].as_array()?;
            let vals: Vec<f64> = a.iter().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect();
            vals.iter().any(|x| x.is_finite()).then_some(vals)
        };
        for (id, name, res) in MODELS {
            let (d, h) = (&v["daily"], &v["hourly"]);
            let (Some(max), Some(min)) = (
                values(d, &format!("temperature_2m_max_{id}")),
                values(d, &format!("temperature_2m_min_{id}")),
            ) else {
                continue;
            };
            out.push(crate::model::ModelSeries {
                model_id: id.into(),
                model_name: name.into(),
                resolution: res.into(),
                daily_max_c: max,
                daily_min_c: min,
                daily_precip_mm: values(d, &format!("precipitation_sum_{id}")).unwrap_or_default(),
                hourly_temp_c: values(h, &format!("temperature_2m_{id}")).unwrap_or_default(),
            });
        }
        out
    }

    /// Current wind, temperature and sea-level pressure at a `size`×`size` grid over
    /// `bounds`, in one request.
    pub fn fetch_field(&self, bounds: Bounds, size: usize) -> Result<FieldGrid, ProviderError> {
        let coords = bounds.points(size);
        let join = |f: fn(&(f64, f64)) -> f64| coords.iter().map(|c| format!("{:.3}", f(c))).collect::<Vec<_>>().join(",");
        let v = self.get_json(
            &self.config.forecast_url,
            &[
                ("latitude", join(|c| c.0)),
                ("longitude", join(|c| c.1)),
                ("current", "temperature_2m,pressure_msl,wind_speed_10m,wind_direction_10m".into()),
                ("wind_speed_unit", "kmh".into()),
            ],
        )?;
        // Several locations come back as an array, one object each, in request order.
        let items = match &v {
            Value::Array(a) => a.clone(),
            other => vec![other.clone()],
        };
        if items.len() != coords.len() {
            return Err(ProviderError::InvalidResponse);
        }
        let num = |c: &Value, k: &str| c[k].as_f64().unwrap_or(f64::NAN);
        let points = coords
            .iter()
            .zip(&items)
            .map(|(&(lat, lon), item)| {
                let c = &item["current"];
                FieldPoint {
                    lat,
                    lon,
                    temperature_c: num(c, "temperature_2m"),
                    pressure_hpa: num(c, "pressure_msl"),
                    wind_speed_kmh: num(c, "wind_speed_10m"),
                    wind_direction_deg: num(c, "wind_direction_10m"),
                }
            })
            .collect();
        Ok(FieldGrid { size, bounds, points, fetched_at: crate::model::now_unix() })
    }

    /// The forecast URL to use: Open-Meteo's customer host when an API key is set
    /// and the URL is the free one.
    fn endpoint<'a>(&self, url: &'a str) -> std::borrow::Cow<'a, str> {
        let has_key = self.config.api_key.as_ref().is_some_and(|k| !k.trim().is_empty());
        match url.strip_prefix("https://") {
            Some(rest) if has_key && rest.contains("api.open-meteo.com/") && !rest.starts_with("customer-") => {
                format!("https://customer-{rest}").into()
            }
            _ => url.into(),
        }
    }
}

/// The models compared with the blended forecast: (Open-Meteo id, name, resolution).
const MODELS: [(&str, &str, &str); 3] = [
    ("ecmwf_ifs025", "ECMWF IFS", "0.25° (~25 km)"),
    ("gfs_seamless", "NOAA GFS", "0.25° / HRRR"),
    ("icon_seamless", "DWD ICON", "11 km / EU 7 km"),
];

impl WeatherProvider for OpenMeteoProvider {
    fn id(&self) -> &'static str {
        "open-meteo"
    }

    fn attribution(&self) -> ProviderAttribution {
        let commercial_allowed = self.config.commercial_use
            && (self.config.api_key.as_ref().is_some_and(|k| !k.trim().is_empty())
                || !self.config.forecast_url.contains("//api.open-meteo.com"));
        ProviderAttribution {
            provider_id: "open-meteo".into(),
            provider_name: "Open-Meteo".into(),
            license_name: "CC BY 4.0 (Non-Commercial on free hosted tier)".into(),
            attribution_text: "Weather data by Open-Meteo.com".into(),
            attribution_url: "https://open-meteo.com/".into(),
            commercial_use_allowed: commercial_allowed,
        }
    }

    fn search_locations(&self, query: &str, limit: usize) -> Result<Vec<Location>, ProviderError> {
        let q = query.trim();
        if q.len() < 2 {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 20);
        let v = self.get_json(
            &self.config.geocoding_url,
            &[
                ("name", q.to_string()),
                ("count", limit.to_string()),
                ("language", "en".into()),
                ("format", "json".into()),
            ],
        )?;
        Self::parse_geocoding_json(&v, limit)
    }

    fn fetch_forecast(&self, location: &Location) -> Result<ForecastSnapshot, ProviderError> {
        if !(-90.0..=90.0).contains(&location.latitude) || !(-180.0..=180.0).contains(&location.longitude) {
            return Err(ProviderError::InvalidLocation);
        }
        let lat = format!("{:.4}", location.latitude);
        let lon = format!("{:.4}", location.longitude);
        let tz = if location.timezone.trim().is_empty() {
            "auto"
        } else {
            location.timezone.as_str()
        };

        let forecast_val = self.get_json(
            &self.config.forecast_url,
            &[
                ("latitude", lat.clone()),
                ("longitude", lon.clone()),
                (
                    "current",
                    "temperature_2m,relative_humidity_2m,dew_point_2m,apparent_temperature,is_day,precipitation,weather_code,cloud_cover,surface_pressure,wind_speed_10m,wind_direction_10m,wind_gusts_10m,visibility".into(),
                ),
                (
                    "hourly",
                    "temperature_2m,apparent_temperature,precipitation_probability,precipitation,weather_code,is_day,wind_speed_10m,wind_gusts_10m,wind_direction_10m,uv_index,relative_humidity_2m,dew_point_2m,surface_pressure,cloud_cover,visibility,shortwave_radiation".into(),
                ),
                (
                    "daily",
                    "weather_code,temperature_2m_max,temperature_2m_min,apparent_temperature_max,apparent_temperature_min,sunrise,sunset,daylight_duration,sunshine_duration,uv_index_max,precipitation_sum,precipitation_hours,precipitation_probability_max,wind_speed_10m_max,wind_gusts_10m_max,wind_direction_10m_dominant,shortwave_radiation_sum".into(),
                ),
                ("minutely_15", "precipitation".into()),
                ("timezone", tz.to_string()),
                ("forecast_days", "10".into()),
                ("temperature_unit", "celsius".into()),
                ("wind_speed_unit", "kmh".into()),
                ("precipitation_unit", "mm".into()),
            ],
        )?;

        let aq_val = self
            .get_json(
                &self.config.air_quality_url,
                &[
                    ("latitude", lat),
                    ("longitude", lon),
                    (
                        "current",
                        "us_aqi,european_aqi,pm10,pm2_5,carbon_monoxide,nitrogen_dioxide,sulphur_dioxide,ozone,aerosol_optical_depth,dust,uv_index,uv_index_clear_sky,alder_pollen,birch_pollen,grass_pollen,mugwort_pollen,olive_pollen,ragweed_pollen".into(),
                    ),
                    (
                        "hourly",
                        "us_aqi,pm2_5,uv_index".into(),
                    ),
                    ("forecast_days", "3".into()),
                    ("timezone", tz.to_string()),
                ],
            )
            .ok();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let mut snapshot = Self::parse_forecast_json(
            location,
            &forecast_val,
            aq_val.as_ref(),
            now,
            self.attribution(),
        )?;

        // The same days and hours, from each compared model.
        let models_val = self
            .get_json(
                &self.config.forecast_url,
                &[
                    ("latitude", format!("{:.4}", location.latitude)),
                    ("longitude", format!("{:.4}", location.longitude)),
                    ("daily", "temperature_2m_max,temperature_2m_min,precipitation_sum".into()),
                    ("hourly", "temperature_2m".into()),
                    ("models", MODELS.map(|m| m.0).join(",")),
                    ("timezone", tz.to_string()),
                    ("forecast_days", "10".into()),
                ],
            )
            .ok();
        snapshot.models = Self::build_model_series(&snapshot.daily, &snapshot.hourly, models_val.as_ref());

        if self.config.fetch_us_noaa_alerts && snapshot.location.is_us() {
            match NoaaAlertProvider::default().fetch_alerts(location.latitude, location.longitude) {
                Ok(alerts) => snapshot.alerts = alerts,
                Err(_) => snapshot.alerts_unavailable = true,
            }
        }

        snapshot.advance_to(now);
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WeatherCondition;

    const SAMPLE_GEOCODING_JSON: &str = r#"{
        "results": [
            {
                "id": 4671654,
                "name": "Austin",
                "latitude": 30.26715,
                "longitude": -97.74306,
                "elevation": 149.0,
                "timezone": "America/Chicago",
                "country_code": "US",
                "country": "United States",
                "admin1": "Texas"
            },
            {
                "id": 2643743,
                "name": "London",
                "latitude": 51.50853,
                "longitude": -0.12574,
                "elevation": 25.0,
                "timezone": "Europe/London",
                "country_code": "GB",
                "country": "United Kingdom",
                "admin1": "England"
            }
        ]
    }"#;

    const SAMPLE_FORECAST_JSON: &str = r#"{
        "latitude": 30.26715,
        "longitude": -97.74306,
        "utc_offset_seconds": -18000,
        "timezone": "America/Chicago",
        "elevation": 149.0,
        "current": {
            "time": "2026-10-18T14:15",
            "temperature_2m": 24.5,
            "relative_humidity_2m": 54,
            "dew_point_2m": 14.6,
            "apparent_temperature": 25.1,
            "is_day": 1,
            "precipitation": 0.0,
            "weather_code": 2,
            "cloud_cover": 35,
            "surface_pressure": 1016.2,
            "wind_speed_10m": 14.8,
            "wind_direction_10m": 165,
            "wind_gusts_10m": 24.1,
            "visibility": 24140.0
        },
        "hourly": {
            "time": ["2026-10-18T13:00", "2026-10-18T14:00", "2026-10-18T15:00"],
            "temperature_2m": [23.2, 24.5, 25.0],
            "apparent_temperature": [23.8, 25.1, 25.4],
            "precipitation_probability": [5, 10, 20],
            "precipitation": [0.0, 0.0, 0.1],
            "weather_code": [1, 2, 2],
            "is_day": [1, 1, 1],
            "wind_speed_10m": [12.0, 14.8, 15.2],
            "uv_index": [5.2, 6.1, 5.4],
            "relative_humidity_2m": [58, 54, 51],
            "visibility": [24140.0, 24140.0, 24140.0]
        },
        "daily": {
            "time": ["2026-10-18", "2026-10-19"],
            "weather_code": [2, 61],
            "temperature_2m_max": [26.4, 22.1],
            "temperature_2m_min": [15.2, 13.8],
            "apparent_temperature_max": [27.0, 22.5],
            "apparent_temperature_min": [14.8, 13.1],
            "sunrise": ["2026-10-18T07:34", "2026-10-19T07:35"],
            "sunset": ["2026-10-18T18:55", "2026-10-19T18:54"],
            "uv_index_max": [6.4, 4.2],
            "precipitation_sum": [0.0, 6.8],
            "precipitation_probability_max": [15, 80],
            "wind_speed_10m_max": [18.5, 22.0],
            "wind_direction_10m_dominant": [170, 340]
        }
    }"#;

    const SAMPLE_AQ_JSON: &str = r#"{
        "current": {
            "time": "2026-10-18T14:00",
            "us_aqi": 34,
            "european_aqi": 22,
            "pm10": 14.2,
            "pm2_5": 8.1,
            "nitrogen_dioxide": 6.4,
            "ozone": 68.0,
            "uv_index": 6.1
        }
    }"#;

    #[test]
    fn parses_geocoding_fixture() {
        let v: Value = serde_json::from_str(SAMPLE_GEOCODING_JSON).unwrap();
        let locs = OpenMeteoProvider::parse_geocoding_json(&v, 5).unwrap();
        assert_eq!(locs.len(), 2);
        assert_eq!(locs[0].name, "Austin");
        assert_eq!(locs[0].subtitle(), "Texas, United States");
        assert!(locs[0].is_us());
        assert!(!locs[1].is_us());
    }

    #[test]
    fn parses_forecast_and_air_quality_fixtures() {
        let v_fc: Value = serde_json::from_str(SAMPLE_FORECAST_JSON).unwrap();
        let v_aq: Value = serde_json::from_str(SAMPLE_AQ_JSON).unwrap();
        let provider = OpenMeteoProvider::default();
        let loc = Location::new(
            "Austin",
            Some("Texas".into()),
            Some("United States".into()),
            Some("US".into()),
            30.2672,
            -97.7431,
            Some("America/Chicago".into()),
            Some(149.0),
        );

        let snap = OpenMeteoProvider::parse_forecast_json(
            &loc,
            &v_fc,
            Some(&v_aq),
            1_760_000_000,
            provider.attribution(),
        )
        .unwrap();

        assert_eq!(snap.current.temperature_c, 24.5);
        assert_eq!(snap.current.condition, WeatherCondition::PartlyCloudy);
        assert_eq!(snap.current.uv_index, Some(6.1));
        assert_eq!(snap.hourly.len(), 3);
        assert_eq!(snap.upcoming_hours(2).len(), 2);
        assert_eq!(snap.upcoming_hours(2)[0].time, "2026-10-18T14:00");
        assert_eq!(snap.daily.len(), 2);
        assert_eq!(snap.daily[1].condition, WeatherCondition::Rain);
        assert_eq!(snap.air_quality.as_ref().unwrap().us_aqi, Some(34));
    }

    fn fixture() -> ForecastSnapshot {
        let v_fc: Value = serde_json::from_str(SAMPLE_FORECAST_JSON).unwrap();
        let loc = Location::new("Austin", None, None, Some("US".into()), 30.27, -97.74, None, None);
        OpenMeteoProvider::parse_forecast_json(&loc, &v_fc, None, 0, OpenMeteoProvider::default().attribution()).unwrap()
    }

    /// Unix time of a local wall-clock time in the fixture's zone (UTC-5).
    fn local(s: &str) -> i64 {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").unwrap().and_utc().timestamp() + 18_000
    }

    #[test]
    fn advances_a_stale_snapshot() {
        let mut snap = fixture();
        snap.advance_to(local("2026-10-18T15:20"));
        // 14:15's observation is replaced by 15:00's forecast, and past hours go.
        assert_eq!(snap.hourly[0].time, "2026-10-18T15:00");
        assert_eq!(snap.current.temperature_c, 25.0);
        assert_eq!(snap.daily[0].date, "2026-10-18");

        // The next day: yesterday goes; hours that are all past are kept rather than emptied.
        snap.advance_to(local("2026-10-19T09:00"));
        assert_eq!(snap.daily.len(), 1);
        assert_eq!(snap.daily[0].date, "2026-10-19");
        assert_eq!(snap.hourly.len(), 1);

        // Nothing moves when the snapshot is current.
        let mut fresh = fixture();
        fresh.advance_to(local("2026-10-18T14:20"));
        assert_eq!(fresh.hourly.len(), 2);
        assert_eq!(fresh.current.temperature_c, 24.5);
    }

    #[test]
    fn drops_expired_alerts() {
        let mut snap = fixture();
        let alert = |event: &str, expires: Option<&str>| crate::model::WeatherAlert {
            id: event.into(),
            event: event.into(),
            severity: crate::model::AlertSeverity::Moderate,
            expires: expires.map(str::to_string),
            ..Default::default()
        };
        snap.alerts = vec![
            alert("Wind Advisory", Some("2026-10-18T14:00:00-05:00")),
            alert("Frost Advisory", Some("2026-10-18T18:00:00-05:00")),
            alert("Special Weather Statement", None),
        ];
        snap.advance_to(local("2026-10-18T15:00"));
        assert_eq!(snap.alerts.len(), 2);
    }

    #[test]
    fn nowcasts_rain() {
        let mut snap = fixture();
        let steps = |mm: &[f64]| mm.iter().enumerate().map(|(i, &m)| (format!("2026-10-18T14:{:02}", i * 15 % 60), m)).collect();
        snap.minutely_precip = steps(&[0.0, 0.0, 0.4, 0.5]);
        assert_eq!(snap.rain_nowcast().as_deref(), Some("Rain in 30 min"));
        snap.minutely_precip = steps(&[0.3, 0.2, 0.0]);
        assert_eq!(snap.rain_nowcast().as_deref(), Some("Rain stopping in 30 min"));
        snap.minutely_precip = steps(&[0.0, 0.0]);
        assert_eq!(snap.rain_nowcast(), None);
    }

    #[test]
    fn compares_only_models_that_answered() {
        let snap = fixture();
        assert_eq!(OpenMeteoProvider::build_model_series(&snap.daily, &snap.hourly, None).len(), 1);
        let models = serde_json::json!({
            "daily": {
                "temperature_2m_max_gfs_seamless": [26.0, null],
                "temperature_2m_min_gfs_seamless": [15.0, 13.0]
            },
            "hourly": {}
        });
        let series = OpenMeteoProvider::build_model_series(&snap.daily, &snap.hourly, Some(&models));
        assert_eq!(series.len(), 2);
        assert_eq!(series[1].model_id, "gfs_seamless");
        assert!(series[1].daily_max_c[1].is_nan());
    }

    #[test]
    fn aligns_air_quality_hours_with_now() {
        let v_fc: Value = serde_json::from_str(SAMPLE_FORECAST_JSON).unwrap();
        let aq = serde_json::json!({
            "current": {"us_aqi": 30},
            "hourly": {
                "time": ["2026-10-18T13:00", "2026-10-18T14:00", "2026-10-18T15:00"],
                "us_aqi": [10, null, 30],
                "pm2_5": [1.0, 2.0, 3.0]
            }
        });
        let loc = Location::new("Austin", None, None, Some("US".into()), 30.27, -97.74, None, None);
        let snap = OpenMeteoProvider::parse_forecast_json(&loc, &v_fc, Some(&aq), 0, OpenMeteoProvider::default().attribution()).unwrap();
        let aq = snap.air_quality.unwrap();
        assert_eq!(aq.hourly_start.as_deref(), Some("2026-10-18T14:00"));
        assert_eq!(aq.hourly_us_aqi, vec![None, Some(30)]);
        assert_eq!(aq.hourly_pm2_5, vec![Some(2.0), Some(3.0)]);
    }

    #[test]
    fn uses_the_customer_host_with_a_key() {
        let keyed = OpenMeteoProvider::new(OpenMeteoConfig { api_key: Some("k".into()), ..Default::default() });
        assert_eq!(keyed.endpoint(DEFAULT_FORECAST_URL), "https://customer-api.open-meteo.com/v1/forecast");
        assert_eq!(
            keyed.endpoint(DEFAULT_AIR_QUALITY_URL),
            "https://customer-air-quality-api.open-meteo.com/v1/air-quality"
        );
        assert_eq!(keyed.endpoint("https://meteo.example.com/v1/forecast"), "https://meteo.example.com/v1/forecast");
        assert_eq!(OpenMeteoProvider::default().endpoint(DEFAULT_FORECAST_URL), DEFAULT_FORECAST_URL);
    }

    #[test]
    fn enforces_commercial_licensing_guard() {
        let provider = OpenMeteoProvider::new(OpenMeteoConfig {
            commercial_use: true,
            api_key: None,
            ..Default::default()
        });
        assert_eq!(
            provider.check_licensing(),
            Err(ProviderError::CommercialKeyRequired)
        );

        let with_key = OpenMeteoProvider::new(OpenMeteoConfig {
            commercial_use: true,
            api_key: Some("comm-key-123".into()),
            ..Default::default()
        });
        assert!(with_key.check_licensing().is_ok());
    }
}
