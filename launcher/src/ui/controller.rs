//! Shared launcher state. Views subscribe to changes; long-running work runs
//! on worker threads and reports back to the GTK main loop over channels.

use std::cell::{Cell, Ref, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gtk::{gio, glib};

use crate::config::{Config, Paths, Theme, VERSION};
use crate::drafts::{self, BrokenDraft, DraftReport};
use crate::installer::{self, InstallError, InstallOptions, InstalledApp, Progress, Source};
use crate::logging::Log;
use crate::preflight::{self, HostProbe};
use crate::runner::{self, PrefixAction, PrefixStatus, ResolvedRunner, RunnerEnv, Session};
use crate::update::{self, UpdateStatus};
use crate::util;

#[derive(Clone, Debug, PartialEq)]
pub enum Activity {
    Idle,
    Installing(Progress),
    Running,
    Stopping,
}

/// What changed, so views can skip unrelated (and costlier) refreshes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Activity,
    Installed,
    Config,
    Prefix,
    Drafts,
    Preflight,
}

type Listener = Box<dyn Fn(Change)>;

pub struct Controller {
    pub paths: Paths,
    pub log: Log,
    config: RefCell<Config>,
    activity: RefCell<Activity>,
    installed: RefCell<Option<InstalledApp>>,
    installed_size: Cell<Option<u64>>,
    install_cancel: RefCell<Option<Arc<AtomicBool>>>,
    session: RefCell<Option<Session>>,
    prefix_busy: Cell<bool>,
    preflight: RefCell<Option<preflight::Report>>,
    listeners: RefCell<Vec<Listener>>,
    toasts: RefCell<Option<adw::ToastOverlay>>,
    window: glib::WeakRef<adw::ApplicationWindow>,
}

impl Controller {
    pub fn new() -> Rc<Self> {
        let paths = Paths::from_env();
        let log = Log::open(paths.latest_log());
        let (config, note) = Config::load_or_default(&paths.config_file());
        if let Some(note) = note {
            log.info(note);
        }
        let this = Rc::new(Self {
            paths,
            log,
            config: RefCell::new(config),
            activity: RefCell::new(Activity::Idle),
            installed: RefCell::new(None),
            installed_size: Cell::new(None),
            install_cancel: RefCell::new(None),
            session: RefCell::new(None),
            prefix_busy: Cell::new(false),
            preflight: RefCell::new(None),
            listeners: RefCell::new(Vec::new()),
            toasts: RefCell::new(None),
            window: glib::WeakRef::new(),
        });
        this.refresh_installed();
        this.adopt_running_session();
        this.run_preflight();
        this
    }

    // -- wiring ---------------------------------------------------------------

    pub fn attach(&self, window: &adw::ApplicationWindow, toasts: &adw::ToastOverlay) {
        self.window.set(Some(window));
        self.toasts.replace(Some(toasts.clone()));
    }

    pub fn window(&self) -> Option<adw::ApplicationWindow> {
        self.window.upgrade()
    }

    pub fn subscribe(&self, listener: impl Fn(Change) + 'static) {
        self.listeners.borrow_mut().push(Box::new(listener));
    }

    fn notify(&self, change: Change) {
        for listener in self.listeners.borrow().iter() {
            listener(change);
        }
    }

    pub fn toast(&self, message: &str) {
        if let Some(overlay) = self.toasts.borrow().as_ref() {
            overlay.add_toast(adw::Toast::builder().title(glib::markup_escape_text(message)).timeout(4).build());
        }
    }

