//! The main window: header bar with a centered Home/Settings view switcher.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use super::controller::{Change, Controller};
use crate::config::{APP_ID, REPOSITORY, VERSION};

pub fn build(app: &adw::Application, ctl: Rc<Controller>) -> adw::ApplicationWindow {
    ctl.apply_theme();

    let stack = adw::ViewStack::new();
    stack.add_titled_with_icon(&super::home::build(&ctl), Some("home"), "Home", "cutmary-home-symbolic");
    stack.add_titled_with_icon(&super::settings::build(&ctl), Some("settings"), "Settings", "emblem-system-symbolic");

    let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
    let menu = gio::Menu::new();
    menu.append(Some("Show _Logs"), Some("win.logs"));
    menu.append(Some("_About Cutmary"), Some("win.about"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Main Menu")
        .primary(true)
        .build();
    let header = adw::HeaderBar::builder().title_widget(&switcher).build();
    header.pack_end(&menu_button);

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&stack));
    // Non-blocking notice when host libraries Rosemary needs are missing.
    let banner = adw::Banner::builder().button_label("Details").revealed(false).build();
    banner.connect_button_clicked({
        let stack = stack.clone();
        move |_| stack.set_visible_child_name("settings")
    });
    ctl.subscribe({
        let (banner, ctl) = (banner.clone(), Rc::downgrade(&ctl));
        move |change| {
            let Some(ctl) = ctl.upgrade() else { return };
            if change != Change::Preflight {
                return;
            }
            match ctl.preflight().filter(|r| !r.all_ok()) {
                Some(report) => {
                    let names: Vec<&str> = report.missing().iter().map(|c| c.name).collect();
                    banner.set_title(&gtk::glib::markup_escape_text(&format!(
                        "Missing runtime libraries: {} — CapCut may fail to start or import media",
                        names.join(", ")
                    )));
                    banner.set_revealed(true);
                }
                None => banner.set_revealed(false),
            }
        }
    });

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&banner);
    toolbar.set_content(Some(&toasts));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Cutmary")
        .icon_name(APP_ID)
        .default_width(640)
        .default_height(640)
        .width_request(360)
        .height_request(420)
        .content(&toolbar)
        .build();
    ctl.attach(&window, &toasts);

    let logs = gio::ActionEntry::builder("logs")
        .activate({
            let ctl = ctl.clone();
            move |_: &adw::ApplicationWindow, _, _| super::logs::present(&ctl)
        })
        .build();
    let about =
        gio::ActionEntry::builder("about").activate(|window: &adw::ApplicationWindow, _, _| show_about(window)).build();
    window.add_action_entries([logs, about]);
    window
}

fn show_about(window: &adw::ApplicationWindow) {
    let about = adw::AboutDialog::builder()
        .application_name("Cutmary")
        .application_icon(APP_ID)
        .version(VERSION)
        .developer_name("Cutmary contributors")
        .license_type(gtk::License::MitX11)
        .website(format!("https://github.com/{REPOSITORY}"))
        .issue_url(format!("https://github.com/{REPOSITORY}/issues"))
        .comments(
            "Runs the official macOS build of CapCut natively on Linux through the Rosemary \
             compatibility runtime — no Wine, Proton or virtual machine.\n\n\
             Cutmary does not include or redistribute CapCut: it downloads the official \
             installer from ByteDance on your machine. CapCut is a trademark of ByteDance Ltd.",
        )
        .build();
    about.add_credit_section(Some("Runtime"), &["Rosemary https://github.com/microlift/rosemary"]);
    about.present(Some(window));
}
