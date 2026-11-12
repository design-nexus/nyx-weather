//! The main Weather window: the header (the app and the active location, the
//! sidebar toggle; the location search in the middle; when the forecast was
//! updated, refresh, floating and settings), the collapsible sidebar (Forecast,
//! Maps & Radar, Environment, and Saved Locations) and a crossfading section stack.

use crate::sections::{self, Section};
use crate::widgets;
use crate::{backend, cmd, prefs, settings_dialog, theme};
use gtk::prelude::*;
use gtk::{gdk, glib};
use nyx_ui::chrome;
use nyx_weather_core::{ForecastSnapshot, Location};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+F", "Search or add a location"),
    ("Ctrl+R / F5", "Refresh active forecast"),
    ("Ctrl+B", "Collapse or expand sidebar"),
    ("Ctrl+M", "Toggle floating / tiled window"),
    ("Ctrl+1…6", "Switch section in sidebar"),
    ("Alt+1…9", "Switch saved location"),
    ("Alt+↑ / Alt+↓", "Move the active location up or down"),
    ("Ctrl+Shift+P", "Command palette"),
    ("Ctrl+,", "Settings"),
    ("F1 / ?", "Keyboard shortcuts"),
    ("Ctrl+Q / Ctrl+W", "Close window"),
];

struct Ui {
    window: gtk::ApplicationWindow,
    stack: gtk::Stack,
    nav: gtk::Box,
    nav_items: HashMap<String, gtk::Button>,
    locations_box: gtk::Box,
    search: gtk::SearchEntry,
    search_results_panel: gtk::Box,
    search_results_list: gtk::Box,
    header: chrome::AppHeader,
    /// When the forecast was updated (or that it's refreshing), at the header's end.
    status_readout: gtk::Label,
    overlay: gtk::Overlay,
    sections: Vec<Section>,
    current_section: String,
    active_snapshot: Option<ForecastSnapshot>,
    stale_fallback: bool,
    last_error: Option<String>,
    refresh_source: Option<glib::SourceId>,
}

thread_local! {
    static UI: RefCell<Option<Rc<RefCell<Ui>>>> = const { RefCell::new(None) };
    static NARROW: Cell<bool> = const { Cell::new(false) };
    static SEARCH_SEQ: Cell<u64> = const { Cell::new(0) };
    static REFRESH_SEQ: Cell<u64> = const { Cell::new(0) };
    /// The locations the search strip shows, so Enter can open the first.
    static SEARCH_RESULTS: RefCell<Vec<Location>> = const { RefCell::new(Vec::new()) };
}

/// Key caps for a SHORTCUTS entry: "Ctrl+Q / Ctrl+W" is two alternatives.
pub fn shortcut_caps(keys: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for (i, alt) in keys.split(" / ").enumerate() {
        if i > 0 {
            out.push("/");
        }
        out.extend(alt.split('+').map(str::trim).filter(|k| !k.is_empty()));
    }
    out
}

/// True when the window is narrow (floating, or under 980 px).
pub fn is_narrow() -> bool {
    NARROW.with(|n| n.get())
}

fn ui() -> Option<Rc<RefCell<Ui>>> {
    UI.with(|u| u.borrow().clone())
}

pub fn window() -> Option<gtk::ApplicationWindow> {
    ui().map(|u| u.borrow().window.clone())
}

pub fn present(app: &gtk::Application, section: Option<&str>, location_arg: Option<&str>) {
    if let Some(u) = ui() {
        let w = u.borrow().window.clone();
        if let Some(loc_q) = location_arg {
            handle_location_arg(loc_q);
        }
        if let Some(sec) = section {
            navigate(sec);
        }
        w.present();
        return;
    }

    // Ensure Hyprland rule matches user's saved preference BEFORE creating/mapping the window
    prefs::sync_hypr_rule_on_startup(prefs::get().float_window);

    theme::install();
    build(app);

    let start = section
        .map(String::from)
        .unwrap_or_else(|| nyx_theme::start_page("overview", &prefs::get().last_section));
    navigate(&start);

    if let Some(loc_q) = location_arg {
        handle_location_arg(loc_q);
    } else {
        refresh_active(false);
    }
    restart_refresh_timer();

    // Developer aid: WEATHER_SNAPSHOT=/path.png renders the window to a PNG
    // (invisibly) and quits, so layouts can be checked without a visible window.
    if let Some(out) = std::env::var_os("WEATHER_SNAPSHOT") {
        snapshot_and_quit(app, std::path::PathBuf::from(out));
        return;
    }
    if let Some(u) = ui() {
        u.borrow().window.present();
    }
    let want_float = prefs::get().float_window;
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
        std::thread::spawn(move || {
            prefs::ensure_live_window_state(want_float);
        });
    });
    if prefs::take_broken() {
        toast("Your settings file couldn't be read, so defaults are in use.");
    }
}

