//! Preferences for Weather (`~/.config/nyx-weather/settings.toml`).

pub use nyx_theme::ThemeMode;

use nyx_weather_core::{
    DistanceUnit, Location, OpenMeteoConfig, PrecipitationUnit, PressureUnit, TemperatureUnit,
    UnitPrefs, WindSpeedUnit,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub mode: ThemeMode,
    pub theme: String,
    pub glow: bool,
    pub reduce_motion: bool,
    pub sidebar_collapsed: bool,
    pub last_section: String,
    /// When true, adds a Hyprland floating window rule (`460×720`) and floats the window.
    pub float_window: bool,
    pub temperature_unit: TemperatureUnit,
    pub wind_speed_unit: WindSpeedUnit,
    pub precipitation_unit: PrecipitationUnit,
    pub pressure_unit: PressureUnit,
    pub distance_unit: DistanceUnit,
    pub use_24h_time: bool,
    /// Default layer on the Radar & Overlays page (`"radar"`, `"satellite"`, `"wind"`, `"temp"`).
    pub default_radar_layer: String,
    /// Radar overlay opacity (`40..=100` percent).
    pub radar_opacity_pct: u8,
    /// Forecast refresh interval in minutes (15, 30, or 60).
    pub refresh_interval_mins: u32,
    /// Active location ID (`"{lat:.4},{lon:.4}"`).
    pub active_location_id: String,
    /// Saved locations in the user's picker.
    pub saved_locations: Vec<Location>,
    /// Fetch active US severe-weather alerts from NOAA/NWS.
    pub us_noaa_alerts: bool,
    /// Send a desktop notification for each new Severe or Extreme alert.
    pub notify_alerts: bool,
    /// Alerts already notified (their ids, newest last).
    pub notified_alerts: Vec<String>,
    /// Whether this installation is used commercially (requires API key or self-hosted URL).
    pub commercial_use: bool,
    /// Optional Open-Meteo commercial API key.
    pub open_meteo_api_key: String,
    /// Optional custom Open-Meteo forecast endpoint URL.
    pub custom_forecast_url: String,
    /// Optional custom Open-Meteo air-quality endpoint URL.
    pub custom_air_quality_url: String,
    /// Optional custom Open-Meteo geocoding endpoint URL.
    pub custom_geocoding_url: String,
}

/// True when the locale measures in US customary units (the US, Liberia, Myanmar).
fn us_customary() -> bool {
    ["LC_ALL", "LC_MEASUREMENT", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .is_none_or(|v| ["_US", "_LR", "_MM"].iter().any(|c| v.contains(c)))
}

impl Default for Prefs {
    fn default() -> Self {
        let us = us_customary();
        let default_loc = Location::new(
            "New York",
            Some("New York".into()),
            Some("United States".into()),
            Some("US".into()),
            40.7143,
            -74.0060,
            Some("America/New_York".into()),
            Some(10.0),
        );
        Self {
            mode: ThemeMode::Omarchy,
            theme: "tokyo-night".into(),
            glow: true,
            reduce_motion: false,
            sidebar_collapsed: false,
            last_section: "overview".into(),
            float_window: false,
            temperature_unit: if us { TemperatureUnit::Fahrenheit } else { TemperatureUnit::Celsius },
            wind_speed_unit: if us { WindSpeedUnit::Mph } else { WindSpeedUnit::Kmh },
            precipitation_unit: if us { PrecipitationUnit::Inches } else { PrecipitationUnit::Millimeters },
            pressure_unit: if us { PressureUnit::Inhg } else { PressureUnit::Hpa },
            distance_unit: if us { DistanceUnit::Miles } else { DistanceUnit::Kilometers },
            use_24h_time: !us,
            default_radar_layer: "radar".into(),
            radar_opacity_pct: 80,
            refresh_interval_mins: 30,
            active_location_id: default_loc.id.clone(),
            saved_locations: vec![default_loc],
            us_noaa_alerts: true,
            notify_alerts: true,
            notified_alerts: Vec::new(),
            commercial_use: false,
            open_meteo_api_key: String::new(),
            custom_forecast_url: String::new(),
            custom_air_quality_url: String::new(),
            custom_geocoding_url: String::new(),
        }
    }
}

impl Prefs {
    pub fn units(&self) -> UnitPrefs {
        UnitPrefs {
            temperature: self.temperature_unit,
            wind_speed: self.wind_speed_unit,
            precipitation: self.precipitation_unit,
            pressure: self.pressure_unit,
            distance: self.distance_unit,
            use_24h_time: self.use_24h_time,
        }
    }

    pub fn active_location(&self) -> Option<Location> {
        self.saved_locations
            .iter()
            .find(|l| l.id == self.active_location_id)
            .or_else(|| self.saved_locations.first())
            .cloned()
    }

    pub fn open_meteo_config(&self) -> OpenMeteoConfig {
        let mut cfg = OpenMeteoConfig::default();
        if !self.custom_forecast_url.trim().is_empty() {
            cfg.forecast_url = self.custom_forecast_url.trim().to_string();
        }
        if !self.custom_air_quality_url.trim().is_empty() {
            cfg.air_quality_url = self.custom_air_quality_url.trim().to_string();
        }
        if !self.custom_geocoding_url.trim().is_empty() {
            cfg.geocoding_url = self.custom_geocoding_url.trim().to_string();
        }
        if !self.open_meteo_api_key.trim().is_empty() {
            cfg.api_key = Some(self.open_meteo_api_key.trim().to_string());
        }
        cfg.commercial_use = self.commercial_use;
        cfg.fetch_us_noaa_alerts = self.us_noaa_alerts;
        cfg
    }
}

/// Synchronously update `~/.config/hypr/nyx.lua` to match `float`.
/// Returns `true` if the file on disk was modified.
pub fn write_hypr_rule(float: bool) -> bool {
    let hypr_dir = crate::paths::home().join(".config/hypr");
    let nyx_lua = hypr_dir.join("nyx.lua");
    let rule_line =
        r#"o.window("^io\\.github\\.design_nexus\\.nyx\\.Weather$", { float = true, center = true, size = { 460, 720 } })"#;

    if !hypr_dir.is_dir() {
        return false;
    }
    let existing = std::fs::read_to_string(&nyx_lua).unwrap_or_else(|_| {
        "-- Written by the Nyx installer: Nyx apps that open as floating windows.\n-- Reinstalling an app rewrites this file.\n".to_string()
    });
    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|l| !l.contains("nyx\\.Weather") && !l.contains("nyx.Weather"))
        .collect();
    if float {
        lines.push(rule_line);
    }
    let mut out = lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if out == existing {
        return false;
    }
    let _ = crate::cmd::atomic_write(&nyx_lua, &out);
    true
}

