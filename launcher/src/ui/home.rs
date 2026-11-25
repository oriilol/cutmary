//! Home tab: emblem, versions, and the Install / Launch call to action.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use super::controller::{Activity, Change, Controller};
use crate::config::{APP_ID, VERSION};
use crate::installer::Source;

pub fn build(ctl: &Rc<Controller>) -> gtk::Widget {
    let icon = gtk::Image::builder().icon_name(APP_ID).pixel_size(128).css_classes(["cutmary-hero-icon"]).build();
    let title = gtk::Label::builder().label("Cutmary").css_classes(["cutmary-title"]).build();
    let subtitle = gtk::Label::builder().css_classes(["dim-label"]).build();

    // Install progress.
    let step = gtk::Label::builder().css_classes(["heading"]).build();
    let bar = gtk::ProgressBar::builder().css_classes(["cutmary-progress"]).build();
    let detail = gtk::Label::builder()
        .css_classes(["dim-label", "caption", "numeric"])
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .max_width_chars(48)
        .build();
    let progress_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
    progress_box.append(&step);
    progress_box.append(&bar);
    progress_box.append(&detail);
    let progress =
        gtk::Revealer::builder().child(&progress_box).transition_type(gtk::RevealerTransitionType::SlideDown).build();

    // Actions.
    let primary = gtk::Button::builder().css_classes(["pill", "suggested-action", "cutmary-cta"]).build();
    let stop = gtk::Button::builder().label("Stop").css_classes(["pill", "destructive-action"]).build();
    let cancel = gtk::Button::builder().label("Cancel").css_classes(["pill"]).build();
    let buttons = gtk::Box::builder().spacing(12).halign(gtk::Align::Center).build();
    buttons.append(&primary);
    buttons.append(&stop);
    buttons.append(&cancel);

    let local_dmg = gtk::Button::builder()
        .label("Use local .dmg…")
        .tooltip_text("Install from a CapCut DMG you already downloaded")
        .halign(gtk::Align::Center)
        .css_classes(["flat", "dim-label"])
        .build();

    let logs_content = adw::ButtonContent::builder().icon_name("text-x-generic-symbolic").label("Show Logs").build();
    let logs = gtk::Button::builder()
        .child(&logs_content)
        .halign(gtk::Align::Center)
        .css_classes(["flat", "dim-label"])
        .build();

    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    column.append(&icon);
    column.append(&title);
    column.append(&subtitle);
    let spacer = gtk::Box::builder().height_request(18).build();
    column.append(&spacer);
    column.append(&progress);
    column.append(&buttons);
    column.append(&local_dmg);
    column.append(&logs);

    primary.connect_clicked({
        let ctl = ctl.clone();
        move |_| match ctl.installed() {
            Some(_) => ctl.launch(),
            None => ctl.start_install(Source::Download),
        }
    });
    stop.connect_clicked({
        let ctl = ctl.clone();
        move |_| ctl.stop()
    });
    cancel.connect_clicked({
        let ctl = ctl.clone();
        move |button| {
            button.set_sensitive(false);
            ctl.cancel_install();
        }
    });
    local_dmg.connect_clicked({
        let ctl = ctl.clone();
        move |_| choose_local_dmg(&ctl)
    });
    logs.connect_clicked({
        let ctl = ctl.clone();
        move |_| super::logs::present(&ctl)
    });

    let refresh = {
        let ctl = Rc::downgrade(ctl);
        move || {
            let Some(ctl) = ctl.upgrade() else { return };
            let installed = ctl.installed();
            subtitle.set_label(&match installed.as_ref() {
                Some(app) => format!("CapCut {} • Cutmary {VERSION}", app.version.as_deref().unwrap_or("(unknown)")),
                None => format!("CapCut not installed • Cutmary {VERSION}"),
            });
            let activity = ctl.activity();
            let idle = activity == Activity::Idle;
            primary.set_sensitive(idle);
            primary.set_label(match (&activity, installed.is_some()) {
                (Activity::Installing(_), _) => "Installing…",
                (Activity::Running, _) => "Running…",
                (Activity::Stopping, _) => "Stopping…",
                (Activity::Idle, true) => "Launch",
                (Activity::Idle, false) => "Install CapCut",
            });
            stop.set_visible(matches!(activity, Activity::Running | Activity::Stopping));
            stop.set_sensitive(activity == Activity::Running);
            local_dmg.set_visible(idle && installed.is_none());
            match &activity {
                Activity::Installing(p) => {
                    if !cancel.is_visible() {
                        cancel.set_sensitive(true);
                    }
                    cancel.set_visible(true);
                    progress.set_reveal_child(true);
                    step.set_label(p.step.label());
                    bar.set_fraction(p.fraction);
                    detail.set_label(&p.detail);
                }
                _ => {
                    cancel.set_visible(false);
                    progress.set_reveal_child(false);
                }
            }
        }
    };
    refresh();
    ctl.subscribe(move |change| {
        if matches!(change, Change::Activity | Change::Installed) {
            refresh();
        }
    });

    gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&column).build().upcast()
}

fn choose_local_dmg(ctl: &Rc<Controller>) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("macOS disk images (*.dmg)"));
    filter.add_suffix("dmg");
    filter.add_mime_type("application/x-apple-diskimage");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title("Choose a CapCut DMG")
        .modal(true)
        .filters(&filters)
        .default_filter(&filter)
        .build();
    let ctl = ctl.clone();
    glib::spawn_future_local(async move {
        let window = ctl.window();
        if let Ok(file) = dialog.open_future(window.as_ref()).await
            && let Some(path) = file.path()
        {
            ctl.start_install(Source::LocalDmg(path));
        }
    });
}