fn nav_button(icon: &str, title: &str, tooltip: &str) -> (gtk::Button, gtk::Label) {
    let button = gtk::Button::new();
    button.add_css_class("nav-item");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let img = gtk::Image::from_icon_name(icon);
    let color_class = match icon {
        "weather-clear-symbolic" => "cond-sun",
        "view-continuous-symbolic" => "cond-rain",
        "x-office-calendar-symbolic" => "cond-warm",
        "find-location-symbolic" => "cond-wind",
        "weather-fog-symbolic" => "cond-uv",
        "weather-clear-night-symbolic" => "cond-cold",
        _ => "",
    };
    if !color_class.is_empty() {
        img.add_css_class(color_class);
    }
    content.append(&img);
    let l = widgets::label(title, "nav-label");
    l.set_hexpand(true);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&l);
    button.set_child(Some(&content));
    button.set_tooltip_text(Some(tooltip));
    (button, l)
}

fn build(app: &gtk::Application) {
    let p = prefs::get();
    let (def_w, def_h) = if p.float_window {
        (460, 720)
    } else {
        (1280, 800)
    };
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Weather")
        .default_width(def_w)
        .default_height(def_h)
        .build();
    window.add_css_class("weather-window");
    window.set_size_request(360, 480);
    // No client-side titlebar: Hyprland manages windows.
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    nyx_ui::keys::install(
        &window,
        nyx_ui::keys::Common {
            settings: Some(settings_dialog::open),
            close: true,
            ..Default::default()
        },
    );
    window.set_icon_name(Some(crate::APP_ID));

    let sections = sections::all();

    // ----- Sidebar (exact Music / STYLE.md construction) -----
    let nav = gtk::Box::new(gtk::Orientation::Vertical, 0);
    nav.add_css_class("settings-navigation");
    nav.set_hexpand(false);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.set_hexpand(false);
    let mut nav_items = HashMap::new();
    let mut last_group = "";
    for s in sections.iter().filter(|s| s.nav) {
        if s.group != last_group {
            let g = widgets::label(&s.group.to_uppercase(), "nav-group");
            g.add_css_class("compact-hide");
            if last_group.is_empty() {
                g.add_css_class("first");
            }
            list.append(&g);
            last_group = s.group;
        }
        let (button, label) = nav_button(s.icon, s.title, s.description);
        label.add_css_class("compact-hide");
        let id = s.id;
        button.connect_clicked(move |_| navigate(id));
        list.append(&button);
        nav_items.insert(s.id.to_string(), button);
    }

    // Saved Locations group in the sidebar
    let loc_group = widgets::label("SAVED LOCATIONS", "nav-group");
    loc_group.add_css_class("compact-hide");
    list.append(&loc_group);

    let locations_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    locations_box.set_hexpand(false);
    list.append(&locations_box);

    let (add_loc_btn, add_loc_lbl) = nav_button(
        "list-add-symbolic",
        "Add location",
        "Search for a city to add (Ctrl+F)",
    );
    add_loc_lbl.add_css_class("compact-hide");
    add_loc_btn.add_css_class("nav-add");
    add_loc_btn.connect_clicked(|_| show_search());
    list.append(&add_loc_btn);

    let nav_scroll = gtk::ScrolledWindow::builder()
        .hexpand(false)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(&list)
        .build();

    nav.append(&nav_scroll);

    // ----- Content column -----
    let stack = gtk::Stack::new();
    stack.add_css_class("settings-content");
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_hhomogeneous(false);
    stack.set_vhomogeneous(false);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(if p.reduce_motion { 0 } else { 160 });

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search city, state, or postal code  (Ctrl+F)"));
    search.add_css_class("bar-search");
    search.set_width_chars(SEARCH_WIDE);
    search.update_property(&[gtk::accessible::Property::Label("Search or add a location")]);

    let search_results_panel = widgets::vbox(6);
    search_results_panel.add_css_class("search-results-panel");
    search_results_panel.set_visible(false);
    let search_results_list = widgets::vbox(2);
    search_results_panel.append(&search_results_list);

    search.connect_search_changed(|entry| {
        on_search_changed(entry.text().to_string());
    });
    search.connect_activate(|_| {
        if let Some(first) = SEARCH_RESULTS.with(|r| r.borrow().first().cloned()) {
            open_search_result(first);
        }
    });
    {
        let panel = search_results_panel.clone();
        search.connect_stop_search(move |e| {
            e.set_text("");
            panel.set_visible(false);
        });
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("settings-content");
    content.set_hexpand(true);
    content.append(&search_results_panel);
    content.append(&stack);

    // ----- Header -----
    // The app and the active location, the sidebar toggle; the location search in
    // the middle; when the forecast was updated, refresh, floating and Settings.
    let header = chrome::app_header(&window, "weather-few-clouds-symbolic", "Weather");
    let collapse = widgets::bar_button("sidebar-show-symbolic", "Collapse the sidebar (Ctrl+B)");
    collapse.connect_clicked(|_| toggle_sidebar());
    header.start.append(&collapse);
    header.center.append(&search);
    header.end.set_spacing(4);
    let status_readout = widgets::label("Loading…", "header-status");
    status_readout.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status_readout.set_margin_end(4);
    header.end.append(&status_readout);
    let refresh = widgets::bar_button("view-refresh-symbolic", "Refresh the forecast (Ctrl+R)");
    refresh.connect_clicked(|_| refresh_active(true));
    header.end.append(&refresh);
    let float = widgets::bar_button("view-restore-symbolic", "Floating or tiled window (Ctrl+M)");
    float.connect_clicked(|_| toggle_floating_window());
    header.end.append(&float);
    header.end.append(&chrome::settings_button(settings_dialog::open));

    let frame = nyx_ui::frame::AppFrame::new(&header.root);
    frame.set_sidebar(&nav);
    frame.set_page(&content);
    let overlay = frame.install(&window);

    install_keys(&window, &search);

    // Responsive sidebar sizing (collapses below 980px or when sidebar_collapsed is true)
    let apply_width = {
        let nav = nav.clone();
        move |w: &gtk::ApplicationWindow| {
            let width = if w.width() > 0 {
                w.width()
            } else {
                w.default_width()
            };
            let narrow = width > 0 && width < 980;
            settings_dialog::fit(w);
            if narrow == NARROW.with(|n| n.get()) && nav.has_css_class("sized") {
                return;
            }
            nav.add_css_class("sized");
            set_narrow(narrow);
            apply_compact(&nav, narrow || prefs::get().sidebar_collapsed);
        }
    };
    let aw = apply_width.clone();
    window.connect_default_width_notify(move |w| aw(w));
    let aw = apply_width.clone();
    window.connect_realize(move |w| aw(w));

    let probe = gtk::DrawingArea::new();
    probe.set_can_target(false);
    probe.set_can_focus(false);
    overlay.add_overlay(&probe);
    overlay.set_measure_overlay(&probe, false);
    {
        let (aw, w2) = (apply_width.clone(), window.clone());
        probe.connect_resize(move |_, _, _| {
            let (aw, w2) = (aw.clone(), w2.clone());
            glib::idle_add_local_once(move || aw(&w2));
        });
    }

    // Coming back to the window (after suspend, or a while away) catches up: past hours
    // and days drop off, and a stale forecast is fetched again.
    window.connect_is_active_notify(|w| {
        if w.is_active() {
            catch_up();
        }
    });

    // Theme changes restyle through CSS, and each chart and map redraws itself
    // (`theme::subscribe`), so pages aren't rebuilt for them.

    let ui = Ui {
        window,
        stack,
        nav,
        nav_items,
        locations_box,
        search,
        search_results_panel,
        search_results_list,
        header,
        status_readout,
        overlay,
        sections,
        current_section: String::new(),
        active_snapshot: None,
        stale_fallback: false,
        last_error: None,
        refresh_source: None,
    };
    UI.with(|u| *u.borrow_mut() = Some(Rc::new(RefCell::new(ui))));

    rebuild_sidebar_locations();
    backend::prune_forecasts(prefs::get().saved_locations.into_iter().map(|l| l.id).collect());
}

/// Refresh when the forecast is older than the refresh interval (the interval timer
/// doesn't run during suspend), and redraw when the hour has moved on.
fn catch_up() {
    let Some(u) = ui() else { return };
    let Some(fetched) = u.borrow().active_snapshot.as_ref().map(|s| (s.fetched_at, s.current.time.clone())) else {
        return;
    };
    let max_age = prefs::get().refresh_interval_mins.max(5) as i64 * 60;
    if backend::now_epoch() - fetched.0 >= max_age {
        refresh_active(false);
        return;
    }
    let advanced = {
        let mut b = u.borrow_mut();
        let Some(s) = b.active_snapshot.as_mut() else { return };
        s.advance_to(backend::now_epoch());
        s.current.time != fetched.1
    };
    if advanced {
        rerender_current();
    }
}

/// The location search's width in characters, in a full window and a narrow one.
const SEARCH_WIDE: i32 = 30;
const SEARCH_NARROW: i32 = 10;

/// Narrow windows (floating, or a half-screen tile) get a shorter search and no
/// location or update time in the header, so it fits at 460 px.
fn set_narrow(narrow: bool) {
    let changed = NARROW.with(|n| n.replace(narrow)) != narrow;
    let Some(u) = ui() else { return };
    {
        let b = u.borrow();
        b.search.set_width_chars(if narrow { SEARCH_NARROW } else { SEARCH_WIDE });
        b.status_readout.set_visible(!narrow);
        b.header.subtitle.set_visible(!narrow && !b.header.subtitle.text().is_empty());
    }
    // Pages size some parts for the window when they're built (the 7-day bars).
    if changed {
        rerender_current();
    }
}

fn apply_compact(nav: &gtk::Box, compact: bool) {
    if compact {
        nav.add_css_class("compact");
    } else {
        nav.remove_css_class("compact");
    }
    set_compact_hidden(nav, compact);
}

pub fn toggle_sidebar() {
    prefs::update(|p| p.sidebar_collapsed = !p.sidebar_collapsed);
    let Some(u) = ui() else { return };
    let nav = u.borrow().nav.clone();
    apply_compact(&nav, NARROW.with(|n| n.get()) || prefs::get().sidebar_collapsed);
}

fn set_compact_hidden(root: &gtk::Box, compact: bool) {
    fn walk(w: &gtk::Widget, compact: bool) {
        if w.has_css_class("compact-hide") {
            w.set_visible(!compact);
        }
        if w.has_css_class("compact-show") {
            w.set_visible(compact);
        }
        if w.has_css_class("nav-item")
            && let Some(content) = w.downcast_ref::<gtk::Button>().and_then(|b| b.child())
        {
            content.set_halign(if compact {
                gtk::Align::Center
            } else {
                gtk::Align::Fill
            });
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            walk(&c, compact);
            child = c.next_sibling();
        }
    }
    walk(root.upcast_ref(), compact);
}

pub fn toggle_floating_window() {
    let next = !prefs::get().float_window;
    prefs::update(|p| p.float_window = next);
    apply_floating_size(next);
    toast(if next {
        "Switched to floating window mode (460×720)."
    } else {
        "Switched to tiled window mode."
    });
}

pub fn apply_floating_size(float: bool) {
    let Some(u) = ui() else { return };
    let (w, nav) = {
        let b = u.borrow();
        (b.window.clone(), b.nav.clone())
    };
    if float {
        if settings_dialog::is_open() {
            nyx_ui::settings_card::close();
        }
        set_narrow(true);
        apply_compact(&nav, true);
        w.set_default_size(460, 720);
    } else {
        set_narrow(false);
        apply_compact(&nav, prefs::get().sidebar_collapsed);
        w.set_default_size(1280, 800);
        settings_dialog::fit(&w);
    }
    glib::timeout_add_local_once(Duration::from_millis(45), move || {
        prefs::apply_floating_window(float);
    });
}

pub fn navigate(section: &str) {
    if section == "settings" {
        settings_dialog::open();
        return;
    }
    let Some(u) = ui() else { return };
    let (target_id, title) = {
        let b = u.borrow();
        let found = b
            .sections
            .iter()
            .find(|s| s.id == section)
            .or_else(|| b.sections.first());
        let Some(s) = found else { return };
        (s.id.to_string(), s.title.to_string())
    };

    prefs::update(|p| p.last_section = target_id.clone());
    {
        let mut b = u.borrow_mut();
        b.current_section = target_id.clone();
        for (id, btn) in &b.nav_items {
            if id == &target_id {
                btn.add_css_class("active");
            } else {
                btn.remove_css_class("active");
            }
        }
        chrome::set_title(&b.window, "Weather", &title);
    }
    rerender_current();
}

pub fn rebuild_sidebar_locations() {
    let Some(u) = ui() else { return };
    let (loc_box, nav) = {
        let b = u.borrow();
        (b.locations_box.clone(), b.nav.clone())
    };
    while let Some(c) = loc_box.first_child() {
        loc_box.remove(&c);
    }
    let p = prefs::get();
    let units = p.units();
    let active_id = p
        .active_location()
        .map(|l| l.id)
        .unwrap_or_default();

    for (idx, loc) in p.saved_locations.iter().enumerate() {
        let button = gtk::Button::new();
        button.add_css_class("nav-item");
        // The section is the sidebar's selection; the location in use is marked quietly.
        if loc.id == active_id {
            button.add_css_class("current");
        }
        let cached = backend::get_cached(&loc.id);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let loc_icon = if let Some(c) = &cached {
            let img = gtk::Image::from_icon_name(c.current.condition.icon_name(c.current.is_day));
            img.add_css_class(theme::condition_color_class(
                c.current.condition,
                c.current.is_day,
            ));
            img
        } else {
            let img = gtk::Image::from_icon_name("mark-location-symbolic");
            img.add_css_class("cond-rain");
            img
        };
        content.append(&loc_icon);

        let name_lbl = widgets::label(&loc.name, "nav-label");
        name_lbl.add_css_class("compact-hide");
        name_lbl.set_hexpand(true);
        name_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.append(&name_lbl);

        if let Some(c) = cached {
            let badge_txt = units.temperature.format(c.current.temperature_c);
            let badge = widgets::label(&badge_txt, "nav-loc-badge");
            badge.add_css_class("compact-hide");
            content.append(&badge);
        }

        button.set_child(Some(&content));
        button.set_tooltip_text(Some(&format!(
            "{} (Alt+{})",
            loc.display_name(),
            idx + 1
        )));
        let id = loc.id.clone();
        button.connect_clicked(move |_| select_location(&id));
        let menu_click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let (id, b2) = (loc.id.clone(), button.clone());
        menu_click.connect_pressed(move |_, _, _, _| location_menu(&b2, &id));
        button.add_controller(menu_click);
        loc_box.append(&button);
    }

    let compact = NARROW.with(|n| n.get()) || p.sidebar_collapsed;
    set_compact_hidden(&nav, compact);
}

/// The right-click menu of a saved location: rename, move, remove.
fn location_menu(anchor: &gtk::Button, id: &str) {
    let menu = gtk::gio::Menu::new();
    menu.append(Some("Rename…"), Some(&format!("loc.rename('{id}')")));
    menu.append(Some("Move up"), Some(&format!("loc.move-up('{id}')")));
    menu.append(Some("Move down"), Some(&format!("loc.move-down('{id}')")));
    menu.append(Some("Remove"), Some(&format!("loc.remove('{id}')")));
    let group = gtk::gio::SimpleActionGroup::new();
    let add = |name: &str, f: fn(&str)| {
        let a = gtk::gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
        a.connect_activate(move |_, v| {
            if let Some(id) = v.and_then(|v| v.get::<String>()) {
                f(&id);
            }
        });
        group.add_action(&a);
    };
    add("rename", rename_location);
    add("move-up", |id| move_location(id, -1));
    add("move-down", |id| move_location(id, 1));
    add("remove", remove_location_with_undo);
    let pop = gtk::PopoverMenu::from_model(Some(&menu));
    pop.insert_action_group("loc", Some(&group));
    pop.set_parent(anchor);
    pop.set_has_arrow(false);
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
}

fn rename_location(id: &str) {
    let Some(loc) = prefs::get().saved_locations.into_iter().find(|l| l.id == id) else { return };
    let id = id.to_string();
    widgets::ask_text("Rename location", "The name shown in the sidebar.", &loc.name, "Rename", move |name| {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        prefs::update(|p| {
            if let Some(l) = p.saved_locations.iter_mut().find(|l| l.id == id) {
                l.name = name.clone();
            }
        });
        rebuild_sidebar_locations();
        refresh_active(false);
    });
}

/// Move a saved location `by` places in the list (Alt+Up/Down for the active one).
pub fn move_location(id: &str, by: i32) {
    let mut moved = false;
    prefs::update(|p| {
        let Some(i) = p.saved_locations.iter().position(|l| l.id == id) else { return };
        let j = i as i32 + by;
        if j >= 0 && (j as usize) < p.saved_locations.len() {
            p.saved_locations.swap(i, j as usize);
            moved = true;
        }
    });
    if moved {
        rebuild_sidebar_locations();
    }
}

pub fn show_search() {
    let Some(u) = ui() else { return };
    let search = u.borrow().search.clone();
    search.grab_focus();
}

fn on_search_changed(text: String) {
    let Some(u) = ui() else { return };
    let (panel, list) = {
        let b = u.borrow();
        (b.search_results_panel.clone(), b.search_results_list.clone())
    };
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
    let q = text.trim().to_string();
    if q.len() < 2 {
        panel.set_visible(false);
        return;
    }

    panel.set_visible(true);
    list.append(&widgets::label("Searching Open-Meteo geocoding…", "dim"));

    let seq = SEARCH_SEQ.with(|s| {
        let next = s.get() + 1;
        s.set(next);
        next
    });

    let cfg = prefs::get().open_meteo_config();
    glib::timeout_add_local_once(Duration::from_millis(240), move || {
        if SEARCH_SEQ.with(|s| s.get()) != seq {
            return;
        }
        cmd::background(
            move || backend::search_locations(&q, &cfg),
            move |res| {
                if SEARCH_SEQ.with(|s| s.get()) != seq {
                    return;
                }
                let Some(u) = ui() else { return };
                let list = u.borrow().search_results_list.clone();
                while let Some(c) = list.first_child() {
                    list.remove(&c);
                }
                match res {
                    Ok(locs) if locs.is_empty() => {
                        SEARCH_RESULTS.with(|r| r.borrow_mut().clear());
                        list.append(&widgets::label("No matching locations found.", "dim"));
                    }
                    Ok(locs) => {
                        SEARCH_RESULTS.with(|r| *r.borrow_mut() = locs.clone());
                        for loc in locs {
                            let row = widgets::hbox(8);
                            row.add_css_class("search-result-row");
                            let info = widgets::vbox(2);
                            info.set_hexpand(true);
                            info.append(&widgets::label(&loc.name, ""));
                            let sub = loc.subtitle();
                            if !sub.is_empty() {
                                info.append(&widgets::label(&sub, "dim"));
                            }
                            row.append(&info);

                            let coords = widgets::label(
                                &format!("{:.2}°, {:.2}°", loc.latitude, loc.longitude),
                                "dim",
                            );
                            row.append(&coords);

                            let save_btn = widgets::icon_button("list-add-symbolic", "Save and open (Enter for the first)");
                            let loc_clone = loc.clone();
                            save_btn.connect_clicked(move |_| open_search_result(loc_clone.clone()));
                            row.append(&save_btn);
                            list.append(&row);
                        }
                    }
                    Err(err) => {
                        SEARCH_RESULTS.with(|r| r.borrow_mut().clear());
                        list.append(&widgets::label(&err.to_string(), "dim"));
                    }
                }
            },
        );
    });
}

fn open_search_result(loc: Location) {
    save_and_select_location(loc);
    SEARCH_RESULTS.with(|r| r.borrow_mut().clear());
    if let Some(u) = ui() {
        let b = u.borrow();
        b.search.set_text("");
        b.search_results_panel.set_visible(false);
    }
}

pub fn select_location(location_id: &str) {
    prefs::update(|p| p.active_location_id = location_id.to_string());
    rebuild_sidebar_locations();
    refresh_active(false);
}

pub fn save_and_select_location(loc: Location) {
    let id = loc.id.clone();
    let name = loc.name.clone();
    prefs::update(|p| {
        if !p.saved_locations.iter().any(|l| l.id == loc.id) {
            p.saved_locations.push(loc);
        }
        p.active_location_id = id.clone();
    });
    rebuild_sidebar_locations();
    refresh_active(false);
    toast(&format!("Saved {name}"));
}

pub fn remove_location_with_undo(location_id: &str) {
    let p = prefs::get();
    if p.saved_locations.len() <= 1 {
        toast("At least one saved location is required.");
        return;
    }
    let Some(idx) = p.saved_locations.iter().position(|l| l.id == location_id) else {
        return;
    };
    let removed = p.saved_locations[idx].clone();
    prefs::update(|p| {
        p.saved_locations.retain(|l| l.id != location_id);
        if p.active_location_id == location_id
            && let Some(first) = p.saved_locations.first()
        {
            p.active_location_id = first.id.clone();
        }
    });
    rebuild_sidebar_locations();
    refresh_active(false);

    let msg = format!("Removed {}", removed.name);
    toast_action(&msg, "Undo", move || {
        let restored = removed.clone();
        prefs::update(|p| {
            let insert_at = idx.min(p.saved_locations.len());
            p.saved_locations.insert(insert_at, restored.clone());
            p.active_location_id = restored.id;
        });
        rebuild_sidebar_locations();
        refresh_active(false);
    });
}

fn handle_location_arg(query: &str) {
    let q = query.trim();
    if q.is_empty() {
        refresh_active(false);
        return;
    }
    let p = prefs::get();
    if let Some(found) = p
        .saved_locations
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(q) || l.id == q)
    {
        select_location(&found.id);
        return;
    }
    let q_owned = q.to_string();
    let cfg = p.open_meteo_config();
    cmd::background(
        move || backend::search_locations(&q_owned, &cfg),
        move |res| {
            if let Ok(mut locs) = res
                && let Some(first) = locs.drain(..).next()
            {
                save_and_select_location(first);
            } else {
                refresh_active(false);
            }
        },
    );
}

