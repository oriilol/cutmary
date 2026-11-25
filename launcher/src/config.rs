//! XDG paths and the persistent TOML configuration (`~/.config/cutmary/config.toml`).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const APP_ID: &str = "org.cutmary.Cutmary";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPOSITORY: &str = "oriilol/cutmary";

/// Every directory the launcher reads or writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME/cutmary`
    pub config_dir: PathBuf,
    /// `$XDG_DATA_HOME/cutmary`
    pub data_dir: PathBuf,
    /// `$XDG_CACHE_HOME/cutmary`
    pub cache_dir: PathBuf,
    /// Rosemary's own default prefix, which always lives on the host.
    pub default_prefix: PathBuf,
    pub in_flatpak: bool,
}

impl Paths {
    pub fn from_env() -> Self {
        Self::resolve(|key| std::env::var_os(key), Path::new("/.flatpak-info").exists())
    }

    /// Resolves paths from an environment lookup, so tests can inject one.
    pub fn resolve(get: impl Fn(&str) -> Option<OsString>, in_flatpak: bool) -> Self {
        let absolute = |key: &str| get(key).map(PathBuf::from).filter(|p| p.is_absolute());
        let home = absolute("HOME").unwrap_or_else(|| PathBuf::from("/tmp"));
        let xdg = |key: &str, fallback: &str| absolute(key).unwrap_or_else(|| home.join(fallback));

        // Inside a Flatpak, XDG_DATA_HOME is the sandbox's private directory, but
        // Rosemary's prefix belongs to the host (where `rosemary` runs). Flatpak
        // exports the host's value as HOST_XDG_DATA_HOME when the user set one.
        let rosemary_data_home = if in_flatpak {
            absolute("HOST_XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"))
        } else {
            xdg("XDG_DATA_HOME", ".local/share")
        };

        Self {
            config_dir: xdg("XDG_CONFIG_HOME", ".config").join("cutmary"),
            data_dir: xdg("XDG_DATA_HOME", ".local/share").join("cutmary"),
            cache_dir: xdg("XDG_CACHE_HOME", ".cache").join("cutmary"),
            default_prefix: rosemary_data_home.join("rosemary/prefixes/default"),
            home,
            in_flatpak,
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn latest_log(&self) -> PathBuf {
        self.logs_dir().join("latest.log")
    }

    /// Where the prepared `CapCut.app` bundle lives.
    pub fn app_bundle(&self) -> PathBuf {
        self.data_dir.join("CapCut.app")
    }

    /// Scratch directory used while extracting a DMG.
    pub fn staging_dir(&self) -> PathBuf {
        self.data_dir.join("staging")
    }

    /// Metadata written after a successful install.
    pub fn install_record(&self) -> PathBuf {
        self.data_dir.join("install.toml")
    }
}

/// A closed set of choices shown in an `AdwComboRow`.
pub trait Choice: Copy + PartialEq + 'static {
    const ALL: &'static [Self];
    fn label(self) -> &'static str;

    fn index(self) -> u32 {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0) as u32
    }

    fn from_index(index: u32) -> Self {
        Self::ALL.get(index as usize).copied().unwrap_or(Self::ALL[0])
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    #[default]
    Dark,
    Light,
}

impl Choice for Theme {
    const ALL: &'static [Self] = &[Self::System, Self::Dark, Self::Light];
    fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerChoice {
    #[default]
    Auto,
    Bundled,
    Custom,
}

impl Choice for RunnerChoice {
    const ALL: &'static [Self] = &[Self::Auto, Self::Bundled, Self::Custom];
    fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto-detected",
            Self::Bundled => "Bundled",
            Self::Custom => "Custom",
        }
    }
}

/// `ROSEMARY_DECORATIONS`: who draws the titlebar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decorations {
    /// The host window manager draws it (`server`).
    #[default]
    Native,
    /// Rosemary draws a macOS-style titlebar (`client`).
    Client,
}

impl Decorations {
    pub fn env_value(self) -> &'static str {
        match self {
            Self::Native => "server",
            Self::Client => "client",
        }
    }
}

impl Choice for Decorations {
    const ALL: &'static [Self] = &[Self::Native, Self::Client];
    fn label(self) -> &'static str {
        match self {
            Self::Native => "Native (Recommended)",
            Self::Client => "Client (macOS)",
        }
    }
}

/// `ROSEMARY_METAL_PRESENTATION_MODE`: the Vulkan present mode of the Metal translator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentMode {
    #[default]
    Default,
    Fifo,
    FifoRelaxed,
    Mailbox,
    Immediate,
}

