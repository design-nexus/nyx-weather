//! Theming: the shared Nyx theme engine, plus rich semantic weather colors
//! derived from Omarchy's `colors.toml` or the active Nyx theme palette.

pub use nyx_theme::*;

use crate::prefs;
use nyx_weather_core::WeatherCondition;

/// Rich semantic colors for weather conditions, charts, and radar maps.
/// Changes automatically whenever the Omarchy theme or Nyx theme changes.
#[derive(Debug, Clone, Copy)]
pub struct WeatherColors {
    /// Yellow / Gold — Sun, clear sky, moderate alerts, daylight arc
    pub sun: Rgb,
    /// Orange / Amber — Warm temperatures, daily highs, elevated pollen/UV
    pub warm: Rgb,
    /// Cyan / Teal — Rain, precipitation probability, humidity, dew point
    pub rain: Rgb,
    /// Green / Mint — Wind, healthy AQI, low UV, parks
    pub wind: Rgb,
    /// Magenta / Violet — UV index, thunderstorms, ensemble models
    pub uv: Rgb,
    /// Blue / Ice — Cold temperatures, daily lows, snow, pressure
    pub cold: Rgb,
    /// Red / Coral — Severe alerts, extreme heat/AQI
    pub danger: Rgb,
}

fn rotate_hue(c: Rgb, deg: f64, min_sat: f64, target_light: Option<f64>) -> Rgb {
    let max = c.0.max(c.1).max(c.2);
    let min = c.0.min(c.1).min(c.2);
    let mut l = (max + min) * 0.5;
    let d = max - min;
    let (mut h, mut s) = if d < 1e-5 {
        (210.0, 0.5)
    } else {
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        let h = if (max - c.0).abs() < 1e-5 {
            ((c.1 - c.2) / d + if c.1 < c.2 { 6.0 } else { 0.0 }) * 60.0
        } else if (max - c.1).abs() < 1e-5 {
            ((c.2 - c.0) / d + 2.0) * 60.0
        } else {
            ((c.0 - c.1) / d + 4.0) * 60.0
        };
        (h, s)
    };

    h = (h + deg).rem_euclid(360.0);
    s = s.max(min_sat).clamp(0.35, 0.92);
    if let Some(tl) = target_light {
        l = (l * 0.45 + tl * 0.55).clamp(0.35, 0.82);
    }

    let c_val = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c_val * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c_val * 0.5;
    let (r1, g1, b1) = match h as u32 {
        0..=59 => (c_val, x, 0.0),
        60..=119 => (x, c_val, 0.0),
        120..=179 => (0.0, c_val, x),
        180..=239 => (0.0, x, c_val),
        240..=299 => (x, 0.0, c_val),
        _ => (c_val, 0.0, x),
    };
    Rgb(
        (r1 + m).clamp(0.0, 1.0),
        (g1 + m).clamp(0.0, 1.0),
        (b1 + m).clamp(0.0, 1.0),
    )
}

/// Resolve the active `WeatherColors` from Omarchy's `colors.toml` (when in Omarchy mode)
/// or by harmonizing against the active `Palette`.
pub fn weather_colors_for(p: &Palette) -> WeatherColors {
    // Charts ask on every frame; work it out (and read colors.toml) once per palette.
    thread_local! {
        static LAST: std::cell::RefCell<Option<(String, WeatherColors)>> = const { std::cell::RefCell::new(None) };
    }
    let key = format!("{:?}{}{}{}", prefs::get().mode, p.accent, p.danger, p.light);
    if let Some(wc) = LAST.with(|l| l.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, wc)| *wc)) {
        return wc;
    }
    let wc = derive_weather_colors(p);
    LAST.with(|l| *l.borrow_mut() = Some((key, wc)));
    wc
}

