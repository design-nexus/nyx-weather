//! Normalized weather domain models and unit conversions.

use serde::{Deserialize, Serialize};

/// Canonical location record used across providers and saved preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    /// Deterministic coordinate-based identifier (`"{lat:.4},{lon:.4}"`).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub admin1: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub country_code: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub elevation_m: Option<f64>,
}

fn default_timezone() -> String {
    "auto".into()
}

impl Location {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: impl Into<String>,
        admin1: Option<String>,
        country: Option<String>,
        country_code: Option<String>,
        latitude: f64,
        longitude: f64,
        timezone: Option<String>,
        elevation_m: Option<f64>,
    ) -> Self {
        let id = Self::make_id(latitude, longitude);
        Self {
            id,
            name: name.into(),
            admin1: admin1.filter(|s| !s.trim().is_empty()),
            country: country.filter(|s| !s.trim().is_empty()),
            country_code: country_code.filter(|s| !s.trim().is_empty()),
            latitude,
            longitude,
            timezone: timezone.filter(|s| !s.trim().is_empty()).unwrap_or_else(default_timezone),
            elevation_m,
        }
    }

    pub fn make_id(latitude: f64, longitude: f64) -> String {
        format!("{latitude:.4},{longitude:.4}")
    }

    /// Region and country subtitle, e.g. `"Texas, United States"`.
    pub fn subtitle(&self) -> String {
        match (&self.admin1, &self.country) {
            (Some(a), Some(c)) if a != &self.name && a != c => format!("{a}, {c}"),
            (None, Some(c)) if c != &self.name => c.clone(),
            (Some(a), None) if a != &self.name => a.clone(),
            (Some(_), Some(c)) => c.clone(),
            _ => format!("{:.2}°, {:.2}°", self.latitude, self.longitude),
        }
    }

    /// Full display label (`"Austin, Texas, United States"`).
    pub fn display_name(&self) -> String {
        let sub = self.subtitle();
        if sub.is_empty() {
            self.name.clone()
        } else {
            format!("{} · {}", self.name, sub)
        }
    }

    /// True when the location is within the United States (eligible for NOAA/NWS alerts).
    pub fn is_us(&self) -> bool {
        if let Some(code) = &self.country_code {
            return code.eq_ignore_ascii_case("US");
        }
        if let Some(country) = &self.country {
            let c = country.to_lowercase();
            return c == "united states" || c == "usa" || c == "united states of america";
        }
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

impl TemperatureUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Celsius => "celsius",
            Self::Fahrenheit => "fahrenheit",
        }
    }

    pub fn from_id(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "fahrenheit" | "f" => Self::Fahrenheit,
            _ => Self::Celsius,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Celsius => "°C",
            Self::Fahrenheit => "°F",
        }
    }

    pub fn convert(self, celsius: f64) -> f64 {
        match self {
            Self::Celsius => celsius,
            Self::Fahrenheit => celsius * 9.0 / 5.0 + 32.0,
        }
    }

    pub fn format(self, celsius: f64) -> String {
        // `+ 0.0` turns a rounded -0 into 0.
        format!("{:.0}°", self.convert(celsius).round() + 0.0)
    }

    pub fn format_with_unit(self, celsius: f64) -> String {
        format!("{:.0}{}", self.convert(celsius).round() + 0.0, self.symbol())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindSpeedUnit {
    #[default]
    Kmh,
    Mph,
    Ms,
    Knots,
}

impl WindSpeedUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kmh => "kmh",
            Self::Mph => "mph",
            Self::Ms => "ms",
            Self::Knots => "knots",
        }
    }

    pub fn from_id(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "mph" => Self::Mph,
            "ms" | "m/s" => Self::Ms,
            "knots" | "kn" => Self::Knots,
            _ => Self::Kmh,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Kmh => "km/h",
            Self::Mph => "mph",
            Self::Ms => "m/s",
            Self::Knots => "kn",
        }
    }

    pub fn convert(self, kmh: f64) -> f64 {
        match self {
            Self::Kmh => kmh,
            Self::Mph => kmh * 0.621_371,
            Self::Ms => kmh / 3.6,
            Self::Knots => kmh * 0.539_957,
        }
    }

    pub fn format(self, kmh: f64) -> String {
        match self {
            Self::Ms => format!("{:.1} {}", self.convert(kmh), self.symbol()),
            _ => format!("{:.0} {}", self.convert(kmh).round(), self.symbol()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PrecipitationUnit {
    #[default]
    Millimeters,
    Inches,
}

impl PrecipitationUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Millimeters => "mm",
            Self::Inches => "inch",
        }
    }

    pub fn from_id(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "inch" | "in" | "inches" => Self::Inches,
            _ => Self::Millimeters,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Millimeters => "mm",
            Self::Inches => "in",
        }
    }

    pub fn convert(self, mm: f64) -> f64 {
        match self {
            Self::Millimeters => mm,
            Self::Inches => mm / 25.4,
        }
    }

    pub fn format(self, mm: f64) -> String {
        match self {
            Self::Millimeters => {
                if mm < 1.0 && mm > 0.0 {
                    format!("{mm:.1} mm")
                } else {
                    format!("{:.0} mm", mm.round())
                }
            }
            Self::Inches => format!("{:.2} in", self.convert(mm)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PressureUnit {
    #[default]
    Hpa,
    Inhg,
    Mmhg,
}

impl PressureUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hpa => "hpa",
            Self::Inhg => "inhg",
            Self::Mmhg => "mmhg",
        }
    }

    pub fn from_id(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "inhg" => Self::Inhg,
            "mmhg" => Self::Mmhg,
            _ => Self::Hpa,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Hpa => "hPa",
            Self::Inhg => "inHg",
            Self::Mmhg => "mmHg",
        }
    }

    pub fn convert(self, hpa: f64) -> f64 {
        match self {
            Self::Hpa => hpa,
            Self::Inhg => hpa * 0.029_529_983,
            Self::Mmhg => hpa * 0.750_061_68,
        }
    }

    pub fn format(self, hpa: f64) -> String {
        match self {
            Self::Inhg => format!("{:.2} inHg", self.convert(hpa)),
            _ => format!("{:.0} {}", self.convert(hpa), self.symbol()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DistanceUnit {
    #[default]
    Kilometers,
    Miles,
}

impl DistanceUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kilometers => "km",
            Self::Miles => "mi",
        }
    }

    pub fn from_id(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "mi" | "miles" => Self::Miles,
            _ => Self::Kilometers,
        }
    }

    /// A visibility or distance in metres, as "12 km" or "7.5 mi".
    pub fn format_m(self, metres: f64) -> String {
        let v = match self {
            Self::Kilometers => metres / 1000.0,
            Self::Miles => metres / 1609.344,
        };
        if v < 10.0 { format!("{v:.1} {}", self.as_str()) } else { format!("{v:.0} {}", self.as_str()) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UnitPrefs {
    pub temperature: TemperatureUnit,
    pub wind_speed: WindSpeedUnit,
    pub precipitation: PrecipitationUnit,
    #[serde(default)]
    pub pressure: PressureUnit,
    #[serde(default)]
    pub distance: DistanceUnit,
    pub use_24h_time: bool,
}

/// Normalized weather condition across all providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WeatherCondition {
    Clear,
    MostlyClear,
    PartlyCloudy,
    Overcast,
    Fog,
    Drizzle,
    FreezingDrizzle,
    Rain,
    HeavyRain,
    FreezingRain,
    Snow,
    HeavySnow,
    SnowGrains,
    RainShowers,
    SnowShowers,
    Thunderstorm,
    ThunderstormHail,
    #[default]
    Unknown,
}

impl WeatherCondition {
    /// Map WMO weather interpretation code (WW) to [`WeatherCondition`].
    pub fn from_wmo(code: u16) -> Self {
        match code {
            0 => Self::Clear,
            1 => Self::MostlyClear,
            2 => Self::PartlyCloudy,
            3 => Self::Overcast,
            45 | 48 => Self::Fog,
            51 | 53 | 55 => Self::Drizzle,
            56 | 57 => Self::FreezingDrizzle,
            61 | 63 => Self::Rain,
            65 => Self::HeavyRain,
            66 | 67 => Self::FreezingRain,
            71 | 73 => Self::Snow,
            75 => Self::HeavySnow,
            77 => Self::SnowGrains,
            80..=82 => Self::RainShowers,
            85 | 86 => Self::SnowShowers,
            95 => Self::Thunderstorm,
            96 | 99 => Self::ThunderstormHail,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Clear => "Clear sky",
            Self::MostlyClear => "Mostly clear",
            Self::PartlyCloudy => "Partly cloudy",
            Self::Overcast => "Overcast",
            Self::Fog => "Fog",
            Self::Drizzle => "Light drizzle",
            Self::FreezingDrizzle => "Freezing drizzle",
            Self::Rain => "Rain",
            Self::HeavyRain => "Heavy rain",
            Self::FreezingRain => "Freezing rain",
            Self::Snow => "Snow",
            Self::HeavySnow => "Heavy snow",
            Self::SnowGrains => "Snow grains",
            Self::RainShowers => "Rain showers",
            Self::SnowShowers => "Snow showers",
            Self::Thunderstorm => "Thunderstorm",
            Self::ThunderstormHail => "Thunderstorm with hail",
            Self::Unknown => "Unknown",
        }
    }

    /// Standard freedesktop symbolic icon name for GTK.
    pub fn icon_name(self, is_day: bool) -> &'static str {
        match self {
            Self::Clear | Self::MostlyClear => {
                if is_day {
                    "weather-clear-symbolic"
                } else {
                    "weather-clear-night-symbolic"
                }
            }
            Self::PartlyCloudy => {
                if is_day {
                    "weather-few-clouds-symbolic"
                } else {
                    "weather-few-clouds-night-symbolic"
                }
            }
            Self::Overcast => "weather-overcast-symbolic",
            Self::Fog => "weather-fog-symbolic",
            Self::Drizzle | Self::FreezingDrizzle | Self::RainShowers => "weather-showers-scattered-symbolic",
            Self::Rain | Self::HeavyRain | Self::FreezingRain => "weather-showers-symbolic",
            Self::Snow | Self::HeavySnow | Self::SnowGrains | Self::SnowShowers => "weather-snow-symbolic",
            Self::Thunderstorm | Self::ThunderstormHail => "weather-storm-symbolic",
            Self::Unknown => "weather-overcast-symbolic",
        }
    }

    /// Monospace glyph suitable for Waybar / status bars.
    pub fn waybar_glyph(self, is_day: bool) -> &'static str {
        match self {
            Self::Clear | Self::MostlyClear => {
                if is_day {
                    "☀"
                } else {
                    "☾"
                }
            }
            Self::PartlyCloudy => "⛅",
            Self::Overcast => "☁",
            Self::Fog => "≋",
            Self::Drizzle | Self::FreezingDrizzle | Self::RainShowers => "🌦",
            Self::Rain | Self::HeavyRain | Self::FreezingRain => "🌧",
            Self::Snow | Self::HeavySnow | Self::SnowGrains | Self::SnowShowers => "❄",
            Self::Thunderstorm | Self::ThunderstormHail => "⛈",
            Self::Unknown => "☁",
        }
    }

    /// CSS class identifier for Waybar styling.
    pub fn css_class(self, is_day: bool) -> &'static str {
        match self {
            Self::Clear | Self::MostlyClear => {
                if is_day {
                    "clear-day"
                } else {
                    "clear-night"
                }
            }
            Self::PartlyCloudy => "partly-cloudy",
            Self::Overcast => "overcast",
            Self::Fog => "fog",
            Self::Drizzle | Self::FreezingDrizzle | Self::Rain | Self::HeavyRain | Self::FreezingRain | Self::RainShowers => {
                "rain"
            }
            Self::Snow | Self::HeavySnow | Self::SnowGrains | Self::SnowShowers => "snow",
            Self::Thunderstorm | Self::ThunderstormHail => "storm",
            Self::Unknown => "unknown",
        }
    }

    pub fn is_wet(self) -> bool {
        matches!(
            self,
            Self::Drizzle
                | Self::FreezingDrizzle
                | Self::Rain
                | Self::HeavyRain
                | Self::FreezingRain
                | Self::RainShowers
                | Self::Snow
                | Self::HeavySnow
                | Self::SnowGrains
                | Self::SnowShowers
                | Self::Thunderstorm
                | Self::ThunderstormHail
        )
    }
}

