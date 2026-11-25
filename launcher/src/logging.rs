//! The launcher log at `~/.local/share/cutmary/logs/latest.log`.
//!
//! Launcher messages and Rosemary's stdout/stderr share one file so a bug
//! report needs a single attachment. Each install or launch rotates the
//! previous session to `previous.log`.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Log {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug)]
struct Inner {
    path: PathBuf,
    file: Option<File>,
}

fn open_append(path: &Path) -> Option<File> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

fn timestamp() -> String {
    gtk::glib::DateTime::now_local()
        .and_then(|now| now.format("%Y-%m-%d %H:%M:%S"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

impl Log {
    pub fn open(path: PathBuf) -> Self {
        let file = open_append(&path);
        Self { inner: Arc::new(Mutex::new(Inner { path, file })) }
    }

    pub fn path(&self) -> PathBuf {
        self.lock().path.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Moves `latest.log` to `previous.log` and starts a fresh file.
    pub fn rotate(&self) {
        let mut inner = self.lock();
        inner.file = None;
        let previous = inner.path.with_file_name("previous.log");
        let _ = fs::rename(&inner.path, previous);
        inner.file = open_append(&inner.path);
    }

    /// Writes a timestamped launcher message.
    pub fn info(&self, message: impl AsRef<str>) {
        let line = format!("[{}] cutmary: {}", timestamp(), message.as_ref());
        eprintln!("{line}");
        self.raw(&line);
    }

    /// Writes a line exactly as given (used for Rosemary/7-Zip output).
    pub fn raw(&self, line: &str) {
        let mut inner = self.lock();
        if let Some(file) = inner.file.as_mut() {
            let _ = writeln!(file, "{}", line.trim_end_matches(['\r', '\n']));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_rotates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/latest.log");
        let log = Log::open(path.clone());
        log.info("hello");
        log.raw("rosemary: raw line\n");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("cutmary: hello"));
        assert!(text.ends_with("rosemary: raw line\n"));

        log.rotate();
        log.raw("fresh");
        assert_eq!(fs::read_to_string(&path).unwrap(), "fresh\n");
        assert!(fs::read_to_string(dir.path().join("logs/previous.log")).unwrap().contains("hello"));
    }
}