fn derive_weather_colors(p: &Palette) -> WeatherColors {
    let accent = Rgb::hex(&p.accent);
    let danger = Rgb::hex(&p.danger);
    let tl = if p.light { Some(0.46) } else { Some(0.72) };

    let mut sun = danger.mix(rotate_hue(accent, 155.0, 0.72, tl), 0.65);
    let mut warm = danger.mix(sun, 0.52);
    let mut rain = rotate_hue(accent, -28.0, 0.68, tl);
    let mut wind = rotate_hue(accent, -85.0, 0.62, tl);
    let mut uv = rotate_hue(accent, 65.0, 0.68, tl);
    let mut cold = accent;

    if prefs::get().mode == ThemeMode::Omarchy
        && let Ok(text) = std::fs::read_to_string(nyx_core::paths::omarchy_colors())
        && let Ok(table) = toml::from_str::<toml::Table>(&text)
    {
        let get_rgb = |key: &str| {
            table
                .get(key)
                .and_then(|v| v.as_str())
                .map(Rgb::hex)
        };
        if let Some(c) = get_rgb("yellow").or_else(|| get_rgb("bright_yellow")) {
            sun = c;
        }
        if let Some(c) = get_rgb("orange") {
            warm = c;
        } else {
            warm = sun.mix(danger, 0.45);
        }
        if let Some(c) = get_rgb("cyan").or_else(|| get_rgb("bright_cyan")) {
            rain = c;
        }
        if let Some(c) = get_rgb("green").or_else(|| get_rgb("bright_green")) {
            wind = c;
        }
        if let Some(c) = get_rgb("magenta").or_else(|| get_rgb("bright_magenta")) {
            uv = c;
        }
        if let Some(c) = get_rgb("blue").or_else(|| get_rgb("bright_blue")) {
            cold = c;
        }
    }

    WeatherColors {
        sun,
        warm,
        rain,
        wind,
        uv,
        cold,
        danger,
    }
}

fn extra_colors(p: &Palette) -> String {
    let wc = weather_colors_for(p);
    format!(
        "@define-color weather_sun {};\n\
         @define-color weather_warm {};\n\
         @define-color weather_rain {};\n\
         @define-color weather_wind {};\n\
         @define-color weather_uv {};\n\
         @define-color weather_cold {};\n",
        wc.sun.to_hex(),
        wc.warm.to_hex(),
        wc.rain.to_hex(),
        wc.wind.to_hex(),
        wc.uv.to_hex(),
        wc.cold.to_hex(),
    )
}

/// A swatch from the active theme (`"amber"`, `"blue"`), for drawings that follow the
/// sidebar's section colours rather than the weather palette.
pub fn swatch(name: &str) -> Option<Rgb> {
    nyx_theme::look().swatches.iter().find(|(n, _)| n == name).map(|(_, c)| Rgb::hex(c))
}

/// Semantic CSS class for coloring a weather condition icon with the active theme palette.
pub fn condition_color_class(cond: WeatherCondition, is_day: bool) -> &'static str {
    match cond {
        WeatherCondition::Clear => {
            if is_day {
                "cond-sun"
            } else {
                "cond-uv"
            }
        }
        WeatherCondition::MostlyClear => {
            if is_day {
                "cond-sun"
            } else {
                "cond-cold"
            }
        }
        WeatherCondition::PartlyCloudy => {
            if is_day {
                "cond-warm"
            } else {
                "cond-cold"
            }
        }
        WeatherCondition::Overcast | WeatherCondition::Fog => "cond-cloud",
        WeatherCondition::Drizzle
        | WeatherCondition::Rain
        | WeatherCondition::HeavyRain
        | WeatherCondition::RainShowers => "cond-rain",
        WeatherCondition::FreezingDrizzle
        | WeatherCondition::FreezingRain
        | WeatherCondition::Snow
        | WeatherCondition::HeavySnow
        | WeatherCondition::SnowGrains
        | WeatherCondition::SnowShowers => "cond-cold",
        WeatherCondition::Thunderstorm | WeatherCondition::ThunderstormHail => "cond-uv",
        WeatherCondition::Unknown => "cond-cloud",
    }
}