/// Convert compass bearing degrees (0..=360) to 8-point cardinal direction.
pub fn cardinal_direction(degrees: u16) -> &'static str {
    let dirs = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let idx = (((degrees as f64 + 22.5).rem_euclid(360.0)) / 45.0).floor() as usize;
    dirs[idx.min(7)]
}

/// Human-readable UV risk level.
pub fn uv_category(uv: f64) -> &'static str {
    match uv {
        x if x < 3.0 => "Low",
        x if x < 6.0 => "Moderate",
        x if x < 8.0 => "High",
        x if x < 11.0 => "Very high",
        _ => "Extreme",
    }
}

/// Human-readable US EPA AQI category.
pub fn us_aqi_category(aqi: u16) -> &'static str {
    match aqi {
        0..=50 => "Good",
        51..=100 => "Moderate",
        101..=150 => "Unhealthy for sensitive groups",
        151..=200 => "Unhealthy",
        201..=300 => "Very unhealthy",
        _ => "Hazardous",
    }
}

/// Beaufort wind scale (0..=12) and descriptive label from wind speed in km/h.
pub fn beaufort_scale(wind_kmh: f64) -> (u8, &'static str) {
    match wind_kmh {
        x if x < 1.0 => (0, "Calm"),
        x if x < 6.0 => (1, "Light air"),
        x if x < 12.0 => (2, "Light breeze"),
        x if x < 20.0 => (3, "Gentle breeze"),
        x if x < 29.0 => (4, "Moderate breeze"),
        x if x < 39.0 => (5, "Fresh breeze"),
        x if x < 50.0 => (6, "Strong breeze"),
        x if x < 62.0 => (7, "High wind"),
        x if x < 75.0 => (8, "Gale"),
        x if x < 89.0 => (9, "Strong gale"),
        x if x < 103.0 => (10, "Storm"),
        x if x < 118.0 => (11, "Violent storm"),
        _ => (12, "Hurricane force"),
    }
}