pub fn restart_refresh_timer() {
    let Some(u) = ui() else { return };
    if let Some(old) = u.borrow_mut().refresh_source.take() {
        old.remove();
    }
    let mins = prefs::get().refresh_interval_mins.max(5) as u64;
    let id = glib::timeout_add_local(Duration::from_secs(mins * 60), || {
        refresh_active(false);
        glib::ControlFlow::Continue
    });
    u.borrow_mut().refresh_source = Some(id);
}

pub fn refresh_active(force: bool) {
    let p = prefs::get();
    let Some(loc) = p.active_location() else { return };
    backend::prewarm_radar(&loc, window().map_or(1, |w| w.scale_factor()));

    // Show cached forecast immediately while worker checks/fetches
    if let Some(cached) = backend::get_cached(&loc.id)
        && let Some(u) = ui()
    {
        {
            let mut b = u.borrow_mut();
            b.active_snapshot = Some(cached);
            b.stale_fallback = false;
            b.last_error = None;
        }
        rerender_current();
    }

    if let Some(u) = ui() {
        u.borrow().status_readout.set_text("Refreshing…");
    }

    let cfg = p.open_meteo_config();
    let max_age = (p.refresh_interval_mins.max(5) as i64) * 60;
    let seq = REFRESH_SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let loc_id = loc.id.clone();
    cmd::background(
        move || backend::load_or_fetch(&loc, &cfg, max_age, force),
        move |outcome| {
            // A newer refresh (another location, or a forced one) has taken over.
            if REFRESH_SEQ.with(|s| s.get()) != seq
                || prefs::get().active_location().map(|l| l.id).as_deref() != Some(loc_id.as_str())
            {
                return;
            }
            let Some(u) = ui() else { return };
            if let Some(snap) = &outcome.snapshot {
                crate::notify::new_alerts(snap);
            }
            {
                let mut b = u.borrow_mut();
                if outcome.snapshot.is_some() {
                    b.active_snapshot = outcome.snapshot;
                }
                b.stale_fallback = outcome.stale_fallback;
                b.last_error = outcome.error.map(|e| e.to_string());
            }
            rebuild_sidebar_locations();
            rerender_current();
            if force
                && let Some(err) = u.borrow().last_error.clone()
            {
                toast(&err);
            }
        },
    );
}

