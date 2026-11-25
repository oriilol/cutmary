//! In-app viewer for `~/.local/share/cutmary/logs/latest.log`.

use std::cell::Cell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::controller::Controller;

/// Only the tail is shown; huge Rosemary traces would make the view sluggish.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

fn read_tail(path: &Path) -> (String, u64) {
    let Ok(mut file) = File::open(path) else {
        return (String::new(), 0);
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(MAX_BYTES);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_ok() {
        let _ = file.read_to_end(&mut bytes);
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0 {
        // Drop the partial first line and say what was skipped.
        let cut = text.find('\n').map(|i| i + 1).unwrap_or(0);
        text = format!("… ({} earlier bytes not shown)\n{}", start, &text[cut..]);
    }
    (text, len)
}

pub fn present(ctl: &Rc<Controller>) {
    let path = ctl.log.path();

    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .css_classes(["cutmary-log"])
        .build();
    let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
    let placeholder = adw::StatusPage::builder()
        .icon_name("text-x-generic-symbolic")
        .title("No Logs Yet")
        .description("Install or launch CapCut and its output will appear here.")
        .build();
    let content = gtk::Stack::new();
    content.add_named(&scroller, Some("log"));
    content.add_named(&placeholder, Some("empty"));

    let copy = gtk::Button::builder().label("Copy").css_classes(["pill"]).build();
    let open = gtk::Button::builder().label("Open Logs Folder").css_classes(["pill"]).build();
    let bottom = gtk::Box::builder().spacing(12).halign(gtk::Align::Center).margin_top(12).margin_bottom(12).build();
    bottom.append(&open);
    bottom.append(&copy);

    let title = adw::WindowTitle::new("Logs", &path.display().to_string());
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&content));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_bottom_bar(&bottom);
    toolbar.set_content(Some(&toasts));

    let dialog = adw::Dialog::builder().title("Logs").content_width(820).content_height(560).child(&toolbar).build();

    // (Re)load when the file changes; stick to the bottom if already there.
    let last_len = Rc::new(Cell::new(u64::MAX));
    let reload = {
        let (view, scroller, content, path, last_len) =
            (view.clone(), scroller.clone(), content.clone(), path.clone(), last_len.clone());
        move || {
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if len == last_len.get() {
                return;
            }
            let adj = scroller.vadjustment();
            let at_bottom = last_len.get() == u64::MAX || adj.value() + adj.page_size() >= adj.upper() - 4.0;
            let (text, len) = read_tail(&path);
            last_len.set(len);
            content.set_visible_child_name(if text.trim().is_empty() { "empty" } else { "log" });
            view.buffer().set_text(&text);
            if at_bottom {
                let view = view.clone();
                glib::idle_add_local_once(move || {
                    let mut end = view.buffer().end_iter();
                    view.scroll_to_iter(&mut end, 0.0, false, 0.0, 1.0);
                });
            }
        }
    };
    reload();
    let alive = Rc::new(Cell::new(true));
    dialog.connect_closed({
        let alive = alive.clone();
        move |_| alive.set(false)
    });
    glib::timeout_add_local(Duration::from_millis(1000), {
        let alive = alive.clone();
        move || {
            if !alive.get() {
                return glib::ControlFlow::Break;
            }
            reload();
            glib::ControlFlow::Continue
        }
    });

    copy.connect_clicked({
        let (view, toasts) = (view.clone(), toasts.clone());
        move |button| {
            let buffer = view.buffer();
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            button.clipboard().set_text(&text);
            toasts.add_toast(adw::Toast::new("Copied to clipboard"));
        }
    });
    open.connect_clicked({
        let ctl = ctl.clone();
        let dir = ctl.paths.logs_dir();
        move |_| {
            let _ = std::fs::create_dir_all(&dir);
            ctl.open_folder(&dir);
        }
    });

    dialog.present(ctl.window().as_ref());
}
