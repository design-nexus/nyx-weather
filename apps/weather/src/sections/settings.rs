//! Settings page groups for Weather.

use crate::widgets::{self, Page};
use crate::{prefs, settings_dialog, window};
use gtk::glib;
use gtk::prelude::*;
use nyx_weather_core::{DistanceUnit, PrecipitationUnit, PressureUnit, TemperatureUnit, WindSpeedUnit};

fn apply_units(change: impl FnOnce(&mut prefs::Prefs)) {
    // The sidebar's temperature badges use the units too.
    glib::idle_add_local_once(window::rebuild_sidebar_locations);
    prefs::update(change);
    window::rerender_current();
}

pub fn build(page: &Page) {
    let p = prefs::get();

    // ----- Units & forecast -----
    let g = page.group("Units & forecast");
    g.add(&widgets::segmented_row(
        "Temperature",
        "Unit for current, hourly and daily temperatures.",
        widgets::opts(&[("celsius", "°C"), ("fahrenheit", "°F")]),
        p.temperature_unit.as_str(),
        |v| apply_units(|p| p.temperature_unit = TemperatureUnit::from_id(&v)),
    ));
    g.add(&widgets::segmented_row(
        "Wind speed",
        "Unit for wind speed and gusts.",
        widgets::opts(&[("kmh", "km/h"), ("mph", "mph"), ("ms", "m/s"), ("knots", "kn")]),
        p.wind_speed_unit.as_str(),
        |v| apply_units(|p| p.wind_speed_unit = WindSpeedUnit::from_id(&v)),
    ));
    g.add(&widgets::segmented_row(
        "Precipitation",
        "Unit for rain and snow totals.",
        widgets::opts(&[("mm", "mm"), ("inch", "in")]),
        p.precipitation_unit.as_str(),
        |v| apply_units(|p| p.precipitation_unit = PrecipitationUnit::from_id(&v)),
    ));
    g.add(&widgets::segmented_row(
        "Pressure",
        "Unit for air pressure.",
        widgets::opts(&[("hpa", "hPa"), ("inhg", "inHg"), ("mmhg", "mmHg")]),
        p.pressure_unit.as_str(),
        |v| apply_units(|p| p.pressure_unit = PressureUnit::from_id(&v)),
    ));
    g.add(&widgets::segmented_row(
        "Distance",
        "Unit for visibility.",
        widgets::opts(&[("km", "km"), ("mi", "mi")]),
        p.distance_unit.as_str(),
        |v| apply_units(|p| p.distance_unit = DistanceUnit::from_id(&v)),
    ));
    g.add(&widgets::segmented_row(
        "Time format",
        "Clock format for hourly forecasts and sunrise/sunset.",
        widgets::opts(&[("12", "12-hour"), ("24", "24-hour")]),
        if p.use_24h_time { "24" } else { "12" },
        |v| apply_units(|p| p.use_24h_time = v == "24"),
    ));
    g.add(&widgets::segmented_row(
        "Default map overlay",
        "Initial overlay shown on the Radar & Overlays page.",
        widgets::opts(&[("radar", "Radar"), ("satellite", "Satellite"), ("wind", "Wind"), ("temp", "Temp")]),
        &p.default_radar_layer,
        |v| apply_units(|p| p.default_radar_layer = v),
    ));
    g.add(&widgets::segmented_row(
        "Refresh interval",
        "How often forecasts refresh while open or polled by Waybar.",
        widgets::opts(&[("15", "15 min"), ("30", "30 min"), ("60", "60 min")]),
        &p.refresh_interval_mins.to_string(),
        |v| {
            prefs::update(|p| p.refresh_interval_mins = v.parse().unwrap_or(30));
            window::restart_refresh_timer();
        },
    ));
    let (row, _) = widgets::slider_row(
        "Radar opacity",
        "Blend the weather overlay with the map below it.",
        (35.0, 100.0, 5.0),
        p.radar_opacity_pct as f64,
        0,
        "%",
        |value| {
            prefs::update(|p| p.radar_opacity_pct = value.round().clamp(35.0, 100.0) as u8);
            window::rerender_current();
        },
    );
    g.add(&row);
    let (r, _) = widgets::switch_row(
        "US severe-weather alerts",
        "Fetch active watches and warnings from NOAA / National Weather Service for US locations.",
        p.us_noaa_alerts,
        |on| {
            prefs::update(|p| p.us_noaa_alerts = on);
            window::refresh_active(true);
        },
    );
    g.add(&r);
    let (r, _) = widgets::switch_row(
        "Notify severe alerts",
        "Send a desktop notification when a new Severe or Extreme alert is issued for the active location.",
        p.notify_alerts,
        |on| prefs::update(|p| p.notify_alerts = on),
    );
    g.add(&r);

    let (r, _) = widgets::switch_row(
        "Commercial use mode",
        "Open-Meteo's free hosted API is for non-commercial use only (CC BY 4.0). Enable this only with a commercial API key or self-hosted instance.",
        p.commercial_use,
        |on| {
            prefs::update(|p| p.commercial_use = on);
            window::refresh_active(true);
        },
    );
    g.add(&r);

    let api_key_entry = gtk::Entry::new();
    api_key_entry.set_placeholder_text(Some("Optional commercial API key"));
    api_key_entry.set_visibility(false);
    api_key_entry.set_text(&p.open_meteo_api_key);
    api_key_entry.set_width_chars(20);
    api_key_entry.connect_changed(|e| {
        let val = e.text().trim().to_string();
        prefs::update(|p| p.open_meteo_api_key = val);
    });
    g.add(&widgets::row(
        "Open-Meteo API key",
        "With a key, requests go to Open-Meteo's customer servers.",
        Some(api_key_entry.upcast_ref()),
    ));

    for (title, placeholder, current, set) in [
        (
            "Custom forecast endpoint",
            "https://api.open-meteo.com/v1/forecast",
            p.custom_forecast_url.clone(),
            (|p: &mut prefs::Prefs, v: String| p.custom_forecast_url = v) as fn(&mut prefs::Prefs, String),
        ),
        (
            "Custom air-quality endpoint",
            "https://air-quality-api.open-meteo.com/v1/air-quality",
            p.custom_air_quality_url.clone(),
            |p, v| p.custom_air_quality_url = v,
        ),
        (
            "Custom geocoding endpoint",
            "https://geocoding-api.open-meteo.com/v1/search",
            p.custom_geocoding_url.clone(),
            |p, v| p.custom_geocoding_url = v,
        ),
    ] {
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(placeholder));
        entry.set_text(&current);
        entry.set_width_chars(24);
        entry.connect_changed(move |e| {
            let val = e.text().trim().to_string();
            prefs::update(|p| set(p, val));
        });
        g.add(&widgets::row(
            title,
            "Leave blank for Open-Meteo's own server (a self-hosted instance goes here).",
            Some(entry.upcast_ref()),
        ));
    }

    // ----- Locations -----
    let g = page.group("Locations");
    g.note("Saved places appear in the sidebar under SAVED LOCATIONS.");
    let count = p.saved_locations.len();
    for (i, loc) in p.saved_locations.iter().enumerate() {
        let is_active = loc.id == p.active_location_id;
        let controls = widgets::hbox(4);
        if is_active {
            controls.append(&widgets::tag("Active", ""));
        } else {
            let select_btn = widgets::icon_button("object-select-symbolic", "Make active");
            let id = loc.id.clone();
            select_btn.connect_clicked(move |_| {
                window::select_location(&id);
                settings_dialog::open();
            });
            controls.append(&select_btn);
        }
        for (icon, tip, by, enabled) in [
            ("go-up-symbolic", "Move up (Alt+↑ for the active one)", -1, i > 0),
            ("go-down-symbolic", "Move down (Alt+↓ for the active one)", 1, i + 1 < count),
        ] {
            let b = widgets::icon_button(icon, tip);
            b.set_sensitive(enabled);
            let id = loc.id.clone();
            b.connect_clicked(move |_| {
                window::move_location(&id, by);
                settings_dialog::open();
            });
            controls.append(&b);
        }
        if p.saved_locations.len() > 1 {
            let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
            remove_btn.add_css_class("flat");
            remove_btn.set_tooltip_text(Some("Remove location"));
            let id = loc.id.clone();
            remove_btn.connect_clicked(move |_| {
                window::remove_location_with_undo(&id);
                settings_dialog::open();
            });
            controls.append(&remove_btn);
        }
        let desc = glib::markup_escape_text(&loc.subtitle());
        g.add(&widgets::row(&loc.name, &desc, Some(controls.upcast_ref())));
    }
    let add = widgets::icon_button("list-add-symbolic", "Add a location (Ctrl+F)");
    add.connect_clicked(|_| {
        nyx_ui::settings_card::close();
        window::show_search();
    });
    g.add(&widgets::row(
        "Add a location",
        "Search for a city or region to add to your saved places. Right-click a place in the sidebar to rename it.",
        Some(add.upcast_ref()),
    ));

    // ----- Desktop -----
    let g = page.group("Desktop");
    let (r, _) = widgets::switch_row(
        "Open as floating window",
        "Float and center Weather in Hyprland (460×720) instead of tiling it.",
        p.float_window,
        |on| {
            prefs::update(|p| p.float_window = on);
            window::apply_floating_size(on);
        },
    );
    g.add(&r);
    g.note(&format!(
        "Use <tt>{}</tt> in Waybar and <tt>{}</tt> for click-to-open.",
        glib::markup_escape_text("nyx-weather --waybar"),
        glib::markup_escape_text("nyx-weather --toggle"),
    ));
}