pub fn rerender_current() {
    let Some(u) = ui() else { return };
    let (stack, sec_id, snap, stale, err_str, readout_lbl, subtitle, build_fn) = {
        let b = u.borrow();
        let sec_id = if b.current_section.is_empty() {
            "overview".to_string()
        } else {
            b.current_section.clone()
        };
        let build_fn = b
            .sections
            .iter()
            .find(|s| s.id == sec_id)
            .or_else(|| b.sections.first())
            .map(|s| s.build);
        (
            b.stack.clone(),
            sec_id,
            b.active_snapshot.clone(),
            b.stale_fallback,
            b.last_error.clone(),
            b.status_readout.clone(),
            b.header.subtitle.clone(),
            build_fn,
        )
    };
    let Some(build_fn) = build_fn else { return };

    // The header: the location after the app's name, and when the forecast is from.
    if let Some(loc) = prefs::get().active_location() {
        subtitle.set_text(&loc.name);
        subtitle.set_visible(!is_narrow());
        subtitle.set_tooltip_text(Some(&loc.display_name()));
    }
    if let Some(s) = &snap {
        let units = prefs::get().units();
        let age = backend::human_age(s.fetched_at);
        let state_str = match (&err_str, stale) {
            (Some(e), true) => format!("{e} · cached {age}"),
            _ => format!("Updated {age}"),
        };
        readout_lbl.set_text(&state_str);
        readout_lbl.set_tooltip_text(Some(&format!(
            "{} · {} {} · {state_str} · {} ({})",
            s.location.name,
            units.temperature.format_with_unit(s.current.temperature_c),
            s.current.condition.label(),
            s.attribution.provider_name,
            s.attribution.license_name,
        )));
    } else if let Some(e) = &err_str {
        readout_lbl.set_text(e);
        readout_lbl.set_tooltip_text(Some(e));
    }

    let snap = snap.map(|mut s| {
        s.advance_to(backend::now_epoch());
        s
    });
    let page = widgets::page(&sec_id);
    build_fn(&page, snap.as_ref(), stale, err_str.as_deref());
    // The data's licence asks for credit wherever it's shown; this is its place now
    // that there's no status bar.
    if let Some(s) = &snap {
        page.body.append(&attribution(&s.attribution));
    }

    if let Some(old) = stack.child_by_name(&sec_id) {
        stack.remove(&old);
    }
    stack.add_named(&page.root, Some(&sec_id));
    stack.set_visible_child_name(&sec_id);

    // Pages are rebuilt when shown, so drop the others once the crossfade is over
    // (a hidden Radar page would otherwise keep its timers).
    let wait = stack.transition_duration() as u64 + 50;
    let keep = page.root.clone();
    glib::timeout_add_local_once(Duration::from_millis(wait), move || {
        let mut child = stack.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            if c != *keep.upcast_ref::<gtk::Widget>() && stack.visible_child().as_ref() != Some(&c) {
                stack.remove(&c);
            }
        }
    });
}

