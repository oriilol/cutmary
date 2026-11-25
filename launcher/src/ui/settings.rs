//! Settings tab: Cutmary, CapCut installation, Rosemary runtime, graphics.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use super::controller::{Activity, Change, Controller};
use crate::config::{Choice, Config, Decorations, PresentMode, RunnerChoice, Theme, VERSION};
use crate::installer::Source;
use crate::runner::{self, PrefixStatus};
use crate::update::UpdateStatus;
use crate::util;

fn combo<T: Choice>(ctl: &Rc<Controller>, title: &str, current: T, apply: fn(&mut Config, T)) -> adw::ComboRow {
    let labels: Vec<&str> = T::ALL.iter().map(|c| c.label()).collect();
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(&labels))
        .selected(current.index())
        .build();
    let ctl = ctl.clone();
    row.connect_selected_notify(move |row| {
        let value = T::from_index(row.selected());
        ctl.update_config(|c| apply(c, value));
    });
    row
}

fn switch(ctl: &Rc<Controller>, title: &str, subtitle: &str, active: bool, apply: fn(&mut Config, bool)) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).subtitle(subtitle).active(active).build();
    let ctl = ctl.clone();
    row.connect_active_notify(move |row| {
        let value = row.is_active();
        ctl.update_config(|c| apply(c, value));
    });
    row
}

fn suffix_button(label: &str) -> gtk::Button {
    gtk::Button::builder().label(label).valign(gtk::Align::Center).build()
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build()
}