/// Icons the app brings (the icon theme has no radar, leaf, drop, gauge, umbrella or
/// line chart), written once to its cache and added to the icon theme. GTK fills every
/// path of a symbolic icon, so lines are drawn as filled outlines.
const ICONS: &[(&str, &str)] = &[
    (
        "nyx-weather-radar-symbolic",
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="#000"><path fill-rule="evenodd" d="M2.2 12a9.8 9.8 0 1 1 19.6 0a9.8 9.8 0 1 1 -19.6 0zM3.8 12a8.2 8.2 0 1 0 16.4 0a8.2 8.2 0 1 0 -16.4 0z"/><path fill-rule="evenodd" d="M6.2 12a5.8 5.8 0 1 1 11.6 0a5.8 5.8 0 1 1 -11.6 0zM7.8 12a4.2 4.2 0 1 0 8.4 0a4.2 4.2 0 1 0 -8.4 0z"/><path d="M10.4 12a1.6 1.6 0 1 1 3.2 0a1.6 1.6 0 1 1 -3.2 0z"/><path d="M11.43 11.43L17.83 5.03L18.97 6.17L12.57 12.57z"/></svg>"##,
    ),
    (
        "nyx-weather-leaf-symbolic",
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="#000"><path fill-rule="evenodd" d="M4.4 19.6C4.4 9.8 9.6 3.6 20.4 3.6C20.4 14.4 14.2 19.6 4.4 19.6zM6.05 17.95C6.5 10.7 10.5 5.75 18.75 5.25C18.25 13.5 13.3 17.5 6.05 17.95z"/><path d="M3.43 19.43L13.43 9.43L14.57 10.57L4.57 20.57z"/></svg>"##,
    ),
    (
        "nyx-weather-drop-symbolic",
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="#000"><path fill-rule="evenodd" d="M12 2.4C12 2.4 5.2 9.4 5.2 14.4a6.8 6.8 0 0 0 13.6 0C18.8 9.4 12 2.4 12 2.4zM12 4.75C10.1 6.9 6.8 11.3 6.8 14.4a5.2 5.2 0 0 0 10.4 0C17.2 11.3 13.9 6.9 12 4.75z"/></svg>"##,
    ),
    (
        "nyx-weather-gauge-symbolic",
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="#000"><path d="M4.21 17.5A9 9 0 1 1 19.79 17.5L18.41 16.7A7.4 7.4 0 1 0 5.59 16.7z"/><path d="M11.37 13.01L15.37 8.01L16.63 9.01L12.63 14.01z"/><path d="M10.4 13.5a1.6 1.6 0 1 1 3.2 0a1.6 1.6 0 1 1 -3.2 0z"/></svg>"##,
    ),
    (
        "nyx-weather-umbrella-symbolic",
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="#000"><path d="M3 12a9 9 0 0 1 18 0z"/><path d="M11.2 12h1.6v6.5a2.6 2.6 0 0 1 -5.2 0h1.6a1 1 0 0 0 2 0z"/></svg>"##,
    ),
    (
        "nyx-weather-graph-symbolic",
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="#000"><path d="M3.63 17.57L9.13 11.57L7.87 10.43L2.37 16.43z"/><path d="M7.98 11.67L12.48 15.17L13.52 13.83L9.02 10.33z"/><path d="M13.62 15.08L21.62 6.58L20.38 5.42L12.38 13.92z"/><path d="M2.15 17a0.85 0.85 0 1 0 1.7 0a0.85 0.85 0 1 0 -1.7 0z"/><path d="M7.65 11a0.85 0.85 0 1 0 1.7 0a0.85 0.85 0 1 0 -1.7 0z"/><path d="M12.15 14.5a0.85 0.85 0 1 0 1.7 0a0.85 0.85 0 1 0 -1.7 0z"/><path d="M20.15 6a0.85 0.85 0 1 0 1.7 0a0.85 0.85 0 1 0 -1.7 0z"/><path d="M3 20.2h18v1.6h-18z"/></svg>"##,
    ),
];

fn install_icons() {
    let dir = nyx_core::paths::cache_dir().join("icons");
    for (name, svg) in ICONS {
        let file = dir.join(format!("{name}.svg"));
        if std::fs::read_to_string(&file).ok().as_deref() != Some(*svg) {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&file, svg);
        }
    }
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(&dir);
    }
}

/// Install the stylesheet and the current palette, and follow Omarchy live.
pub fn install() {
    install_icons();
    nyx_theme::use_design_2();
    nyx_theme::install(Style {
        window_class: "weather-window",
        base_skip: &[],
        css: include_str!("style.css"),
        no_glow_css: "",
        extra_colors: Some(extra_colors),
        prefs: || {
            let p = prefs::get();
            ThemePrefs {
                mode: p.mode,
                theme: p.theme,
                glow: p.glow,
                reduce_motion: p.reduce_motion,
            }
        },
        set_theme: Some(|mode, theme| {
            prefs::update(|p| {
                p.mode = mode;
                p.theme = theme;
            })
        }),
    });
}
