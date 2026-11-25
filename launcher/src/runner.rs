//! Locating Rosemary, building its command line and environment, and running
//! CapCut under it with output captured to the launcher log.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::config::{Config, RunnerChoice};
use crate::logging::Log;
use crate::util;

pub const ROSEMARY: &str = "rosemary";

// ---------------------------------------------------------------------------
// Runner resolution
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnerSource {
    /// Shipped next to the launcher (AppImage, Flatpak, distro package).
    Bundled,
    /// Found in `$PATH` or a usual user bin directory.
    System,
    /// Chosen by the user.
    Custom,
    /// Inside a Flatpak, run the host's `rosemary` through `flatpak-spawn --host`.
    Host,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRunner {
    pub program: PathBuf,
    pub source: RunnerSource,
}

impl ResolvedRunner {
    pub fn describe(&self) -> String {
        match self.source {
            RunnerSource::Host => "Host rosemary (via flatpak-spawn)".to_owned(),
            _ => self.program.display().to_string(),
        }
    }
}

/// What runner resolution may look at; captured once so tests can fake it.
#[derive(Clone, Debug, Default)]
pub struct RunnerEnv {
    pub exe_dir: Option<PathBuf>,
    pub path_var: Option<OsString>,
    pub home: PathBuf,
    pub in_flatpak: bool,
}

impl RunnerEnv {
    pub fn current(home: &Path, in_flatpak: bool) -> Self {
        Self {
            exe_dir: std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)),
            path_var: std::env::var_os("PATH"),
            home: home.to_path_buf(),
            in_flatpak,
        }
    }

    fn bundled(&self) -> Option<PathBuf> {
        let dir = self.exe_dir.as_ref()?;
        [dir.join(ROSEMARY), dir.join("../lib/cutmary").join(ROSEMARY), dir.join("../libexec/cutmary").join(ROSEMARY)]
            .into_iter()
            .map(|p| util::normalize_lexically(&p))
            .find(|p| util::is_executable(p))
    }

    fn system(&self) -> Option<PathBuf> {
        util::which_in(ROSEMARY, self.path_var.as_deref()).or_else(|| {
            [self.home.join(".local/bin").join(ROSEMARY), self.home.join(".cargo/bin").join(ROSEMARY)]
                .into_iter()
                .find(|p| util::is_executable(p))
        })
    }
}

pub fn resolve_runner(config: &Config, env: &RunnerEnv) -> Result<ResolvedRunner, String> {
    let found = |program, source| Ok(ResolvedRunner { program, source });
    match config.runtime.runner {
        RunnerChoice::Custom => match &config.runtime.custom_runner {
            Some(path) if util::is_executable(path) => found(path.clone(), RunnerSource::Custom),
            Some(path) => Err(format!("custom runner {} is not an executable file", path.display())),
            None => Err("no custom Rosemary binary selected".to_owned()),
        },
        RunnerChoice::Bundled => match env.bundled() {
            Some(path) => found(path, RunnerSource::Bundled),
            None => Err("this Cutmary build does not bundle Rosemary".to_owned()),
        },
        RunnerChoice::Auto => {
            if let Some(path) = env.bundled() {
                found(path, RunnerSource::Bundled)
            } else if let Some(path) = env.system() {
                found(path, RunnerSource::System)
            } else if env.in_flatpak {
                found(PathBuf::from(ROSEMARY), RunnerSource::Host)
            } else {
                Err("Rosemary was not found. Install it or choose a custom runner in Settings.".to_owned())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Command + environment
// ---------------------------------------------------------------------------

/// A fully resolved process invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(String, String)>,
}

impl LaunchSpec {
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args).envs(self.env.iter().map(|(k, v)| (k, v)));
        cmd
    }

    #[cfg(test)]
    pub fn env_value(&self, key: &str) -> Option<&str> {
        self.env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// A shell-like rendering for the log.
    pub fn display(&self) -> String {
        let quote = |s: &str| {
            if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c)) {
                s.to_owned()
            } else {
                format!("'{}'", s.replace('\'', r"'\''"))
            }
        };
        let env = self.env.iter().map(|(k, v)| format!("{k}={}", quote(v)));
        let argv = std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(self.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .map(|a| quote(&a));
        env.chain(argv).collect::<Vec<_>>().join(" ")
    }
}

fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parses `KEY=VALUE` pairs separated by whitespace, newlines or `;`.
/// Values may be single- or double-quoted; `\` escapes inside double quotes.
pub fn parse_env_assignments(input: &str) -> Result<Vec<(String, String)>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() || c == ';' => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err("unterminated single quote".to_owned()),
                    }
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c) => current.push(c),
                            None => return Err("dangling backslash".to_owned()),
                        },
                        Some(c) => current.push(c),
                        None => return Err("unterminated double quote".to_owned()),
                    }
                }
            }
            '\\' => {
                in_token = true;
                current.push(chars.next().ok_or("dangling backslash")?);
            }
            c => {
                in_token = true;
                current.push(c);
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    tokens
        .into_iter()
        .map(|token| {
            let (key, value) = token.split_once('=').ok_or_else(|| format!("`{token}` is not KEY=VALUE"))?;
            if !is_valid_env_key(key) {
                return Err(format!("`{key}` is not a valid variable name"));
            }
            Ok((key.to_owned(), value.to_owned()))
        })
        .collect()
}