/// Human-readable pollen category from grains/m³.
pub fn pollen_category(grains: f64) -> &'static str {
    match grains {
        x if x <= 0.5 => "None",
        x if x < 15.0 => "Low",
        x if x < 50.0 => "Moderate",
        x if x < 150.0 => "High",
        _ => "Very high",
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoonPhaseInfo {
    /// Phase fraction in `0.0..1.0` (`0.0` = New Moon, `0.5` = Full Moon).
    pub phase_fraction: f64,
    /// Illuminated percentage (`0..=100`).
    pub illumination_pct: u8,
    /// Lunar age in days (`0.0..29.53`).
    pub age_days: f64,
    pub phase_name: &'static str,
    pub glyph: &'static str,
    pub days_to_full: f64,
    pub days_to_new: f64,
}

/// Compute lunar phase, illumination, and days to next full/new moon from a Unix epoch timestamp.
pub fn calculate_moon_phase(unix_epoch: i64) -> MoonPhaseInfo {
    // Reference new moon: 2000-01-06 18:14 UTC = 947182440
    const SYNODIC_DAYS: f64 = 29.530588853;
    let days_since_ref = (unix_epoch as f64 - 947_182_440.0) / 86_400.0;
    let age_days = days_since_ref.rem_euclid(SYNODIC_DAYS);
    let phase_fraction = (age_days / SYNODIC_DAYS).clamp(0.0, 1.0);
    let illum = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * phase_fraction).cos());
    let illumination_pct = (illum * 100.0).round().clamp(0.0, 100.0) as u8;

    let (phase_name, glyph) = match phase_fraction {
        f if !(0.03..0.97).contains(&f) => ("New Moon", "🌑"),
        f if f < 0.22 => ("Waxing Crescent", "🌒"),
        f if f < 0.28 => ("First Quarter", "🌓"),
        f if f < 0.47 => ("Waxing Gibbous", "🌔"),
        f if f < 0.53 => ("Full Moon", "🌕"),
        f if f < 0.72 => ("Waning Gibbous", "🌖"),
        f if f < 0.78 => ("Last Quarter", "🌗"),
        _ => ("Waning Crescent", "🌘"),
    };

    let full_age = SYNODIC_DAYS * 0.5;
    let days_to_full = if age_days <= full_age {
        full_age - age_days
    } else {
        SYNODIC_DAYS - age_days + full_age
    };
    let days_to_new = SYNODIC_DAYS - age_days;

    MoonPhaseInfo {
        phase_fraction,
        illumination_pct,
        age_days,
        phase_name,
        glyph,
        days_to_full,
        days_to_new,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrentWeather {
    pub time: String,
    pub temperature_c: f64,
    pub apparent_temperature_c: f64,
    pub condition: WeatherCondition,
    pub is_day: bool,
    pub humidity_pct: u8,
    pub dew_point_c: Option<f64>,
    pub wind_speed_kmh: f64,
    pub wind_gusts_kmh: Option<f64>,
    pub wind_direction_deg: u16,
    pub pressure_hpa: Option<f64>,
    pub precipitation_mm: f64,
    pub cloud_cover_pct: u8,
    pub visibility_m: Option<f64>,
    pub uv_index: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HourlyForecast {
    pub time: String,
    pub temperature_c: f64,
    pub apparent_temperature_c: f64,
    pub precipitation_probability_pct: u8,
    pub precipitation_mm: f64,
    pub condition: WeatherCondition,
    pub is_day: bool,
    pub wind_speed_kmh: f64,
    #[serde(default)]
    pub wind_gusts_kmh: Option<f64>,
    #[serde(default)]
    pub wind_direction_deg: Option<u16>,
    pub uv_index: Option<f64>,
    pub humidity_pct: Option<u8>,
    #[serde(default)]
    pub dew_point_c: Option<f64>,
    #[serde(default)]
    pub pressure_hpa: Option<f64>,
    #[serde(default)]
    pub cloud_cover_pct: Option<u8>,
    #[serde(default)]
    pub visibility_m: Option<f64>,
    #[serde(default)]
    pub shortwave_radiation_wm2: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyForecast {
    pub date: String,
    pub condition: WeatherCondition,
    pub temp_max_c: f64,
    pub temp_min_c: f64,
    pub apparent_max_c: Option<f64>,
    pub apparent_min_c: Option<f64>,
    pub sunrise: Option<String>,
    pub sunset: Option<String>,
    pub precipitation_sum_mm: f64,
    pub precipitation_probability_max_pct: u8,
    #[serde(default)]
    pub precipitation_hours: Option<f64>,
    pub uv_index_max: Option<f64>,
    pub wind_speed_max_kmh: f64,
    #[serde(default)]
    pub wind_gusts_max_kmh: Option<f64>,
    pub wind_direction_dominant_deg: Option<u16>,
    #[serde(default)]
    pub daylight_duration_secs: Option<f64>,
    #[serde(default)]
    pub sunshine_duration_secs: Option<f64>,
    #[serde(default)]
    pub shortwave_radiation_sum_mj: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AirQuality {
    pub us_aqi: Option<u16>,
    pub european_aqi: Option<u16>,
    pub pm2_5: Option<f64>,
    pub pm10: Option<f64>,
    pub ozone: Option<f64>,
    pub nitrogen_dioxide: Option<f64>,
    #[serde(default)]
    pub sulphur_dioxide: Option<f64>,
    #[serde(default)]
    pub carbon_monoxide: Option<f64>,
    #[serde(default)]
    pub aerosol_optical_depth: Option<f64>,
    #[serde(default)]
    pub dust: Option<f64>,
    #[serde(default)]
    pub uv_index_clear_sky: Option<f64>,
    #[serde(default)]
    pub alder_pollen: Option<f64>,
    #[serde(default)]
    pub birch_pollen: Option<f64>,
    #[serde(default)]
    pub grass_pollen: Option<f64>,
    #[serde(default)]
    pub mugwort_pollen: Option<f64>,
    #[serde(default)]
    pub olive_pollen: Option<f64>,
    #[serde(default)]
    pub ragweed_pollen: Option<f64>,
    /// Local time ("YYYY-MM-DDTHH:MM") of the first hourly entry below.
    #[serde(default)]
    pub hourly_start: Option<String>,
    #[serde(default)]
    pub hourly_us_aqi: Vec<Option<u16>>,
    #[serde(default)]
    pub hourly_pm2_5: Vec<Option<f64>>,
    #[serde(default)]
    pub hourly_uv: Vec<Option<f64>>,
}

/// JSON has no NaN: serde_json writes it as `null`. These read `null` back as NaN.
pub(crate) mod nan {
    use serde::{Deserialize, Deserializer};

    pub fn vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
        Ok(Vec::<Option<f64>>::deserialize(d)?.into_iter().map(|v| v.unwrap_or(f64::NAN)).collect())
    }

    pub fn one<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
    }
}

/// Multi-model comparison series (e.g. ECMWF IFS, NOAA GFS, DWD ICON). A missing value
/// (a day past the model's range) is NaN.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSeries {
    pub model_id: String,
    pub model_name: String,
    pub resolution: String,
    #[serde(deserialize_with = "nan::vec")]
    pub daily_max_c: Vec<f64>,
    #[serde(deserialize_with = "nan::vec")]
    pub daily_min_c: Vec<f64>,
    #[serde(deserialize_with = "nan::vec")]
    pub daily_precip_mm: Vec<f64>,
    #[serde(deserialize_with = "nan::vec")]
    pub hourly_temp_c: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    #[default]
    Minor,
    Moderate,
    Severe,
    Extreme,
}

impl AlertSeverity {
    /// The CAP severity. Whether an alert is a watch, warning or advisory is in its
    /// event name, not here.
    pub fn label(self) -> &'static str {
        match self {
            Self::Minor => "Minor",
            Self::Moderate => "Moderate",
            Self::Severe => "Severe",
            Self::Extreme => "Extreme",
        }
    }
}

