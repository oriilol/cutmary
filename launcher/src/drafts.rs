//! CapCut project draft maintenance.
//!
//! When CapCut dies (SIGABRT/SIGSEGV) it leaves 0-byte `*.locked` markers in
//! its draft folders (`.../Projects/com.lveditor.draft/<draft>/`), and the next
//! session refuses to open those projects. Cutmary removes such stale locks
//! before each launch. "Repair Drafts" also restores a missing or empty
//! `draft_info.json` from CapCut's own `draft_info.json.bak`, and offers to
//! archive drafts that cannot be restored. Nothing is ever deleted except
//! 0-byte lock markers, and nothing is touched while any CapCut instance runs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The draft's project file; a draft without it cannot be opened.
pub const DRAFT_INFO: &str = "draft_info.json";
/// CapCut's own previous copy of the project file.
pub const DRAFT_INFO_BACKUP: &str = "draft_info.json.bak";
const LOCK_SUFFIX: &str = ".locked";
/// Lock markers sit in the draft folder or a level or two below it.
const MAX_LOCK_DEPTH: usize = 3;
/// Sibling of `com.lveditor.draft`, so CapCut no longer lists archived drafts.
pub const ARCHIVE_DIR: &str = "Cutmary Archived Drafts";

/// Draft folders: visible, real (non-symlink) subdirectories of `drafts_root`.
pub fn draft_dirs(drafts_root: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(drafts_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry?;
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if !hidden && entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn collect_locks(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() && depth < MAX_LOCK_DEPTH {
            collect_locks(&entry.path(), depth + 1, out)?;
        } else if file_type.is_file() && entry.file_name().to_string_lossy().ends_with(LOCK_SUFFIX) {
            out.push(entry.path());
        }
    }
    Ok(())
}

/// Every `*.locked` regular file inside the draft folders.
pub fn find_locks(drafts_root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut locks = Vec::new();
    for dir in draft_dirs(drafts_root)? {
        collect_locks(&dir, 1, &mut locks)?;
    }
    locks.sort();
    Ok(locks)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LockCleanup {
    /// 0-byte lock markers that were removed.
    pub removed: Vec<PathBuf>,
    /// Lock files with content, left alone because they are not plain markers.
    pub kept: Vec<PathBuf>,
}

/// Removes stale 0-byte `*.locked` markers. Only call this while no CapCut
/// instance is running (see [`capcut_running`]).
pub fn clean_stale_locks(drafts_root: &Path) -> io::Result<LockCleanup> {
    let mut cleanup = LockCleanup::default();
    for lock in find_locks(drafts_root)? {
        if fs::symlink_metadata(&lock)?.len() == 0 {
            fs::remove_file(&lock)?;
            cleanup.removed.push(lock);
        } else {
            cleanup.kept.push(lock);
        }
    }
    Ok(cleanup)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damage {
    MissingInfo,
    EmptyInfo,
}

impl Damage {
    pub fn describe(self) -> &'static str {
        match self {
            Self::MissingInfo => "draft_info.json is missing",
            Self::EmptyInfo => "draft_info.json is empty",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokenDraft {
    pub path: PathBuf,
    pub damage: Damage,
    /// CapCut's `draft_info.json.bak` is present and non-empty.
    pub has_backup: bool,
}

impl BrokenDraft {
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DraftReport {
    pub drafts: usize,
    pub stale_locks: usize,
    pub broken: Vec<BrokenDraft>,
}

/// CapCut's non-empty backup of the project file, if any.
pub fn backup_of(draft: &Path) -> Option<PathBuf> {
    let backup = draft.join(DRAFT_INFO_BACKUP);
    fs::symlink_metadata(&backup).is_ok_and(|m| m.is_file() && m.len() > 0).then_some(backup)
}

/// Restores a damaged draft's `draft_info.json` from CapCut's backup. The
/// backup is copied (never moved), and only into a missing or empty slot.
/// Returns whether the draft was restored.
pub fn restore_from_backup(draft: &Path) -> io::Result<bool> {
    let (Some(_), Some(backup)) = (inspect(draft), backup_of(draft)) else {
        return Ok(false);
    };
    let tmp = draft.join(".draft_info.json.cutmary-restore");
    fs::copy(&backup, &tmp)?;
    fs::rename(&tmp, draft.join(DRAFT_INFO))?;
    Ok(true)
}

pub fn inspect(path: &Path) -> Option<Damage> {
    match fs::metadata(path.join(DRAFT_INFO)) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => None,
        Ok(meta) if meta.is_file() => Some(Damage::EmptyInfo),
        _ => Some(Damage::MissingInfo),
    }
}

/// Counts drafts, 0-byte lock markers and drafts that cannot be opened.
pub fn scan(drafts_root: &Path) -> io::Result<DraftReport> {
    let dirs = draft_dirs(drafts_root)?;
    let stale_locks =
        find_locks(drafts_root)?.iter().filter(|lock| fs::symlink_metadata(lock).is_ok_and(|m| m.len() == 0)).count();
    let broken = dirs
        .iter()
        .filter_map(|path| {
            inspect(path).map(|damage| BrokenDraft {
                path: path.clone(),
                damage,
                has_backup: backup_of(path).is_some(),
            })
        })
        .collect();
    Ok(DraftReport { drafts: dirs.len(), stale_locks, broken })
}

/// The archive folder next to `com.lveditor.draft`.
pub fn archive_root(drafts_root: &Path) -> PathBuf {
    drafts_root.parent().unwrap_or(drafts_root).join(ARCHIVE_DIR)
}

/// Moves damaged drafts out of CapCut's view into [`archive_root`]. Only
/// direct children of `drafts_root` that are still damaged are moved; a name
/// clash gets a numeric suffix. Returns the new locations.
pub fn archive(drafts_root: &Path, drafts: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let archive = archive_root(drafts_root);
    let mut moved = Vec::new();
    for draft in drafts {
        let is_child = draft.parent() == Some(drafts_root);
        let is_dir = fs::symlink_metadata(draft).is_ok_and(|m| m.is_dir());
        if !is_child || !is_dir || inspect(draft).is_none() {
            continue;
        }
        fs::create_dir_all(&archive)?;
        let name = draft.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let mut target = archive.join(&name);
        let mut n = 2;
        while fs::symlink_metadata(&target).is_ok() {
            target = archive.join(format!("{name} ({n})"));
            n += 1;
        }
        fs::rename(draft, &target)?;
        moved.push(target);
    }
    Ok(moved)
}

/// Whether any process other than `except` has a `CapCut.app` bundle on its
/// command line (a Rosemary-run CapCut, its helpers, or our flatpak-spawn).
pub fn capcut_running(except: u32) -> bool {
    capcut_running_in(Path::new("/proc"), except)
}

fn capcut_running_in(proc_root: &Path, except: u32) -> bool {
    let Ok(entries) = fs::read_dir(proc_root) else { return false };
    entries.flatten().any(|entry| {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            return false;
        };
        pid != except
            && fs::read(entry.path().join("cmdline"))
                .is_ok_and(|cmdline| cmdline.split(|b| *b == 0).any(|arg| arg.windows(10).any(|w| w == b"CapCut.app")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(root: &Path, name: &str, info: Option<&str>) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        if let Some(info) = info {
            fs::write(dir.join(DRAFT_INFO), info).unwrap();
        }
        dir
    }

    #[test]
    fn missing_drafts_folder_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("com.lveditor.draft");
        assert_eq!(scan(&root).unwrap(), DraftReport::default());
        assert_eq!(clean_stale_locks(&root).unwrap(), LockCleanup::default());
    }

    #[test]
    fn removes_only_zero_byte_locks_inside_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("com.lveditor.draft");
        let a = draft(&root, "Vacation", Some("{}"));
        let b = draft(&root, "Promo 2026", Some("{}"));
        fs::write(a.join(".locked"), b"").unwrap();
        fs::write(a.join("draft_info.json.locked"), b"").unwrap();
        fs::create_dir_all(b.join("Resources/audio")).unwrap();
        fs::write(b.join("Resources/audio/track.locked"), b"").unwrap();
        // A lock with content is not a bare marker: keep it.
        fs::write(b.join("render.locked"), b"pid 123").unwrap();
        // Locks outside draft folders, in hidden folders, or symlinked are ignored.
        fs::write(root.join("root.locked"), b"").unwrap();
        fs::create_dir_all(root.join(".recycle_bin/Old")).unwrap();
        fs::write(root.join(".recycle_bin/Old/.locked"), b"").unwrap();
        fs::write(dir.path().join("outside.locked"), b"").unwrap();
        std::os::unix::fs::symlink(dir.path().join("outside.locked"), a.join("link.locked")).unwrap();

        let report = scan(&root).unwrap();
        assert_eq!(report.drafts, 2);
        assert_eq!(report.stale_locks, 3);

        let cleanup = clean_stale_locks(&root).unwrap();
        assert_eq!(cleanup.removed.len(), 3);
        assert_eq!(cleanup.kept, vec![b.join("render.locked")]);
        assert!(!a.join(".locked").exists());
        assert!(!a.join("draft_info.json.locked").exists());
        assert!(!b.join("Resources/audio/track.locked").exists());
        assert!(b.join("render.locked").exists());
        assert!(root.join("root.locked").exists());
        assert!(root.join(".recycle_bin/Old/.locked").exists());
        assert!(dir.path().join("outside.locked").exists());
        assert!(fs::symlink_metadata(a.join("link.locked")).is_ok());
        // Project data is untouched.
        assert_eq!(fs::read_to_string(a.join(DRAFT_INFO)).unwrap(), "{}");
        assert_eq!(scan(&root).unwrap().stale_locks, 0);
    }

    #[test]
    fn detects_and_archives_only_damaged_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("Projects");
        let root = projects.join("com.lveditor.draft");
        let good = draft(&root, "Good", Some(r#"{"tracks":[]}"#));
        let missing = draft(&root, "Crashed", None);
        fs::write(missing.join("draft_meta_info.json"), b"{}").unwrap();
        let empty = draft(&root, "Half Saved", Some(""));
        fs::write(root.join("root_meta_info.json"), b"{}").unwrap();

        let report = scan(&root).unwrap();
        assert_eq!(report.drafts, 3);
        assert_eq!(
            report.broken,
            vec![
                BrokenDraft { path: missing.clone(), damage: Damage::MissingInfo, has_backup: false },
                BrokenDraft { path: empty.clone(), damage: Damage::EmptyInfo, has_backup: false },
            ]
        );
        assert_eq!(report.broken[0].name(), "Crashed");

        // A clash in the archive gets a suffix; healthy or foreign paths are refused.
        fs::create_dir_all(archive_root(&root).join("Crashed")).unwrap();
        let outside = draft(dir.path(), "Elsewhere", None);
        let moved = archive(&root, &[missing.clone(), empty.clone(), good.clone(), outside.clone()]).unwrap();
        assert_eq!(
            moved,
            vec![projects.join(ARCHIVE_DIR).join("Crashed (2)"), projects.join(ARCHIVE_DIR).join("Half Saved"),]
        );
        assert!(moved[0].join("draft_meta_info.json").exists(), "contents moved intact");
        assert!(good.join(DRAFT_INFO).exists());
        assert!(outside.exists());
        let after = scan(&root).unwrap();
        assert_eq!(after.drafts, 1);
        assert!(after.broken.is_empty());
    }

    #[test]
    fn restores_damaged_drafts_from_capcut_backup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("com.lveditor.draft");
        let missing = draft(&root, "Missing", None);
        fs::write(missing.join(DRAFT_INFO_BACKUP), r#"{"v":1}"#).unwrap();
        let empty = draft(&root, "Empty", Some(""));
        fs::write(empty.join(DRAFT_INFO_BACKUP), r#"{"v":2}"#).unwrap();
        let empty_backup = draft(&root, "EmptyBackup", None);
        fs::write(empty_backup.join(DRAFT_INFO_BACKUP), b"").unwrap();
        let healthy = draft(&root, "Healthy", Some(r#"{"v":"current"}"#));
        fs::write(healthy.join(DRAFT_INFO_BACKUP), r#"{"v":"old"}"#).unwrap();

        let report = scan(&root).unwrap();
        let flags: Vec<_> = report.broken.iter().map(|d| (d.name(), d.has_backup)).collect();
        assert_eq!(flags, [("Empty".into(), true), ("EmptyBackup".into(), false), ("Missing".into(), true)]);

        assert!(restore_from_backup(&missing).unwrap());
        assert!(restore_from_backup(&empty).unwrap());
        assert!(!restore_from_backup(&empty_backup).unwrap(), "an empty backup is not a restore source");
        assert!(!restore_from_backup(&healthy).unwrap(), "healthy drafts are never overwritten");
        assert_eq!(fs::read_to_string(missing.join(DRAFT_INFO)).unwrap(), r#"{"v":1}"#);
        assert_eq!(fs::read_to_string(empty.join(DRAFT_INFO)).unwrap(), r#"{"v":2}"#);
        assert_eq!(fs::read_to_string(healthy.join(DRAFT_INFO)).unwrap(), r#"{"v":"current"}"#);
        assert!(missing.join(DRAFT_INFO_BACKUP).exists(), "backup is copied, not moved");
        assert!(!missing.join(".draft_info.json.cutmary-restore").exists());
        let after = scan(&root).unwrap();
        assert_eq!(after.broken.len(), 1);
        assert_eq!(after.broken[0].name(), "EmptyBackup");
    }

    #[test]
    fn detects_running_capcut_from_proc() {
        let dir = tempfile::tempdir().unwrap();
        let proc_root = dir.path();
        let process = |pid: u32, argv: &[&str]| {
            fs::create_dir_all(proc_root.join(pid.to_string())).unwrap();
            fs::write(proc_root.join(format!("{pid}/cmdline")), argv.join("\0")).unwrap();
        };
        process(10, &["/usr/bin/cutmary"]);
        process(11, &["bash"]);
        fs::create_dir_all(proc_root.join("self")).unwrap();
        assert!(!capcut_running_in(proc_root, 0));

        process(12, &["/usr/bin/rosemary", "run", "--new-instance", "/home/u/.local/share/cutmary/CapCut.app"]);
        assert!(capcut_running_in(proc_root, 0));
        assert!(!capcut_running_in(proc_root, 12), "the excluded pid is ignored");
        // The real /proc works too (this test process is not CapCut).
        let _ = capcut_running(std::process::id());
    }
}
