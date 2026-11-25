//! Cutmary: a native GTK4/Libadwaita launcher that installs the official macOS
//! CapCut and runs it on Linux through the Rosemary compatibility runtime.

mod config;
mod drafts;
mod installer;
mod logging;
mod macho;
mod preflight;
mod runner;
mod ui;
mod update;
mod util;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::APP_ID;

fn main() -> glib::ExitCode {
    gio::resources_register_include!("cutmary.gresource").expect("embedded resources are valid");
    glib::set_application_name("Cutmary");

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|app| {
        if let Some(display) = gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_resource_path("/org/cutmary/Cutmary/icons");
            let css = gtk::CssProvider::new();
            css.load_from_resource("/org/cutmary/Cutmary/style.css");
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        gtk::Window::set_default_icon_name(APP_ID);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("win.logs", &["<Control>l"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);
        let quit = gio::ActionEntry::builder("quit").activate(|app: &adw::Application, _, _| app.quit()).build();
        app.add_action_entries([quit]);
    });
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        ui::window::build(app, ui::controller::Controller::new()).present();
    });
    app.run()
}