fn set(env: &mut Vec<(String, String)>, key: &str, value: impl Into<String>) {
    let value = value.into();
    match env.iter_mut().find(|(k, _)| k == key) {
        Some(entry) => entry.1 = value,
        None => env.push((key.to_owned(), value)),
    }
}

/// The Rosemary environment for the current settings. Custom variables are
/// applied last (overriding the switches) and only while advanced mode is on.
/// `wayland` says whether the host session is Wayland (see [`util::is_wayland_session`]).
pub fn rosemary_env(config: &Config, prefix: &Path, wayland: bool) -> Result<Vec<(String, String)>, String> {
    let mut env = Vec::new();
    let g = &config.graphics;
    set(&mut env, "ROSEMARY_PREFIX", prefix.to_string_lossy());
    set(&mut env, "ROSEMARY_DECORATIONS", g.decorations.env_value());
    if let Some(menu) = g.menu.env_value(wayland) {
        set(&mut env, "ROSEMARY_MENU", menu);
    }
    if let Some(mode) = g.present_mode.env_value() {
        set(&mut env, "ROSEMARY_METAL_PRESENTATION_MODE", mode);
    }
    if g.keep_fault_handler {
        set(&mut env, "ROSEMARY_KEEP_FAULT_HANDLER", "1");
    }
    if g.ctrl_edit_shortcuts {
        set(&mut env, "ROSEMARY_CONTROL_EDIT_KEYS", "1");
    }
    if config.launcher.advanced {
        if config.runtime.log_all_unbound_imports {
            set(&mut env, "ROSEMARY_UNBOUND_DETAIL", "all");
        }
        for (key, value) in parse_env_assignments(&g.custom_env)? {
            set(&mut env, &key, value);
        }
    }
    Ok(env)
}

/// Wraps `rosemary <args>` for the runner, routing through `flatpak-spawn`
/// (which does not forward the environment) when Rosemary lives on the host.
fn wrap(runner: &ResolvedRunner, args: Vec<OsString>, env: Vec<(String, String)>) -> LaunchSpec {
    match runner.source {
        RunnerSource::Host => {
            let mut wrapped: Vec<OsString> = vec!["--host".into(), "--watch-bus".into()];
            wrapped.extend(env.iter().map(|(k, v)| OsString::from(format!("--env={k}={v}"))));
            wrapped.push(runner.program.clone().into_os_string());
            wrapped.extend(args);
            LaunchSpec { program: PathBuf::from("flatpak-spawn"), args: wrapped, env: Vec::new() }
        }
        _ => LaunchSpec { program: runner.program.clone(), args, env },
    }
}