    /// A toast with a "Show Logs" button, for failures.
    pub fn toast_error(self: &Rc<Self>, message: &str) {
        let Some(overlay) = self.toasts.borrow().clone() else { return };
        let toast =
            adw::Toast::builder().title(glib::markup_escape_text(message)).button_label("Show Logs").timeout(8).build();
        let this = Rc::downgrade(self);
        toast.connect_button_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                super::logs::present(&this);
            }
        });
        overlay.add_toast(toast);
    }

    // -- config ---------------------------------------------------------------

    pub fn config(&self) -> Ref<'_, Config> {
        self.config.borrow()
    }

    /// Mutates and persists the configuration.
    pub fn update_config(&self, change: impl FnOnce(&mut Config)) {
        let snapshot = {
            let mut config = self.config.borrow_mut();
            let before = config.clone();
            change(&mut config);
            if *config == before {
                return;
            }
            config.clone()
        };
        if let Err(e) = snapshot.save(&self.paths.config_file()) {
            self.log.info(format!("could not save settings: {e}"));
            self.toast(&format!("Could not save settings: {e}"));
        }
        self.notify(Change::Config);
    }

    pub fn apply_theme(&self) {
        let scheme = match self.config().launcher.theme {
            Theme::System => adw::ColorScheme::Default,
            Theme::Dark => adw::ColorScheme::ForceDark,
            Theme::Light => adw::ColorScheme::ForceLight,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
    }

    pub fn prefix_dir(&self) -> PathBuf {
        self.config().prefix_dir(&self.paths)
    }

    pub fn projects_dir(&self) -> PathBuf {
        crate::config::projects_dir(&self.prefix_dir(), &crate::config::current_user())
    }

    pub fn resolve_runner(&self) -> Result<ResolvedRunner, String> {
        runner::resolve_runner(&self.config(), &RunnerEnv::current(&self.paths.home, self.paths.in_flatpak))
    }

    // -- state ----------------------------------------------------------------

    pub fn activity(&self) -> Activity {
        self.activity.borrow().clone()
    }

    fn set_activity(&self, activity: Activity) {
        self.activity.replace(activity);
        self.notify(Change::Activity);
    }

    pub fn installed(&self) -> Option<InstalledApp> {
        self.installed.borrow().clone()
    }

    pub fn installed_size(&self) -> Option<u64> {
        self.installed_size.get()
    }

    pub fn prefix_busy(&self) -> bool {
        self.prefix_busy.get()
    }

    pub fn prefix_status(&self) -> PrefixStatus {
        runner::prefix_status(&self.prefix_dir())
    }

    /// Re-reads the installed bundle and measures it in the background.
    pub fn refresh_installed(self: &Rc<Self>) {
        let installed = installer::installed(&self.paths);
        self.installed_size.set(None);
        if let Some(app) = &installed {
            let path = app.path.clone();
            let this = Rc::downgrade(self);
            run_in_background(
                move || util::dir_size(&path),
                move |size| {
                    if let Some(this) = this.upgrade() {
                        this.installed_size.set(Some(size));
                        this.notify(Change::Installed);
                    }
                },
            );
        }
        self.installed.replace(installed);
        self.notify(Change::Installed);
    }

    // -- runtime requirements -------------------------------------------------

    pub fn preflight(&self) -> Option<preflight::Report> {
        self.preflight.borrow().clone()
    }

    /// Checks host libraries in the background; never blocks a launch.
    pub fn run_preflight(self: &Rc<Self>) {
        let in_flatpak = self.paths.in_flatpak;
        let this = Rc::downgrade(self);
        run_in_background(
            move || preflight::run(&HostProbe { in_flatpak }),
            move |report| {
                let Some(this) = this.upgrade() else { return };
                this.log.info(format!("preflight: {}", report.summary()));
                this.preflight.replace(Some(report));
                this.notify(Change::Preflight);
            },
        );
    }

    // -- drafts ---------------------------------------------------------------

    pub fn drafts_report(&self) -> std::io::Result<DraftReport> {
        drafts::scan(&self.projects_dir())
    }

    fn capcut_busy(&self) -> bool {
        *self.activity.borrow() != Activity::Idle || drafts::capcut_running(std::process::id())
    }

    /// Removes stale 0-byte `*.locked` markers, logging each one. Returns how
    /// many were removed, or `None` when CapCut is running and nothing was done.
    fn clean_draft_locks(&self) -> Option<usize> {
        if drafts::capcut_running(std::process::id()) {
            self.log.info("drafts: CapCut is running elsewhere; leaving draft locks alone");
            return None;
        }
        let root = self.projects_dir();
        match drafts::clean_stale_locks(&root) {
            Ok(cleanup) => {
                for lock in &cleanup.removed {
                    let shown = lock.strip_prefix(&root).unwrap_or(lock);
                    self.log.info(format!("drafts: removed stale lock {}", shown.display()));
                }
                for lock in &cleanup.kept {
                    let shown = lock.strip_prefix(&root).unwrap_or(lock);
                    self.log.info(format!("drafts: kept non-empty lock {}", shown.display()));
                }
                Some(cleanup.removed.len())
            }
            Err(e) => {
                self.log.info(format!("drafts: could not clean locks in {}: {e}", root.display()));
                Some(0)
            }
        }
    }

    /// Settings' "Repair Drafts": cleans stale locks and returns damaged
    /// drafts for the UI to offer archiving. `None` if CapCut is running.
    pub fn repair_drafts(&self) -> Option<Vec<BrokenDraft>> {
        if self.capcut_busy() {
            self.toast("Close CapCut before repairing drafts");
            return None;
        }
        self.log.info(format!("drafts: repairing {}", self.projects_dir().display()));
        let removed = self.clean_draft_locks()?;
        let mut restored = 0;
        if let Ok(report) = self.drafts_report() {
            for draft in report.broken.iter().filter(|d| d.has_backup) {
                match drafts::restore_from_backup(&draft.path) {
                    Ok(true) => {
                        restored += 1;
                        self.log.info(format!("drafts: restored \"{}\" from draft_info.json.bak", draft.name()));
                    }
                    Ok(false) => {}
                    Err(e) => self.log.info(format!("drafts: could not restore \"{}\": {e}", draft.name())),
                }
            }
        }
        let report = match self.drafts_report() {
            Ok(report) => report,
            Err(e) => {
                self.toast(&format!("Could not read drafts: {e}"));
                return None;
            }
        };
        for draft in &report.broken {
            self.log.info(format!("drafts: \"{}\" looks damaged ({})", draft.name(), draft.damage.describe()));
        }
        let mut done = vec![format!("removed {removed} stale lock(s)")];
        if restored > 0 {
            done.push(format!("restored {restored} draft(s) from backup"));
        }
        let done = done.join(", ");
        if report.broken.is_empty() {
            self.toast(&format!("Drafts repaired: {done} — all {} draft(s) look healthy", report.drafts));
        } else if removed > 0 || restored > 0 {
            self.toast(&format!("Drafts repaired: {done}"));
        }
        self.notify(Change::Drafts);
        Some(report.broken)
    }

    /// Moves damaged drafts to the archive folder (never deletes them).
    pub fn archive_drafts(self: &Rc<Self>, broken: &[BrokenDraft]) {
        if self.capcut_busy() {
            self.toast("Close CapCut before archiving drafts");
            return;
        }
        let root = self.projects_dir();
        let paths: Vec<PathBuf> = broken.iter().map(|d| d.path.clone()).collect();
        match drafts::archive(&root, &paths) {
            Ok(moved) => {
                for target in &moved {
                    self.log.info(format!("drafts: archived to {}", target.display()));
                }
                self.toast(&format!("Archived {} damaged draft(s) to “{}”", moved.len(), drafts::ARCHIVE_DIR));
            }
            Err(e) => {
                self.log.info(format!("drafts: archiving failed: {e}"));
                self.toast_error(&format!("Archiving failed: {e}"));
            }
        }
        self.notify(Change::Drafts);
    }

    // -- install --------------------------------------------------------------

    pub fn start_install(self: &Rc<Self>, source: Source) {
        if *self.activity.borrow() != Activity::Idle {
            return;
        }
        self.log.rotate();
        self.log.info(format!("Cutmary {VERSION}: installing CapCut ({source:?})"));
        let cancel = Arc::new(AtomicBool::new(false));
        self.install_cancel.replace(Some(cancel.clone()));
        self.set_activity(Activity::Installing(Progress {
            step: installer::Step::Resolve,
            fraction: 0.0,
            detail: "Starting…".into(),
        }));

        enum Msg {
            Progress(Progress),
            Done(Result<installer::InstallSummary, InstallError>),
        }
        let (tx, rx) = async_channel::unbounded();
        let options = InstallOptions { source, thin: self.config().capcut.thin_arm64 };
        let (paths, log) = (self.paths.clone(), self.log.clone());
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let emit = move |p: Progress| {
                let _ = progress_tx.send_blocking(Msg::Progress(p));
            };
            let result = installer::run_install(&paths, &options, &cancel, &log, &emit);
            let _ = tx.send_blocking(Msg::Done(result));
        });

        let this = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                match msg {
                    Msg::Progress(p) => this.set_activity(Activity::Installing(p)),
                    Msg::Done(result) => {
                        this.install_cancel.replace(None);
                        this.refresh_installed();
                        this.set_activity(Activity::Idle);
                        match result {
                            Ok(summary) => {
                                let mut msg = format!("CapCut {} is ready", summary.version.as_deref().unwrap_or(""));
                                if let Some(thin) = summary.thin.filter(|t| t.saved() > 0) {
                                    msg.push_str(&format!(" — saved {}", util::format_bytes(thin.saved())));
                                }
                                this.toast(&msg.replace("  ", " "));
                            }
                            Err(InstallError::Cancelled) => {
                                this.log.info("installation cancelled");
                                this.toast("Installation cancelled");
                            }
                            Err(InstallError::Failed(e)) => {
                                this.log.info(format!("installation failed: {e}"));
                                this.toast_error(&format!("Installation failed: {e}"));
                            }
                        }
                        break;
                    }
                }
            }
        });
    }

    pub fn cancel_install(&self) {
        if let Some(cancel) = self.install_cancel.borrow().as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    // -- launch ---------------------------------------------------------------

    fn session_pid_file(&self) -> PathBuf {
        self.paths.data_dir.join("session.pid")
    }

    /// Re-attaches to CapCut if a previous launcher run left it running.
    fn adopt_running_session(self: &Rc<Self>) {
        let pid_file = self.session_pid_file();
        let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse().ok()) else {
            return;
        };
        let needle = self.paths.app_bundle().to_string_lossy().into_owned();
        let (tx, rx) = async_channel::bounded(1);
        match Session::adopt(pid, &needle, move || {
            let _ = tx.send_blocking(SessionEnded { summary: "re-attached session ended".to_owned(), success: true });
        }) {
            Some(session) => {
                self.log.info(format!("re-attached to running CapCut session (pid {pid})"));
                self.session.replace(Some(session));
                self.activity.replace(Activity::Running);
                self.watch_session_end(rx);
            }
            None => {
                let _ = std::fs::remove_file(pid_file);
            }
        }
    }

    pub fn launch(self: &Rc<Self>) {
        if *self.activity.borrow() != Activity::Idle {
            return;
        }
        let Some(app) = self.installed() else {
            self.toast("Install CapCut first");
            return;
        };
        let runner = match self.resolve_runner() {
            Ok(runner) => runner,
            Err(e) => {
                self.log.info(&e);
                self.toast_error(&e);
                return;
            }
        };
        let wayland = util::is_wayland_session();
        let spec = match runner::launch_spec(&runner, &self.config(), &self.prefix_dir(), &app.path, wayland) {
            Ok(spec) => spec,
            Err(e) => {
                self.toast(&format!("Custom environment variables: {e}"));
                return;
            }
        };
        self.log.rotate();
        self.log.info(format!(
            "Cutmary {VERSION}: launching CapCut {}",
            app.version.as_deref().unwrap_or("(unknown version)")
        ));
        if let Some(removed) = self.clean_draft_locks()
            && removed > 0
        {
            self.toast(&format!("Cleared {removed} stale project lock(s) from a previous crash"));
            self.notify(Change::Drafts);
        }
        if let Some(report) = self.preflight.borrow().as_ref()
            && !report.all_ok()
        {
            self.log.info(format!("preflight: {}", report.summary()));
        }
        self.log.info(format!("$ {}", spec.display()));

        let (tx, rx) = async_channel::bounded(1);
        match Session::start(&spec, &self.log, move |status| {
            let ended = match status {
                Ok(status) => SessionEnded { summary: status.to_string(), success: status.success() },
                Err(e) => SessionEnded { summary: e.to_string(), success: false },
            };
            let _ = tx.send_blocking(ended);
        }) {
            Ok(session) => {
                let _ = std::fs::create_dir_all(&self.paths.data_dir);
                let _ = std::fs::write(self.session_pid_file(), session.pid().to_string());
                self.session.replace(Some(session));
                self.set_activity(Activity::Running);
                self.watch_session_end(rx);
            }
            Err(e) => {
                let msg = format!("Could not start {}: {e}", runner.describe());
                self.log.info(&msg);
                self.toast_error(&msg);
            }
        }
    }

    /// Waits for the session to end and resets the state.
    fn watch_session_end(self: &Rc<Self>, rx: async_channel::Receiver<SessionEnded>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let ended = rx.recv().await.ok();
            let stopping = *this.activity.borrow() == Activity::Stopping;
            this.session.replace(None);
            let _ = std::fs::remove_file(this.session_pid_file());
            this.set_activity(Activity::Idle);
            let Some(ended) = ended else { return };
            this.log.info(format!("CapCut exited ({})", ended.summary));
            if !stopping && !ended.success {
                this.toast_error(&format!("CapCut exited unexpectedly ({})", ended.summary));
            }
        });
    }

    pub fn stop(&self) {
        if *self.activity.borrow() != Activity::Running {
            return;
        }
        if let Some(session) = self.session.borrow().as_ref() {
            self.log.info("stopping CapCut");
            session.stop(Duration::from_secs(5));
        }
        self.set_activity(Activity::Stopping);
    }

    // -- prefix ---------------------------------------------------------------

    pub fn repair_prefix(self: &Rc<Self>) {
        if self.prefix_busy.get() {
            return;
        }
        let runner = match self.resolve_runner() {
            Ok(runner) => runner,
            Err(e) => {
                self.toast_error(&e);
                return;
            }
        };
        let prefix = self.prefix_dir();
        let action = match runner::prefix_status(&prefix) {
            PrefixStatus::Ready { .. } => PrefixAction::Repair,
            _ => PrefixAction::Create,
        };
        let spec = runner::prefix_spec(&runner, action, &prefix);
        self.log.info(format!("$ {}", spec.display()));
        self.prefix_busy.set(true);
        self.notify(Change::Prefix);

        let log = self.log.clone();
        let this = Rc::downgrade(self);
        run_in_background(
            move || {
                runner::spawn_logged(&spec, &log)
                    .and_then(|mut child| child.wait())
                    .map_err(|e| e.to_string())
                    .and_then(|s| if s.success() { Ok(()) } else { Err(format!("rosemary exited with {s}")) })
            },
            move |result| {
                let Some(this) = this.upgrade() else { return };
                this.prefix_busy.set(false);
                this.notify(Change::Prefix);
                match result {
                    Ok(()) => this.toast(match action {
                        PrefixAction::Create => "Prefix created",
                        PrefixAction::Repair => "Prefix repaired",
                    }),
                    Err(e) => {
                        this.log.info(format!("prefix {action:?} failed: {e}"));
                        this.toast_error(&format!("Prefix {} failed", format!("{action:?}").to_lowercase()));
                    }
                }
            },
        );
    }

    // -- updates & files ------------------------------------------------------

    pub fn check_updates(&self, done: impl FnOnce(Result<UpdateStatus, String>) + 'static) {
        run_in_background(update::check, done);
    }

    /// Opens `dir` in the file manager, falling back to its nearest existing
    /// ancestor (with a note) when it does not exist yet.
    pub fn open_folder(&self, dir: &Path) {
        let existing = dir.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf);
        let Some(target) = existing else {
            self.toast("Folder does not exist yet");
            return;
        };
        if target != dir {
            self.toast("That folder does not exist yet — opening the closest one");
        }
        let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&target)));
        launcher.launch(self.window().as_ref(), gio::Cancellable::NONE, |result| {
            if let Err(e) = result {
                eprintln!("cutmary: could not open folder: {e}");
            }
        });
    }

    pub fn open_uri(&self, uri: &str) {
        gtk::UriLauncher::new(uri).launch(self.window().as_ref(), gio::Cancellable::NONE, |_| {});
    }
}

/// How a CapCut session ended.
struct SessionEnded {
    summary: String,
    success: bool,
}

/// Runs `work` on a thread and hands its result to `done` on the main loop.
pub fn run_in_background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work());
    });
    glib::spawn_future_local(async move {
        if let Ok(value) = rx.recv().await {
            done(value);
        }
    });
}