/// When an alert's hazard is in force (RFC 3339 times).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AlertPeriod {
    pub onset: Option<String>,
    pub ends: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WeatherAlert {
    pub id: String,
    pub event: String,
    pub headline: String,
    pub description: String,
    pub instruction: Option<String>,
    pub severity: AlertSeverity,
    pub area: String,
    /// When the message was issued.
    pub effective: Option<String>,
    /// When the message expires (NWS updates it by then), not when the hazard ends.
    pub expires: Option<String>,
    pub sender: Option<String>,
    #[serde(default)]
    pub sent: Option<String>,
    /// The ids of the earlier messages this one updates.
    #[serde(default)]
    pub references: Vec<String>,
    /// VTEC event keys ("KBMX.FZ.W.0002"): the same hazard across its updates.
    #[serde(default)]
    pub vtec: Vec<String>,
    /// When it is in force, oldest first; more than one when one alert covers separate times.
    #[serde(default)]
    pub periods: Vec<AlertPeriod>,
    /// Every message id folded into this alert.
    #[serde(default)]
    pub merged_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAttribution {
    pub provider_id: String,
    pub provider_name: String,
    pub license_name: String,
    pub attribution_text: String,
    pub attribution_url: String,
    pub commercial_use_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForecastSnapshot {
    pub location: Location,
    pub fetched_at: i64,
    pub utc_offset_seconds: i32,
    pub current: CurrentWeather,
    pub hourly: Vec<HourlyForecast>,
    pub daily: Vec<DailyForecast>,
    pub air_quality: Option<AirQuality>,
    #[serde(default)]
    pub alerts: Vec<WeatherAlert>,
    /// True when the alert service was asked and didn't answer.
    #[serde(default)]
    pub alerts_unavailable: bool,
    #[serde(default)]
    pub models: Vec<ModelSeries>,
    /// Precipitation for each 15 minutes ahead, as (local "YYYY-MM-DDTHH:MM", mm).
    #[serde(default)]
    pub minutely_precip: Vec<(String, f64)>,
    pub attribution: ProviderAttribution,
}

/// Local wall-clock time at `now_unix` for a place `utc_offset_seconds` from UTC.
pub fn local_time(now_unix: i64, utc_offset_seconds: i32) -> chrono::NaiveDateTime {
    chrono::DateTime::from_timestamp(now_unix + utc_offset_seconds as i64, 0)
        .unwrap_or_default()
        .naive_utc()
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Hours from `start` ("YYYY-MM-DDTHH:MM") to `now`, or None if `start` doesn't parse.
fn hours_since(start: &str, now: chrono::NaiveDateTime) -> Option<i64> {
    let s = chrono::NaiveDateTime::parse_from_str(start.get(..16)?, "%Y-%m-%dT%H:%M").ok()?;
    Some((now - s).num_hours())
}

impl ForecastSnapshot {
    /// Today's high and low in Celsius, falling back to current temperature.
    pub fn today_high_low_c(&self) -> (f64, f64) {
        self.daily
            .first()
            .map(|d| (d.temp_max_c, d.temp_min_c))
            .unwrap_or((self.current.temperature_c, self.current.temperature_c))
    }

    /// Drop what is already past at `now_unix`: hours before this one, days before today,
    /// expired alerts and old nowcast steps. When the observation is from an earlier
    /// hour (the snapshot is a cached one), "now" becomes this hour's forecast. Lists
    /// that would be left empty are kept as they are.
    pub fn advance_to(&mut self, now_unix: i64) {
        let local = local_time(now_unix, self.utc_offset_seconds);
        let hour = local.format("%Y-%m-%dT%H").to_string();
        let today = local.format("%Y-%m-%d").to_string();

        let past_days = self.daily.iter().take_while(|d| d.date < today).count();
        if past_days > 0 && past_days < self.daily.len() {
            self.daily.drain(..past_days);
            for m in &mut self.models {
                for v in [&mut m.daily_max_c, &mut m.daily_min_c, &mut m.daily_precip_mm] {
                    v.drain(..past_days.min(v.len()));
                }
            }
        }

        let past_hours = self
            .hourly
            .iter()
            .take_while(|h| h.time.get(..13).unwrap_or(&h.time) < hour.as_str())
            .count();
        if past_hours > 0 && past_hours < self.hourly.len() {
            self.hourly.drain(..past_hours);
            for m in &mut self.models {
                m.hourly_temp_c.drain(..past_hours.min(m.hourly_temp_c.len()));
            }
        }

        let observed = self.current.time.get(..13).unwrap_or(&self.current.time);
        if observed < hour.as_str()
            && let Some(h) = self.hourly.first().filter(|h| h.time.starts_with(&hour))
        {
            let c = &mut self.current;
            c.time = h.time.clone();
            c.temperature_c = h.temperature_c;
            c.apparent_temperature_c = h.apparent_temperature_c;
            c.condition = h.condition;
            c.is_day = h.is_day;
            c.precipitation_mm = h.precipitation_mm;
            c.wind_speed_kmh = h.wind_speed_kmh;
            c.wind_gusts_kmh = h.wind_gusts_kmh;
            c.wind_direction_deg = h.wind_direction_deg.unwrap_or(c.wind_direction_deg);
            c.humidity_pct = h.humidity_pct.unwrap_or(c.humidity_pct);
            c.dew_point_c = h.dew_point_c;
            c.pressure_hpa = h.pressure_hpa.or(c.pressure_hpa);
            c.cloud_cover_pct = h.cloud_cover_pct.unwrap_or(c.cloud_cover_pct);
            c.visibility_m = h.visibility_m;
            c.uv_index = h.uv_index;
        }

        if let Some(aq) = &mut self.air_quality
            && let Some(n) = aq.hourly_start.as_deref().and_then(|s| hours_since(s, local))
            && n > 0
        {
            let n = n as usize;
            for v in [&mut aq.hourly_pm2_5, &mut aq.hourly_uv] {
                v.drain(..n.min(v.len()));
            }
            aq.hourly_us_aqi.drain(..n.min(aq.hourly_us_aqi.len()));
            aq.hourly_start = Some(format!("{hour}:00"));
        }

        // Keep the step that contains now (it started up to 15 minutes ago).
        let quarter = (local - chrono::Duration::minutes(15)).format("%Y-%m-%dT%H:%M").to_string();
        self.minutely_precip.retain(|(t, _)| t.as_str() > quarter.as_str());

        let alerts = std::mem::take(&mut self.alerts);
        self.alerts = crate::alerts::merge_alerts(alerts)
            .into_iter()
            .filter_map(|a| a.advanced_to(now_unix))
            .collect();
    }

    /// "Rain in 15 min", "Rain stopping in 30 min" or "Rain for the next 2 hours", from
    /// the 15-minute nowcast; None when no rain is coming in that window.
    pub fn rain_nowcast(&self) -> Option<String> {
        const WET: f64 = 0.1;
        let steps: Vec<f64> = self.minutely_precip.iter().take(8).map(|(_, mm)| *mm).collect();
        let first = *steps.first()?;
        let minutes = |i: usize| i * 15;
        if first >= WET {
            match steps.iter().position(|&mm| mm < WET) {
                Some(i) => Some(format!("Rain stopping in {} min", minutes(i))),
                None => Some("Rain for the next 2 hours".into()),
            }
        } else {
            steps.iter().position(|&mm| mm >= WET).map(|i| format!("Rain in {} min", minutes(i)))
        }
    }

    /// Up to `count` hourly entries starting at or after the current observation time.
    pub fn upcoming_hours(&self, count: usize) -> Vec<&HourlyForecast> {
        let current_prefix = self.current.time.get(..13).unwrap_or(&self.current.time);
        let start = self
            .hourly
            .iter()
            .position(|h| h.time.as_str() >= current_prefix)
            .unwrap_or(0);
        self.hourly.iter().skip(start).take(count).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_units_accurately() {
        assert_eq!(TemperatureUnit::Celsius.format(20.0), "20°");
        assert_eq!(TemperatureUnit::Fahrenheit.format(0.0), "32°");
        assert_eq!(TemperatureUnit::Fahrenheit.format(20.0), "68°");
        assert_eq!(TemperatureUnit::Celsius.format(-0.4), "0°");
        assert_eq!(PressureUnit::Inhg.format(1013.25), "29.92 inHg");
        assert_eq!(DistanceUnit::Miles.format_m(16_093.44), "10 mi");
        assert_eq!(WindSpeedUnit::Kmh.format(18.0), "18 km/h");
        assert_eq!(WindSpeedUnit::Ms.format(18.0), "5.0 m/s");
        assert_eq!(WindSpeedUnit::Mph.format(16.0934), "10 mph");
        assert_eq!(PrecipitationUnit::Millimeters.format(12.4), "12 mm");
        assert_eq!(PrecipitationUnit::Inches.format(25.4), "1.00 in");
    }

    #[test]
    fn maps_wmo_codes_and_cardinals() {
        assert_eq!(WeatherCondition::from_wmo(0), WeatherCondition::Clear);
        assert_eq!(WeatherCondition::from_wmo(2), WeatherCondition::PartlyCloudy);
        assert_eq!(WeatherCondition::from_wmo(63), WeatherCondition::Rain);
        assert_eq!(WeatherCondition::from_wmo(95), WeatherCondition::Thunderstorm);
        assert_eq!(cardinal_direction(0), "N");
        assert_eq!(cardinal_direction(45), "NE");
        assert_eq!(cardinal_direction(180), "S");
        assert_eq!(cardinal_direction(315), "NW");
        assert_eq!(cardinal_direction(359), "N");
        let moon = calculate_moon_phase(1_729_000_000);
        assert!(moon.illumination_pct <= 100);
        assert!(!moon.phase_name.is_empty());
    }
}