/// `rosemary run --new-instance <CapCut.app>` with the configured environment.
/// `--new-instance` is always passed so a stale claim from a crashed session
/// never turns a launch into a no-op hand-off.
pub fn launch_spec(
    runner: &ResolvedRunner,
    config: &Config,
    prefix: &Path,
    app: &Path,
    wayland: bool,
) -> Result<LaunchSpec, String> {
    let env = rosemary_env(config, prefix, wayland)?;
    let args = vec!["run".into(), "--new-instance".into(), app.as_os_str().to_owned()];
    Ok(wrap(runner, args, env))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixAction {
    Create,
    Repair,
}

/// `rosemary prefix create|repair <prefix>`.
pub fn prefix_spec(runner: &ResolvedRunner, action: PrefixAction, prefix: &Path) -> LaunchSpec {
    let verb = match action {
        PrefixAction::Create => "create",
        PrefixAction::Repair => "repair",
    };
    let args = vec!["prefix".into(), verb.into(), prefix.as_os_str().to_owned()];
    // Fetch the Darwin base without an interactive prompt.
    wrap(runner, args, vec![("ROSEMARY_PREFIX_SETUP".to_owned(), "download".to_owned())])
}

// ---------------------------------------------------------------------------
// Prefix status
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefixStatus {
    /// Not created yet; Rosemary creates it on first launch.
    Missing,
    /// The directory exists but has no Darwin base.
    NoBase,
    /// A Darwin base is installed (with its release tag, if known).
    Ready { base: Option<String> },
}

pub fn prefix_status(prefix: &Path) -> PrefixStatus {
    let state = prefix.join(".rosemary/state.yaml");
    if !prefix.join(".rosemary").is_dir() {
        return PrefixStatus::Missing;
    }
    let Ok(text) = std::fs::read_to_string(state) else {
        return PrefixStatus::NoBase;
    };
    if !text.lines().any(|l| l.trim_end() == "base:") || !prefix.join("root/System").is_dir() {
        return PrefixStatus::NoBase;
    }
    let field = |name: &str| {
        text.lines().filter_map(|l| l.trim().strip_prefix(name)).map(|v| v.trim().trim_matches('"').to_owned()).next()
    };
    let base = match (field("project:"), field("tag:")) {
        (Some(p), Some(t)) => Some(format!("{p} {t}")),
        (p, t) => p.or(t),
    };
    PrefixStatus::Ready { base }
}

// ---------------------------------------------------------------------------
// Process lifecycle
// ---------------------------------------------------------------------------

/// Spawns `spec` in its own process group with stdout/stderr appended
/// directly to the log file. No pipes are involved, so the child keeps running
/// (and logging) safely even after the launcher exits.
pub fn spawn_logged(spec: &LaunchSpec, log: &Log) -> io::Result<Child> {
    use std::os::unix::process::CommandExt;
    let out = OpenOptions::new().create(true).append(true).open(log.path())?;
    let err = out.try_clone()?;
    let mut cmd = spec.command();
    cmd.stdin(Stdio::null()).stdout(out).stderr(err).process_group(0);
    cmd.spawn()
}

/// Whether `pid` is alive and its command line mentions `needle`.
fn process_matches(pid: libc::pid_t, needle: &str) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|bytes| String::from_utf8_lossy(&bytes).contains(needle))
        .unwrap_or(false)
}

/// A running CapCut session. Dropping it does not stop CapCut.
pub struct Session {
    pgid: libc::pid_t,
    exited: Arc<AtomicBool>,
}

impl Session {
    /// Spawns the session; `on_exit` runs on a worker thread when it ends.
    pub fn start(
        spec: &LaunchSpec,
        log: &Log,
        on_exit: impl FnOnce(io::Result<ExitStatus>) + Send + 'static,
    ) -> io::Result<Self> {
        let mut child = spawn_logged(spec, log)?;
        let pgid = child.id() as libc::pid_t;
        let exited = Arc::new(AtomicBool::new(false));
        let flag = exited.clone();
        std::thread::spawn(move || {
            let status = child.wait();
            flag.store(true, Ordering::SeqCst);
            on_exit(status);
        });
        Ok(Self { pgid, exited })
    }

    /// Re-attaches to a session started by an earlier launcher run, if
    /// `pid` is still alive and running `needle` (e.g. the bundle path).
    /// `on_exit` fires once the process disappears.
    pub fn adopt(pid: libc::pid_t, needle: &str, on_exit: impl FnOnce() + Send + 'static) -> Option<Self> {
        if pid <= 1 || !process_matches(pid, needle) {
            return None;
        }
        let exited = Arc::new(AtomicBool::new(false));
        let flag = exited.clone();
        let needle = needle.to_owned();
        std::thread::spawn(move || {
            while process_matches(pid, &needle) {
                std::thread::sleep(Duration::from_secs(1));
            }
            flag.store(true, Ordering::SeqCst);
            on_exit();
        });
        Some(Self { pgid: pid, exited })
    }