pub fn build(ctl: &Rc<Controller>) -> gtk::Widget {
    let config = ctl.config().clone();
    let page = adw::PreferencesPage::new();

    // ---- Cutmary ------------------------------------------------------------
    let cutmary = adw::PreferencesGroup::builder().title("Cutmary").build();

    let version_row = adw::ActionRow::builder().title("Launcher Version").subtitle(VERSION).build();
    let update_button = suffix_button("Check for Updates");
    let download_button = suffix_button("Download");
    download_button.add_css_class("suggested-action");
    download_button.set_visible(false);
    version_row.add_suffix(&download_button);
    version_row.add_suffix(&update_button);
    let release_url = Rc::new(std::cell::RefCell::new(String::new()));
    download_button.connect_clicked({
        let (ctl, release_url) = (ctl.clone(), release_url.clone());
        move |_| ctl.open_uri(&release_url.borrow())
    });
    update_button.connect_clicked({
        let (ctl, version_row, download_button) = (ctl.clone(), version_row.clone(), download_button.clone());
        move |button| {
            button.set_sensitive(false);
            button.set_label("Checking…");
            let (button, version_row, download_button, release_url, ctl2) =
                (button.clone(), version_row.clone(), download_button.clone(), release_url.clone(), ctl.clone());
            ctl.check_updates(move |result| {
                button.set_sensitive(true);
                button.set_label("Check for Updates");
                download_button.set_visible(false);
                match result {
                    Ok(UpdateStatus::UpToDate) => version_row.set_subtitle(&format!("{VERSION} — up to date")),
                    Ok(UpdateStatus::NoReleases) => {
                        version_row.set_subtitle(&format!("{VERSION} — no releases published yet"))
                    }
                    Ok(UpdateStatus::Available { version, url }) => {
                        version_row.set_subtitle(&glib::markup_escape_text(&format!(
                            "{VERSION} — Cutmary {version} is available"
                        )));
                        release_url.replace(url);
                        download_button.set_visible(true);
                    }
                    Err(e) => {
                        version_row.set_subtitle(VERSION);
                        ctl2.toast(&format!("Update check failed: {e}"));
                    }
                }
            });
        }
    });
    cutmary.add(&version_row);

    let theme = combo(ctl, "App Theme", config.launcher.theme, |c, v: Theme| c.launcher.theme = v);
    theme.connect_selected_notify({
        let ctl = ctl.clone();
        move |_| ctl.apply_theme()
    });
    cutmary.add(&theme);
    cutmary.add(&switch(
        ctl,
        "Advanced Settings",
        "Show extra environment and debugging options",
        config.launcher.advanced,
        |c, v| c.launcher.advanced = v,
    ));
    page.add(&cutmary);

    // ---- CapCut installation ------------------------------------------------
    let capcut = adw::PreferencesGroup::builder().title("CapCut Installation").build();

    let status_icon = gtk::Image::new();
    let status_row = adw::ActionRow::builder().title("CapCut").build();
    status_row.add_prefix(&status_icon);
    let reinstall = suffix_button("Reinstall / Repair");
    status_row.add_suffix(&reinstall);
    reinstall.connect_clicked({
        let ctl = ctl.clone();
        move |_| confirm_reinstall(&ctl)
    });
    capcut.add(&status_row);

    capcut.add(&switch(
        ctl,
        "Thin to x86_64 Only",
        "Strip unused arm64 slices from universal binaries during install (saves about 1.1 GB)",
        config.capcut.thin_arm64,
        |c, v| c.capcut.thin_arm64 = v,
    ));

    let projects_row = adw::ActionRow::builder().title("CapCut Projects").activatable(true).build();
    projects_row.add_suffix(&gtk::Image::from_icon_name("cutmary-folder-open-symbolic"));
    projects_row.connect_activated({
        let ctl = ctl.clone();
        move |_| ctl.open_folder(&ctl.projects_dir())
    });
    capcut.add(&projects_row);
    page.add(&capcut);

    // ---- Runtime ------------------------------------------------------------
    let runtime = adw::PreferencesGroup::builder()
        .title("Runtime (Rosemary)")
        .description("The macOS compatibility runtime that runs CapCut")
        .build();

    let runner_row = combo(ctl, "Runner", config.runtime.runner, |c, v: RunnerChoice| c.runtime.runner = v);
    runtime.add(&runner_row);

    let custom_runner_row = adw::ActionRow::builder().title("Custom Rosemary Binary").build();
    let choose_runner = suffix_button("Choose…");
    custom_runner_row.add_suffix(&choose_runner);
    choose_runner.connect_clicked({
        let ctl = ctl.clone();
        move |_| choose_custom_runner(&ctl)
    });
    runtime.add(&custom_runner_row);

    let prefix_row = adw::ActionRow::builder().title("Prefix Directory").subtitle_selectable(true).build();
    let open_prefix = icon_button("cutmary-folder-open-symbolic", "Open Prefix Folder");
    let reset_prefix = icon_button("edit-undo-symbolic", "Use Default Prefix");
    let choose_prefix = suffix_button("Choose…");
    prefix_row.add_suffix(&open_prefix);
    prefix_row.add_suffix(&reset_prefix);
    prefix_row.add_suffix(&choose_prefix);
    open_prefix.connect_clicked({
        let ctl = ctl.clone();
        move |_| ctl.open_folder(&ctl.prefix_dir())
    });
    reset_prefix.connect_clicked({
        let ctl = ctl.clone();
        move |_| ctl.update_config(|c| c.runtime.prefix_dir = None)
    });
    choose_prefix.connect_clicked({
        let ctl = ctl.clone();
        move |_| choose_prefix_dir(&ctl)
    });
    runtime.add(&prefix_row);

    let prefix_icon = gtk::Image::new();
    let prefix_status_row = adw::ActionRow::builder().title("Prefix Status").build();
    prefix_status_row.add_prefix(&prefix_icon);
    let repair = suffix_button("Repair");
    prefix_status_row.add_suffix(&repair);
    repair.connect_clicked({
        let ctl = ctl.clone();
        move |_| ctl.repair_prefix()
    });
    runtime.add(&prefix_status_row);

    let unbound = switch(
        ctl,
        "Log All Unbound Imports",
        "ROSEMARY_UNBOUND_DETAIL=all — useful for compatibility bug reports",
        config.runtime.log_all_unbound_imports,
        |c, v| c.runtime.log_all_unbound_imports = v,
    );
    runtime.add(&unbound);
    page.add(&runtime);

    // ---- Graphics & window --------------------------------------------------
    let graphics = adw::PreferencesGroup::builder().title("Graphics &amp; Window").build();
    graphics.add(&combo(ctl, "Window Decorations", config.graphics.decorations, |c, v: Decorations| {
        c.graphics.decorations = v
    }));
    graphics.add(&combo(ctl, "Vulkan Presentation Mode", config.graphics.present_mode, |c, v: PresentMode| {
        c.graphics.present_mode = v
    }));
    graphics.add(&switch(
        ctl,
        "Keep Fault Handler",
        "Keep Rosemary's crash handler instead of CapCut's own (recommended)",
        config.graphics.keep_fault_handler,
        |c, v| c.graphics.keep_fault_handler = v,
    ));
    graphics.add(&switch(
        ctl,
        "Ctrl for Editing Shortcuts",
        "Use Ctrl+C, Ctrl+V, Ctrl+Z… instead of Super, as on Linux",
        config.graphics.ctrl_edit_shortcuts,
        |c, v| c.graphics.ctrl_edit_shortcuts = v,
    ));

    let env_row = adw::EntryRow::builder()
        .title("Custom Environment Variables")
        .text(&config.graphics.custom_env)
        .show_apply_button(true)
        .tooltip_text("Space-separated KEY=VALUE pairs; quote values containing spaces")
        .build();
    env_row.connect_apply({
        let ctl = ctl.clone();
        move |row| {
            let text = row.text().to_string();
            match runner::parse_env_assignments(&text) {
                Ok(vars) => {
                    row.remove_css_class("error");
                    ctl.update_config(|c| c.graphics.custom_env = text.trim().to_owned());
                    ctl.toast(&format!("Saved {} environment variable(s)", vars.len()));
                }
                Err(e) => {
                    row.add_css_class("error");
                    ctl.toast(&format!("Invalid environment variables: {e}"));
                }
            }
        }
    });
    env_row.connect_changed(|row| row.remove_css_class("error"));
    graphics.add(&env_row);
    page.add(&graphics);

    // ---- Live state ---------------------------------------------------------
    let refresh = {
        let ctl = Rc::downgrade(ctl);
        move |change: Change| {
            let Some(ctl) = ctl.upgrade() else { return };
            let config = ctl.config().clone();
            let idle = ctl.activity() == Activity::Idle;

            if matches!(change, Change::Installed | Change::Activity) {
                match ctl.installed() {
                    Some(app) => {
                        status_row.set_title(&format!("CapCut {}", app.version.as_deref().unwrap_or("")));
                        let size = ctl
                            .installed_size()
                            .map(util::format_bytes)
                            .unwrap_or_else(|| "measuring…".to_owned());
                        status_row.set_subtitle(&format!("Installed • {size} on disk"));
                        status_icon.set_icon_name(Some("cutmary-check-symbolic"));
                        status_icon.set_css_classes(&["success"]);
                        reinstall.set_label("Reinstall / Repair");
                    }
                    None => {
                        status_row.set_title("CapCut");
                        status_row.set_subtitle("Not installed");
                        status_icon.set_icon_name(Some("dialog-warning-symbolic"));
                        status_icon.set_css_classes(&["warning"]);
                        reinstall.set_label("Install");
                    }
                }
                reinstall.set_sensitive(idle);
            }

            if matches!(change, Change::Config | Change::Prefix) {
                unbound.set_visible(config.launcher.advanced);
                env_row.set_visible(config.launcher.advanced);

                custom_runner_row.set_visible(config.runtime.runner == RunnerChoice::Custom);
                custom_runner_row.set_subtitle(
                    &config
                        .runtime
                        .custom_runner
                        .as_ref()
                        .map(|p| glib::markup_escape_text(&p.display().to_string()).to_string())
                        .unwrap_or_else(|| "Not set".to_owned()),
                );
                match ctl.resolve_runner() {
                    Ok(r) => {
                        runner_row.set_subtitle(&glib::markup_escape_text(&r.describe()));
                        runner_row.remove_css_class("error");
                    }
                    Err(e) => {
                        runner_row.set_subtitle(&glib::markup_escape_text(&e));
                        runner_row.add_css_class("error");
                    }
                }

                let prefix = ctl.prefix_dir();
                let custom_prefix = config.runtime.prefix_dir.is_some();
                prefix_row.set_subtitle(&glib::markup_escape_text(&format!(
                    "{}{}",
                    prefix.display(),
                    if custom_prefix { "" } else { " (default)" }
                )));
                reset_prefix.set_visible(custom_prefix);
                projects_row.set_subtitle(&glib::markup_escape_text(&ctl.projects_dir().display().to_string()));

                let busy = ctl.prefix_busy();
                let (icon, class, text, action) = match ctl.prefix_status() {
                    PrefixStatus::Ready { base } => (
                        "cutmary-check-symbolic",
                        "success",
                        format!("Darwin base installed{}", base.map(|b| format!(" ({b})")).unwrap_or_default()),
                        "Repair",
                    ),
                    PrefixStatus::NoBase => (
                        "dialog-warning-symbolic",
                        "warning",
                        "Darwin base missing".to_owned(),
                        "Repair",
                    ),
                    PrefixStatus::Missing => (
                        "dialog-information-symbolic",
                        "accent",
                        "Not created yet — Rosemary creates it on first launch".to_owned(),
                        "Create",
                    ),
                };
                prefix_icon.set_icon_name(Some(icon));
                prefix_icon.set_css_classes(&[class]);
                prefix_status_row.set_subtitle(&text);
                repair.set_label(if busy { "Working…" } else { action });
                repair.set_sensitive(!busy);
            }
        }
    };
    refresh(Change::Installed);
    refresh(Change::Config);
    ctl.subscribe(refresh);

    page.upcast()
}