/// "Weather data by Open-Meteo.com · CC BY 4.0", quiet at the end of a page, the
/// provider linked to its site.
fn attribution(a: &nyx_weather_core::ProviderAttribution) -> gtk::Label {
    let text = glib::markup_escape_text(&a.attribution_text);
    let markup = if a.attribution_url.is_empty() {
        text.to_string()
    } else {
        format!("<a href=\"{}\">{text}</a>", glib::markup_escape_text(&a.attribution_url))
    };
    let label = gtk::Label::new(None);
    label.set_markup(&format!("{markup} · {}", glib::markup_escape_text(&a.license_name)));
    label.add_css_class("attribution");
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label
}

pub fn overlay() -> Option<gtk::Overlay> {
    ui().map(|u| u.borrow().overlay.clone())
}

pub fn toast(message: &str) {
    show_toast(message, None);
}

pub fn toast_action(message: &str, label: &str, action: impl Fn() + 'static) {
    show_toast(message, Some((label, Box::new(action))));
}

thread_local! {
    static TOAST: RefCell<Option<gtk::Box>> = const { RefCell::new(None) };
}

type ToastAction<'a> = Option<(&'a str, Box<dyn Fn()>)>;

fn show_toast(message: &str, action: ToastAction<'_>) {
    let Some(u) = ui() else {
        eprintln!("nyx-weather: {message}");
        return;
    };
    let overlay = u.borrow().overlay.clone();
    if let Some(old) = TOAST.with(|t| t.borrow_mut().take())
        && old.parent().is_some()
    {
        overlay.remove_overlay(&old);
    }
    let label = gtk::Label::new(Some(message));
    label.set_wrap(true);
    label.set_max_width_chars(70);
    let bx = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bx.add_css_class("toast");
    bx.append(&label);
    let has_action = action.is_some();
    if let Some((text, f)) = action {
        let b = gtk::Button::with_label(text);
        b.add_css_class("flat");
        b.add_css_class("toast-action");
        b.set_valign(gtk::Align::Center);
        let (o, w) = (overlay.clone(), bx.downgrade());
        b.connect_clicked(move |_| {
            f();
            if let Some(w) = w.upgrade()
                && w.parent().is_some()
            {
                o.remove_overlay(&w);
            }
        });
        bx.append(&b);
    }
    bx.set_halign(gtk::Align::Center);
    bx.set_valign(gtk::Align::End);
    overlay.add_overlay(&bx);
    TOAST.with(|t| *t.borrow_mut() = Some(bx.clone()));
    let ms = if has_action { 6000 } else { 3500 };
    glib::timeout_add_local_once(Duration::from_millis(ms), move || {
        if bx.parent().is_some() {
            overlay.remove_overlay(&bx);
        }
    });
}