    pub fn pid(&self) -> libc::pid_t {
        self.pgid
    }

    pub fn is_running(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }

    /// SIGTERM the whole process group (CEF helpers, `rosemary server`), then
    /// SIGKILL whatever is left after `grace`.
    pub fn stop(&self, grace: Duration) {
        if !self.is_running() {
            return;
        }
        let (pgid, exited) = (self.pgid, self.exited.clone());
        // SAFETY: signalling our own session's process group; `exited` tracks
        // whether the leader is gone so a recycled id is not targeted later.
        unsafe { libc::kill(-pgid, libc::SIGTERM) };
        std::thread::spawn(move || {
            let step = Duration::from_millis(100);
            let mut waited = Duration::ZERO;
            while waited < grace {
                if exited.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(step);
                waited += step;
            }
            if !exited.load(Ordering::SeqCst) {
                // SAFETY: as above.
                unsafe { libc::kill(-pgid, libc::SIGKILL) };
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Decorations, PresentMode};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn runner() -> ResolvedRunner {
        ResolvedRunner { program: PathBuf::from("/usr/bin/rosemary"), source: RunnerSource::System }
    }

    fn args(spec: &LaunchSpec) -> Vec<String> {
        spec.args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn default_launch_command_and_environment() {
        let config = Config::default();
        let spec = launch_spec(&runner(), &config, Path::new("/p"), Path::new("/d/CapCut.app"), true).unwrap();
        assert_eq!(spec.program, PathBuf::from("/usr/bin/rosemary"));
        assert_eq!(args(&spec), ["run", "--new-instance", "/d/CapCut.app"]);
        assert_eq!(spec.env_value("ROSEMARY_PREFIX"), Some("/p"));
        assert_eq!(spec.env_value("ROSEMARY_DECORATIONS"), Some("server"));
        assert_eq!(spec.env_value("ROSEMARY_MENU"), Some("none"), "menu hidden on Wayland");
        assert_eq!(spec.env_value("ROSEMARY_KEEP_FAULT_HANDLER"), Some("1"));
        assert_eq!(spec.env_value("ROSEMARY_CONTROL_EDIT_KEYS"), Some("1"));
        assert_eq!(spec.env_value("ROSEMARY_METAL_PRESENTATION_MODE"), None);
        assert_eq!(spec.env_value("ROSEMARY_UNBOUND_DETAIL"), None);

        // On X11 the menu placement is left to Rosemary unless chosen.
        let x11 = launch_spec(&runner(), &config, Path::new("/p"), Path::new("/d/CapCut.app"), false).unwrap();
        assert_eq!(x11.env_value("ROSEMARY_MENU"), None);
        assert_eq!(args(&x11), ["run", "--new-instance", "/d/CapCut.app"]);
    }

    #[test]
    fn graphics_settings_map_to_rosemary_variables() {
        let mut config = Config::default();
        config.graphics.decorations = Decorations::Client;
        config.graphics.present_mode = PresentMode::Mailbox;
        config.graphics.keep_fault_handler = false;
        config.graphics.ctrl_edit_shortcuts = false;
        config.graphics.menu = crate::config::MenuPlacement::Strip;
        let env = rosemary_env(&config, Path::new("/p"), true).unwrap();
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("ROSEMARY_DECORATIONS"), Some("client"));
        assert_eq!(get("ROSEMARY_METAL_PRESENTATION_MODE"), Some("mailbox"));
        assert_eq!(get("ROSEMARY_KEEP_FAULT_HANDLER"), None);
        assert_eq!(get("ROSEMARY_CONTROL_EDIT_KEYS"), None);
        assert_eq!(get("ROSEMARY_MENU"), Some("strip"));
    }

    #[test]
    fn custom_env_applies_only_in_advanced_mode_and_overrides() {
        let mut config = Config::default();
        config.graphics.custom_env = "ROSEMARY_DECORATIONS=client  ROSEMARY_MENU=strip\nQT_DEBUG=\"a b\"".into();
        config.runtime.log_all_unbound_imports = true;
        let env = rosemary_env(&config, Path::new("/p"), true).unwrap();
        // Without advanced mode the custom value is ignored: the Wayland default stands.
        assert!(env.contains(&("ROSEMARY_MENU".into(), "none".into())));
        assert!(!env.iter().any(|(k, _)| k == "ROSEMARY_UNBOUND_DETAIL"));

        config.launcher.advanced = true;
        let env = rosemary_env(&config, Path::new("/p"), true).unwrap();
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("ROSEMARY_DECORATIONS"), Some("client"), "custom value overrides");
        assert_eq!(env.iter().filter(|(k, _)| k == "ROSEMARY_DECORATIONS").count(), 1);
        assert_eq!(get("ROSEMARY_MENU"), Some("strip"), "custom value overrides the default");
        assert_eq!(env.iter().filter(|(k, _)| k == "ROSEMARY_MENU").count(), 1);
        assert_eq!(get("QT_DEBUG"), Some("a b"));
        assert_eq!(get("ROSEMARY_UNBOUND_DETAIL"), Some("all"));

        config.graphics.custom_env = "1BAD=x".into();
        assert!(rosemary_env(&config, Path::new("/p"), true).is_err());
    }

    #[test]
    fn parses_env_assignments() {
        assert_eq!(parse_env_assignments("").unwrap(), vec![]);
        assert_eq!(
            parse_env_assignments(" A=1; B='x y'  C=\"q\\\"d\" D= E=a=b F=\\ z").unwrap(),
            vec![
                ("A".into(), "1".into()),
                ("B".into(), "x y".into()),
                ("C".into(), "q\"d".into()),
                ("D".into(), "".into()),
                ("E".into(), "a=b".into()),
                ("F".into(), " z".into()),
            ]
        );
        assert!(parse_env_assignments("NOVALUE").is_err());
        assert!(parse_env_assignments("A='open").is_err());
        assert!(parse_env_assignments("A-B=1").is_err());
        assert!(parse_env_assignments("=1").is_err());
    }

    #[test]
    fn flatpak_host_runner_passes_env_as_arguments() {
        let host = ResolvedRunner { program: PathBuf::from("rosemary"), source: RunnerSource::Host };
        let spec = launch_spec(&host, &Config::default(), Path::new("/p"), Path::new("/a/CapCut.app"), true).unwrap();
        assert_eq!(spec.program, PathBuf::from("flatpak-spawn"));
        assert!(spec.env.is_empty());
        let a = args(&spec);
        assert_eq!(&a[..2], ["--host", "--watch-bus"]);
        assert!(a.contains(&"--env=ROSEMARY_DECORATIONS=server".to_owned()));
        assert!(a.contains(&"--env=ROSEMARY_MENU=none".to_owned()));
        assert!(a.contains(&"--env=ROSEMARY_KEEP_FAULT_HANDLER=1".to_owned()));
        assert_eq!(&a[a.len() - 4..], ["rosemary", "run", "--new-instance", "/a/CapCut.app"]);
    }

    #[test]
    fn prefix_commands() {
        let spec = prefix_spec(&runner(), PrefixAction::Repair, Path::new("/p"));
        assert_eq!(args(&spec), ["prefix", "repair", "/p"]);
        assert_eq!(spec.env_value("ROSEMARY_PREFIX_SETUP"), Some("download"));
        let spec = prefix_spec(&runner(), PrefixAction::Create, Path::new("/p q"));
        assert_eq!(args(&spec), ["prefix", "create", "/p q"]);
        assert_eq!(spec.display(), "ROSEMARY_PREFIX_SETUP=download /usr/bin/rosemary prefix create '/p q'");
    }

    fn executable(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn resolves_runners_in_priority_order() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let mut env = RunnerEnv {
            exe_dir: Some(d.join("app/bin")),
            path_var: Some(d.join("usr/bin").into_os_string()),
            home: d.join("home"),
            in_flatpak: false,
        };
        let mut config = Config::default();
        assert!(resolve_runner(&config, &env).is_err());

        executable(&d.join("home/.local/bin/rosemary"));
        assert_eq!(resolve_runner(&config, &env).unwrap().source, RunnerSource::System);
        executable(&d.join("usr/bin/rosemary"));
        assert_eq!(resolve_runner(&config, &env).unwrap().program, d.join("usr/bin/rosemary"));
        executable(&d.join("app/lib/cutmary/rosemary"));
        let bundled = resolve_runner(&config, &env).unwrap();
        assert_eq!(bundled.source, RunnerSource::Bundled);
        assert_eq!(bundled.program, d.join("app/lib/cutmary/rosemary"));

        config.runtime.runner = RunnerChoice::Custom;
        assert!(resolve_runner(&config, &env).is_err());
        config.runtime.custom_runner = Some(d.join("usr/bin/rosemary"));
        assert_eq!(resolve_runner(&config, &env).unwrap().source, RunnerSource::Custom);

        config.runtime.runner = RunnerChoice::Bundled;
        env.exe_dir = Some(d.join("elsewhere"));
        assert!(resolve_runner(&config, &env).is_err());

        config.runtime.runner = RunnerChoice::Auto;
        env.path_var = None;
        env.home = d.join("nohome");
        env.in_flatpak = true;
        assert_eq!(resolve_runner(&config, &env).unwrap().source, RunnerSource::Host);
    }

    #[test]
    fn reads_prefix_status() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        assert_eq!(prefix_status(p), PrefixStatus::Missing);
        fs::create_dir_all(p.join(".rosemary")).unwrap();
        assert_eq!(prefix_status(p), PrefixStatus::NoBase);
        fs::write(
            p.join(".rosemary/state.yaml"),
            "version: 1\nbase:\n  project: \"minidarwin\"\n  arch: \"x86_64\"\n  tag: \"v21.0.0\"\n",
        )
        .unwrap();
        assert_eq!(prefix_status(p), PrefixStatus::NoBase, "root/System missing");
        fs::create_dir_all(p.join("root/System")).unwrap();
        assert_eq!(prefix_status(p), PrefixStatus::Ready { base: Some("minidarwin v21.0.0".into()) });
    }

    #[test]
    fn session_logs_output_and_reports_exit() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::open(dir.path().join("latest.log"));
        let spec = LaunchSpec {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "echo out-$CUTMARY_T; echo err >&2; exit 3".into()],
            env: vec![("CUTMARY_T".into(), "ok".into())],
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let session = Session::start(&spec, &log, move |status| tx.send(status.unwrap().code()).unwrap()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), Some(3));
        assert!(!session.is_running());
        std::thread::sleep(Duration::from_millis(100));
        let text = fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("out-ok") && text.contains("err"), "{text}");
    }

    #[test]
    fn adopts_only_matching_live_processes() {
        // `; true` keeps sh from exec'ing sleep, so its argv ($0) stays visible.
        let mut child = Command::new("/bin/sh").args(["-c", "sleep 30; true", "cutmary-adopt-test"]).spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        // Until exec completes, /proc shows the parent's argv.
        for _ in 0..100 {
            if process_matches(pid, "cutmary-adopt-test") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(Session::adopt(pid, "not-this-process", || {}).is_none());
        let (tx, rx) = std::sync::mpsc::channel();
        let session = Session::adopt(pid, "cutmary-adopt-test", move || tx.send(()).unwrap()).unwrap();
        assert!(session.is_running());
        child.kill().unwrap();
        child.wait().unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!session.is_running());
    }

    #[test]
    fn session_stop_terminates_the_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::open(dir.path().join("latest.log"));
        let spec = LaunchSpec {
            program: PathBuf::from("/bin/sh"),
            // A grandchild in the same group must die too.
            args: vec!["-c".into(), "sleep 30 & sleep 30; wait".into()],
            env: vec![],
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let session = Session::start(&spec, &log, move |status| tx.send(status.unwrap()).unwrap()).unwrap();
        assert!(session.is_running());
        session.stop(Duration::from_secs(2));
        let status = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(!status.success());
        // The group is gone: signalling it fails with ESRCH.
        std::thread::sleep(Duration::from_millis(200));
        // SAFETY: probing with signal 0 has no effect.
        assert_eq!(unsafe { libc::kill(-session.pgid, 0) }, -1);
    }
}
