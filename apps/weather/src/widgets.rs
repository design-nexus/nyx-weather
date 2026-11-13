//! Nyx-style building blocks: pages, groups and option rows.

pub use nyx_ui::widgets::*;

use gtk::prelude::*;

pub struct Page {
    pub root: gtk::ScrolledWindow,
    pub body: gtk::Box,
}

pub fn page(section: &str) -> Page {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("settings-page");
    body.add_css_class(&format!("page-{section}"));
    body.set_hexpand(true);

    let root = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(&body)
        .vexpand(true)
        .build();
    Page { root, body }
}

impl Page {
    pub fn group(&self, title: &str) -> Group {
        let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wrapper.add_css_class("settings-group");
        wrapper.set_widget_name(title);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if !title.is_empty() {
            let l = gtk::Label::new(Some(&title.to_uppercase()));
            l.add_css_class("group-title");
            l.set_xalign(0.0);
            l.set_hexpand(true);
            head.append(&l);
            wrapper.append(&head);
        }
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list.add_css_class("group-list");
        wrapper.append(&list);
        self.body.append(&wrapper);
        Group { wrapper, list }
    }
}

#[derive(Clone)]
pub struct Group {
    pub wrapper: gtk::Box,
    pub list: gtk::Box,
}

impl Group {
    pub fn add(&self, w: &impl IsA<gtk::Widget>) {
        self.list.append(w);
    }

    pub fn note(&self, text: &str) {
        let l = gtk::Label::new(None);
        l.set_markup(text);
        l.add_css_class("group-note");
        l.set_xalign(0.0);
        l.set_wrap(true);
        self.wrapper
            .insert_child_after(&l, self.wrapper.first_child().as_ref());
    }
}

pub fn slider_row(
    title: &str,
    desc: &str,
    (min, max, step): (f64, f64, f64),
    initial: f64,
    digits: i32,
    suffix: &str,
    on_change: impl Fn(f64) + 'static,
) -> (gtk::Box, gtk::Scale) {
    let bx = hbox(10);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_draw_value(false);
    scale.set_width_request(160);
    scale.set_value(initial);
    let suffix_str = suffix.to_string();
    let val_lbl = label(&format!("{initial:.prec$}{suffix_str}", prec = digits.max(0) as usize), "tnum");
    val_lbl.set_width_chars(6);
    val_lbl.set_xalign(1.0);
    let vl = val_lbl.clone();
    scale.connect_value_changed(move |s| {
        let v = s.value();
        vl.set_text(&format!("{v:.prec$}{suffix_str}", prec = digits.max(0) as usize));
        on_change(v);
    });
    bx.append(&scale);
    bx.append(&val_lbl);
    let r = row(title, desc, Some(bx.upcast_ref()));
    (r, scale)
}