fn show_shortcuts() {
    let Some(w) = window() else { return };
    let (dialog, body) = widgets::dialog("Keyboard shortcuts", 520);
    dialog.set_transient_for(Some(&w));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("group-list");
    for (keys, action) in SHORTCUTS {
        let parts = shortcut_caps(keys);
        list.append(&widgets::row(
            action,
            "",
            Some(widgets::key_caps(&parts).upcast_ref()),
        ));
    }
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .propagate_natural_height(true)
        .max_content_height(560)
        .child(&list)
        .build();
    body.append(&scroll);

    let actions = widgets::hbox(8);
    actions.set_halign(gtk::Align::End);
    let close = gtk::Button::with_label("Close");
    let d2 = dialog.clone();
    close.connect_clicked(move |_| d2.close());
    actions.append(&close);
    body.append(&actions);
    dialog.present();
}

/// Render the window to `out` once the forecast has had time to load, then quit.
/// `WEATHER_SNAPSHOT_W`/`_H` set the size and `WEATHER_SNAPSHOT_DELAY` the wait (ms).
fn snapshot_and_quit(app: &gtk::Application, out: std::path::PathBuf) {
    let Some(window) = window() else { return };
    window.set_opacity(0.01);
    let env = |k: &str, d: i32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    window.set_default_size(env("WEATHER_SNAPSHOT_W", 1280), env("WEATHER_SNAPSHOT_H", 800));
    window.present();
    let app = app.clone();
    let delay = env("WEATHER_SNAPSHOT_DELAY", 3000) as u64;
    glib::timeout_add_local_once(Duration::from_millis(delay), move || {
        if let Some(child) = window.child() {
            let paintable = gtk::WidgetPaintable::new(Some(&child));
            let (w, h) = (child.width(), child.height());
            let snapshot = gtk::Snapshot::new();
            snapshot.append_color(&gdk::RGBA::BLACK, &gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32));
            paintable.snapshot(&snapshot, w as f64, h as f64);
            if let (Some(node), Some(renderer)) = (snapshot.to_node(), window.renderer()) {
                match renderer.render_texture(node, None).save_to_png(&out) {
                    Ok(()) => println!("snapshot {w}x{h} -> {}", out.display()),
                    Err(e) => eprintln!("snapshot failed: {e}"),
                }
            }
        }
        app.quit();
    });
}

