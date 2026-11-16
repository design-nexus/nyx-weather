//! Settings as a card over the window: the groups listed on the left, the
//! chosen group's options on the right, and a search across all of them.

use crate::sections::settings;
use crate::{prefs, widgets, window};
use gtk::prelude::*;
use nyx_ui::settings_card::{self, About, Spec, Standard};

pub use nyx_ui::settings_card::{escape, fit, is_open};

const GROUPS: &[(&str, &str, &str)] = &[
    ("Units & forecast", "weather-clear-symbolic", "Temperature, wind, refresh, API"),
    ("Locations", "mark-location-symbolic", "Saved places and active city"),
    ("Desktop", "video-display-symbolic", "Floating window, Waybar"),
];

type Shortcut = (&'static [&'static str], &'static str);

/// Every shortcut as key caps, for the card's Keyboard group.
fn shortcuts() -> &'static [Shortcut] {
    static ALL: std::sync::OnceLock<Vec<Shortcut>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| window::SHORTCUTS.iter().map(|(keys, what)| (&*window::shortcut_caps(keys).leak(), *what)).collect())
}

fn spec() -> Spec {
    Spec {
        groups: GROUPS,
        search_text: None,
        standard: Some(Standard {
            app: "weather",
            about: About {
                name: "Weather",
                icon: "io.github.design_nexus.nyx.Weather",
                version: env!("CARGO_PKG_VERSION"),
                description: "Current conditions, hourly and 7-day forecasts, radar and air quality.",
                licence: "MIT",
                website: "https://github.com/design-nexus/nyx-weather",
                issues: "https://github.com/design-nexus/nyx-weather/issues",
                credits: &[("Open-Meteo", "forecasts"), ("RainViewer and IEM", "radar")],
            },
            shortcuts: shortcuts(),
            reduce_motion: || prefs::get().reduce_motion,
            set_reduce_motion: |on| prefs::update(|p| p.reduce_motion = on),
        }),
    }
}

pub fn open() {
    let (Some(overlay), Some(window)) = (window::overlay(), window::window()) else {
        return;
    };
    let page = widgets::page("settings");
    settings::build(&page);
    settings_card::open_page(&overlay, &window, spec(), page.root.upcast_ref(), &page.body);
}