fn confirm_reinstall(ctl: &Rc<Controller>) {
    if ctl.installed().is_none() {
        ctl.start_install(Source::Download);
        return;
    }
    let dialog = adw::AlertDialog::builder()
        .heading("Reinstall CapCut?")
        .body(
            "Cutmary will download the latest official CapCut DMG and replace the current copy. \
             Your projects and settings in the Rosemary prefix are kept.",
        )
        .close_response("cancel")
        .default_response("reinstall")
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("reinstall", "Reinstall");
    dialog.set_response_appearance("reinstall", adw::ResponseAppearance::Suggested);
    let ctl2 = ctl.clone();
    dialog.connect_response(Some("reinstall"), move |_, _| ctl2.start_install(Source::Download));
    dialog.present(ctl.window().as_ref());
}

fn choose_custom_runner(ctl: &Rc<Controller>) {
    let dialog = gtk::FileDialog::builder().title("Choose the Rosemary Binary").modal(true).build();
    let ctl = ctl.clone();
    glib::spawn_future_local(async move {
        if let Ok(file) = dialog.open_future(ctl.window().as_ref()).await
            && let Some(path) = file.path()
        {
            if !util::is_executable(&path) {
                ctl.toast("That file is not executable");
                return;
            }
            ctl.update_config(|c| c.runtime.custom_runner = Some(path));
        }
    });
}

fn choose_prefix_dir(ctl: &Rc<Controller>) {
    let dialog = gtk::FileDialog::builder().title("Choose a Rosemary Prefix").modal(true).build();
    let current = ctl.prefix_dir();
    if let Some(existing) = current.ancestors().find(|p| p.is_dir()) {
        dialog.set_initial_folder(Some(&gio::File::for_path(existing)));
    }
    let ctl = ctl.clone();
    glib::spawn_future_local(async move {
        if let Ok(file) = dialog.select_folder_future(ctl.window().as_ref()).await
            && let Some(path) = file.path()
        {
            // Picking the default prefix explicitly is stored as "default".
            let custom = (path != ctl.paths.default_prefix).then_some(path);
            ctl.update_config(|c| c.runtime.prefix_dir = custom);
        }
    });
}