fn install_keys(window: &gtk::ApplicationWindow, search: &gtk::SearchEntry) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let s2 = search.clone();
    let w2 = window.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = mods.contains(gdk::ModifierType::ALT_MASK);
        let typing = gtk::prelude::GtkWindowExt::focus(&w2).is_some_and(|f| {
            f.is::<gtk::Text>()
                || f.ancestor(gtk::Entry::static_type()).is_some()
                || f.is::<gtk::SearchEntry>()
        });
        let done = glib::Propagation::Stop;

        if settings_dialog::is_open() {
            return match key {
                gdk::Key::Escape => {
                    settings_dialog::escape();
                    done
                }
                gdk::Key::q | gdk::Key::w if ctrl => {
                    w2.close();
                    done
                }
                _ => glib::Propagation::Proceed,
            };
        }

        match key {
            gdk::Key::f if ctrl => {
                s2.grab_focus();
                done
            }
            gdk::Key::b if ctrl => {
                toggle_sidebar();
                done
            }
            gdk::Key::m if ctrl => {
                toggle_floating_window();
                done
            }
            gdk::Key::r if ctrl => {
                refresh_active(true);
                done
            }
            gdk::Key::F5 => {
                refresh_active(true);
                done
            }
            gdk::Key::F1 => {
                show_shortcuts();
                done
            }
            gdk::Key::question if !typing => {
                show_shortcuts();
                done
            }
            gdk::Key::Escape if s2.has_focus() || !s2.text().is_empty() => {
                s2.set_text("");
                if let Some(u) = ui() {
                    u.borrow().search_results_panel.set_visible(false);
                }
                gtk::prelude::GtkWindowExt::set_focus(&w2, None::<&gtk::Widget>);
                done
            }
            k if ctrl => {
                let sec_idx = match k {
                    gdk::Key::_1 => Some("overview"),
                    gdk::Key::_2 => Some("hourly"),
                    gdk::Key::_3 => Some("daily"),
                    gdk::Key::_4 => Some("radar"),
                    gdk::Key::_5 => Some("air-quality"),
                    gdk::Key::_6 => Some("astronomy"),
                    _ => None,
                };
                if let Some(sec) = sec_idx {
                    navigate(sec);
                    done
                } else {
                    glib::Propagation::Proceed
                }
            }
            gdk::Key::Up | gdk::Key::Down if alt => {
                if let Some(loc) = prefs::get().active_location() {
                    move_location(&loc.id, if key == gdk::Key::Up { -1 } else { 1 });
                }
                done
            }
            k if alt => {
                let idx = match k {
                    gdk::Key::_1 => Some(0),
                    gdk::Key::_2 => Some(1),
                    gdk::Key::_3 => Some(2),
                    gdk::Key::_4 => Some(3),
                    gdk::Key::_5 => Some(4),
                    gdk::Key::_6 => Some(5),
                    gdk::Key::_7 => Some(6),
                    gdk::Key::_8 => Some(7),
                    gdk::Key::_9 => Some(8),
                    _ => None,
                };
                if let Some(i) = idx {
                    let p = prefs::get();
                    if let Some(loc) = p.saved_locations.get(i) {
                        select_location(&loc.id);
                        return done;
                    }
                }
                glib::Propagation::Proceed
            }
            _ => glib::Propagation::Proceed,
        }
    });
    window.add_controller(keys);
}