impl PresentMode {
    /// The env value, or `None` to let Rosemary pick.
    pub fn env_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Fifo => Some("fifo"),
            Self::FifoRelaxed => Some("fifo_relaxed"),
            Self::Mailbox => Some("mailbox"),
            Self::Immediate => Some("immediate"),
        }
    }
}

impl Choice for PresentMode {
    const ALL: &'static [Self] =
        &[Self::Default, Self::Fifo, Self::FifoRelaxed, Self::Mailbox, Self::Immediate];
    fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Fifo => "FIFO (V-Sync)",
            Self::FifoRelaxed => "FIFO Relaxed",
            Self::Mailbox => "Mailbox",
            Self::Immediate => "Immediate",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherSection {
    pub theme: Theme,
    pub advanced: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CapCutSection {
    /// Strip `arm64` slices from universal Mach-O binaries after extraction.
    pub thin_arm64: bool,
}

impl Default for CapCutSection {
    fn default() -> Self {
        Self { thin_arm64: true }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeSection {
    pub runner: RunnerChoice,
    /// Used when `runner = "custom"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_runner: Option<PathBuf>,
    /// `None` means Rosemary's default prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix_dir: Option<PathBuf>,
    /// Advanced: `ROSEMARY_UNBOUND_DETAIL=all`.
    pub log_all_unbound_imports: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSection {
    pub decorations: Decorations,
    pub present_mode: PresentMode,
    /// `ROSEMARY_KEEP_FAULT_HANDLER=1`
    pub keep_fault_handler: bool,
    /// `ROSEMARY_CONTROL_EDIT_KEYS=1`: Ctrl acts as Command for copy/paste/undo.
    pub ctrl_edit_shortcuts: bool,
    /// Extra `KEY=VALUE` pairs, applied only while advanced settings are on.
    pub custom_env: String,
}

impl Default for GraphicsSection {
    fn default() -> Self {
        Self {
            decorations: Decorations::default(),
            present_mode: PresentMode::default(),
            keep_fault_handler: true,
            ctrl_edit_shortcuts: true,
            custom_env: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub launcher: LauncherSection,
    pub capcut: CapCutSection,
    pub runtime: RuntimeSection,
    pub graphics: GraphicsSection,
}

impl Config {
    /// Loads `path`; a missing file yields the defaults.
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Loads `path`, falling back to defaults on error. An unparsable file is
    /// moved aside to `config.toml.bak` so the next save does not lose it silently.
    pub fn load_or_default(path: &Path) -> (Self, Option<String>) {
        match Self::load(path) {
            Ok(config) => (config, None),
            Err(e) => {
                let backup = path.with_extension("toml.bak");
                let moved = fs::rename(path, &backup).is_ok();
                let note = if moved {
                    format!("invalid config ({e}); moved it to {} and using defaults", backup.display())
                } else {
                    format!("could not read config ({e}); using defaults")
                };
                (Self::default(), Some(note))
            }
        }
    }

    /// Atomically writes the config to `path`.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, format!("# Cutmary launcher settings\n{text}"))?;
        fs::rename(&tmp, path)
    }

    /// The prefix directory in effect.
    pub fn prefix_dir(&self, paths: &Paths) -> PathBuf {
        self.runtime
            .prefix_dir
            .clone()
            .unwrap_or_else(|| paths.default_prefix.clone())
    }
}

/// The guest's `~/Movies/CapCut/.../com.lveditor.draft` projects folder inside a prefix.
pub fn projects_dir(prefix: &Path, user: &str) -> PathBuf {
    prefix
        .join("root/Users")
        .join(user)
        .join("Movies/CapCut/User Data/Projects/com.lveditor.draft")
}

/// The host user name Rosemary maps to `/Users/<name>`.
pub fn current_user() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| std::env::var("LOGNAME").ok())
        .unwrap_or_else(|| "user".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn defaults_match_the_documented_behavior() {
        let c = Config::default();
        assert_eq!(c.launcher.theme, Theme::Dark);
        assert!(!c.launcher.advanced);
        assert!(c.capcut.thin_arm64);
        assert_eq!(c.runtime.runner, RunnerChoice::Auto);
        assert_eq!(c.runtime.prefix_dir, None);
        assert_eq!(c.graphics.decorations, Decorations::Native);
        assert_eq!(c.graphics.present_mode, PresentMode::Default);
        assert!(c.graphics.keep_fault_handler);
        assert!(c.graphics.custom_env.is_empty());
    }

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("nope/config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cutmary/config.toml");
        let mut config = Config::default();
        config.launcher.theme = Theme::Light;
        config.launcher.advanced = true;
        config.capcut.thin_arm64 = false;
        config.runtime.runner = RunnerChoice::Custom;
        config.runtime.custom_runner = Some(PathBuf::from("/opt/rosemary/bin/rosemary"));
        config.runtime.prefix_dir = Some(PathBuf::from("/srv/prefixes/capcut"));
        config.graphics.decorations = Decorations::Client;
        config.graphics.present_mode = PresentMode::FifoRelaxed;
        config.graphics.keep_fault_handler = false;
        config.graphics.custom_env = "FOO=bar BAZ=\"a b\"".into();
        config.save(&path).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("theme = \"light\""), "{text}");
        assert!(text.contains("present_mode = \"fifo_relaxed\""), "{text}");
        assert!(text.contains("decorations = \"client\""), "{text}");
        assert!(!path.with_extension("toml.tmp").exists());
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    #[test]
    fn partial_file_fills_in_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[graphics]\npresent_mode = \"mailbox\"\n").unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.graphics.present_mode, PresentMode::Mailbox);
        assert!(config.graphics.keep_fault_handler);
        assert!(config.capcut.thin_arm64);
        assert_eq!(config.launcher, LauncherSection::default());
    }

    #[test]
    fn invalid_file_is_backed_up_and_defaults_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[graphics]\npresent_mode = \"warp-speed\"\n").unwrap();
        assert!(Config::load(&path).is_err());
        let (config, note) = Config::load_or_default(&path);
        assert_eq!(config, Config::default());
        assert!(note.unwrap().contains("config.toml.bak"));
        assert!(path.with_extension("toml.bak").exists());
        assert!(!path.exists());
    }

    #[test]
    fn xdg_paths_resolve_with_and_without_overrides() {
        let p = Paths::resolve(env(&[("HOME", "/home/u")]), false);
        assert_eq!(p.config_file(), PathBuf::from("/home/u/.config/cutmary/config.toml"));
        assert_eq!(p.latest_log(), PathBuf::from("/home/u/.local/share/cutmary/logs/latest.log"));
        assert_eq!(p.cache_dir, PathBuf::from("/home/u/.cache/cutmary"));
        assert_eq!(p.default_prefix, PathBuf::from("/home/u/.local/share/rosemary/prefixes/default"));

        let p = Paths::resolve(
            env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/cfg"), ("XDG_DATA_HOME", "/data"), ("XDG_CACHE_HOME", "relative")]),
            false,
        );
        assert_eq!(p.config_dir, PathBuf::from("/cfg/cutmary"));
        assert_eq!(p.data_dir, PathBuf::from("/data/cutmary"));
        // Relative XDG values are invalid per spec and ignored.
        assert_eq!(p.cache_dir, PathBuf::from("/home/u/.cache/cutmary"));
        assert_eq!(p.default_prefix, PathBuf::from("/data/rosemary/prefixes/default"));
    }

    #[test]
    fn flatpak_keeps_the_prefix_on_the_host() {
        let p = Paths::resolve(
            env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/home/u/.var/app/org.cutmary.Cutmary/data")]),
            true,
        );
        assert_eq!(p.data_dir, PathBuf::from("/home/u/.var/app/org.cutmary.Cutmary/data/cutmary"));
        assert_eq!(p.default_prefix, PathBuf::from("/home/u/.local/share/rosemary/prefixes/default"));
    }

    #[test]
    fn prefix_and_projects_dirs() {
        let paths = Paths::resolve(env(&[("HOME", "/home/u")]), false);
        let mut config = Config::default();
        assert_eq!(config.prefix_dir(&paths), paths.default_prefix);
        config.runtime.prefix_dir = Some(PathBuf::from("/p"));
        assert_eq!(
            projects_dir(&config.prefix_dir(&paths), "u"),
            PathBuf::from("/p/root/Users/u/Movies/CapCut/User Data/Projects/com.lveditor.draft")
        );
    }

    #[test]
    fn choices_map_to_indices_and_env_values() {
        assert_eq!(PresentMode::from_index(PresentMode::Mailbox.index()), PresentMode::Mailbox);
        assert_eq!(PresentMode::from_index(99), PresentMode::Default);
        assert_eq!(Decorations::Native.env_value(), "server");
        assert_eq!(Decorations::Client.env_value(), "client");
        assert_eq!(PresentMode::Default.env_value(), None);
        assert_eq!(PresentMode::Immediate.env_value(), Some("immediate"));
    }
}