/// Ensure `~/.config/hypr/nyx.lua` matches `float` before the window maps on startup.
/// If the rule file changed, reloads Hyprland synchronously so the new window maps with the right rule.
pub fn sync_hypr_rule_on_startup(float: bool) {
    // The rule is for the real app ID; a test copy leaves Hyprland's config alone.
    if crate::app_id() != crate::APP_ID {
        return;
    }
    if write_hypr_rule(float) {
        let _ = std::process::Command::new("hyprctl").arg("reload").output();
    }
}

/// Ensure any currently mapped Weather window in Hyprland matches `float`
/// (and is resized/centered to 460x720 when `float == true`).
pub fn ensure_live_window_state(float: bool) {
    let clients_out = std::process::Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());

    if let Some(arr) = clients_out.as_ref().and_then(|v| v.as_array()) {
        for c in arr {
            let class = c.get("class").and_then(|v| v.as_str()).unwrap_or("");
            if class != crate::app_id() {
                continue;
            }
            let Some(addr) = c.get("address").and_then(|v| v.as_str()) else {
                continue;
            };
            let is_floating = c.get("floating").and_then(|v| v.as_bool()).unwrap_or(false);
            let win_sel = format!("address:{addr}");

            if is_floating != float {
                let action = if float { "on" } else { "off" };
                let lua_float = format!(
                    r#"hl.dsp.window.float({{ window = "{win_sel}", action = "{action}" }})"#
                );
                let ok = std::process::Command::new("hyprctl")
                    .args(["dispatch", &lua_float])
                    .output()
                    .is_ok_and(|o| o.status.success());
                if !ok {
                    let _ = std::process::Command::new("hyprctl")
                        .args(["dispatch", "togglefloating", &win_sel])
                        .output();
                }
            }

            if float {
                let lua_resize = format!(
                    r#"hl.dsp.window.resize({{ window = "{win_sel}", x = 460, y = 720 }})"#
                );
                let ok_r = std::process::Command::new("hyprctl")
                    .args(["dispatch", &lua_resize])
                    .output()
                    .is_ok_and(|o| o.status.success());
                if !ok_r {
                    let _ = std::process::Command::new("hyprctl")
                        .args(["dispatch", "resizeactive", "exact", "460", "720", &win_sel])
                        .output();
                }

                let lua_center = format!(r#"hl.dsp.window.center({{ window = "{win_sel}" }})"#);
                let ok_c = std::process::Command::new("hyprctl")
                    .args(["dispatch", &lua_center])
                    .output()
                    .is_ok_and(|o| o.status.success());
                if !ok_c {
                    let _ = std::process::Command::new("hyprctl")
                        .args(["dispatch", "centerwindow", &win_sel])
                        .output();
                }
            }
        }
    }
}

/// Apply the floating-window preference to `~/.config/hypr/nyx.lua` and the live Hyprland session.
pub fn apply_floating_window(float: bool) {
    let rule_changed = crate::app_id() == crate::APP_ID && write_hypr_rule(float);

    std::thread::spawn(move || {
        if rule_changed {
            let _ = std::process::Command::new("hyprctl").arg("reload").output();
        }
        ensure_live_window_state(float);
    });
}

/// Read preferences directly from disk without initializing GTK (used by `--waybar` and `--refresh`).
pub fn read_disk() -> Prefs {
    let path = crate::paths::prefs_file();
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

nyx_core::prefs_store!(Prefs);
