//! Weather — native desktop weather application for Omarchy and the Nyx suite.

mod backend;
mod charts;
mod cmd;
mod control;
mod maprender;
mod notify;
mod paths;
mod prefs;
mod sections;
mod settings_dialog;
mod theme;
mod waybar;
mod widgets;
mod window;

use gtk::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.github.design_nexus.nyx.Weather";

const USAGE: &str = "Usage: nyx-weather [OPTIONS]\n\
\n\
  --toggle            close the window if it's open, otherwise open it\n\
  --location NAME     switch to or search for NAME on open\n\
  --section SECTION   open a section (overview, hourly, daily, radar, air-quality, astronomy, settings)\n\
  --waybar            output Waybar custom-module JSON from the SQLite cache\n\
  --waybar-cached     output Waybar JSON strictly from local cache without network\n\
  --json              output now, the next 12 hours and 5 days as JSON (for widgets and the bar)\n\
  --refresh           refresh all saved locations in the SQLite cache and exit\n";

fn main() -> glib::ExitCode {
    nyx_core::app::init(nyx_core::app::App {
        slug: "nyx-weather",
        name: "Weather",
    });

    let id = std::env::var("WEATHER_APP_ID").unwrap_or_else(|_| APP_ID.to_string());
    nyx_core::control::describe_if_asked(&control::description(&id));
    let mut args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return glib::ExitCode::SUCCESS;
    }

    // Headless CLI modes run without initializing GTK or opening a display connection.
    if args.iter().any(|a| a == "--waybar") || args.get(1).is_some_and(|a| a == "waybar") {
        waybar::print_waybar_json(true);
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--json") {
        waybar::print_summary_json();
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--waybar-cached") {
        waybar::print_waybar_json(false);
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--refresh") {
        let code = waybar::refresh_all_cli();
        return if code == 0 {
            glib::ExitCode::SUCCESS
        } else {
            glib::ExitCode::FAILURE
        };
    }

    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: still single-threaded; nothing else reads the environment yet.
        unsafe { std::env::set_var("GSK_RENDERER", "ngl") };
    }

    // D-Bus starts Weather with --gapplication-service for a call from nyx, Glimpse or
    // Nyx Assistant: no window, and it quits once idle. GLib won't parse the flag for
    // apps that handle their own command line.
    let service = args.iter().any(|a| a == "--gapplication-service");
    args.retain(|a| a != "--gapplication-service");
    let mut flags = gio::ApplicationFlags::HANDLES_COMMAND_LINE;
    if service {
        flags |= gio::ApplicationFlags::IS_SERVICE;
    }
    let app = gtk::Application::builder().application_id(id).flags(flags).build();

    app.connect_startup(move |app| {
        control::serve(app);
        if service {
            app.set_inactivity_timeout(10_000);
        }
    });

    app.connect_command_line(|app, cl| {
        let argv: Vec<String> = cl
            .arguments()
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let has = |flag: &str| argv.iter().any(|a| a == flag);
        let value = |flag: &str| {
            argv.iter()
                .position(|a| a == flag)
                .and_then(|i| argv.get(i + 1))
                .cloned()
        };

        if has("--toggle")
            && let Some(w) = window::window()
            && w.is_visible()
        {
            w.close();
            return glib::ExitCode::SUCCESS;
        }

        let section = value("--section");
        let location = value("--location");
        window::present(app, section.as_deref(), location.as_deref());
        glib::ExitCode::SUCCESS
    });

    app.connect_shutdown(|_| {
        prefs::flush();
    });

    app.run_with_args(&args)
}
