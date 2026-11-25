//! Small filesystem and formatting helpers shared by the launcher modules.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

/// Finds an executable called `name` in the colon-separated `path_var`.
pub fn which_in(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let path_var = path_var?;
    std::env::split_paths(path_var).map(|dir| dir.join(name)).find(|candidate| is_executable(candidate))
}

/// Finds an executable called `name` in the current `$PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, std::env::var_os("PATH").as_deref())
}

/// Whether `path` is a regular file (following symlinks) with an execute bit set.
pub fn is_executable(path: &Path) -> bool {
    fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// Lexically normalizes a path: drops `.` and resolves `..` against earlier
/// components without touching the filesystem (like Python's `os.path.normpath`).
pub fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let at_root = out.has_root() && out.parent().is_none();
                // `..` at the root stays at the root; on a relative path that
                // has run out of components it is kept.
                if !at_root && !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Whether `path` lies strictly inside `root`, compared lexically.
pub fn is_strictly_inside(path: &Path, root: &Path) -> bool {
    let path = normalize_lexically(path);
    let root = normalize_lexically(root);
    path != root && path.starts_with(&root)
}

/// Total size in bytes of every file under `root`, without following symlinks.
pub fn dir_size(root: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(root) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    let mut total = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

/// Recursively lists regular files under `root` without following symlinks.
pub fn regular_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Removes `path` whether it is a file, a symlink or a directory tree.
pub fn remove_any(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Formats a byte count with binary units, e.g. `1.1 GiB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// Library directories searched when `ldconfig -p` is unavailable.
pub const LIBRARY_DIRS: &[&str] = &[
    "/usr/lib",
    "/usr/lib64",
    "/usr/lib/x86_64-linux-gnu",
    "/lib",
    "/lib64",
    "/lib/x86_64-linux-gnu",
    "/usr/local/lib",
];

/// Finds `soname` in `ldconfig -p` output, returning the library path.
pub fn find_in_ldconfig(listing: &str, soname: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (left, path) = line.split_once(" => ")?;
        let name = left.split_whitespace().next()?;
        // x86_64 entries only: an i686 copy cannot be loaded by Rosemary.
        let wrong_arch = left.contains("(libc6)") || left.contains("i386");
        (name == soname && !wrong_arch).then(|| path.trim().to_owned())
    })
}

/// Whether the desktop session is Wayland, from an environment lookup.
pub fn is_wayland(get: impl Fn(&str) -> Option<String>) -> bool {
    get("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || get("XDG_SESSION_TYPE").is_some_and(|v| v.eq_ignore_ascii_case("wayland"))
}

/// Whether the current desktop session is Wayland.
pub fn is_wayland_session() -> bool {
    is_wayland(|k| std::env::var(k).ok())
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_like_python_normpath() {
        assert_eq!(normalize_lexically(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
        assert_eq!(normalize_lexically(Path::new("/a/../../b")), PathBuf::from("/b"));
        assert_eq!(normalize_lexically(Path::new("a/../../b")), PathBuf::from("../b"));
        assert!(is_strictly_inside(Path::new("/app/x/../y"), Path::new("/app")));
        assert!(!is_strictly_inside(Path::new("/app/../etc"), Path::new("/app")));
        assert!(!is_strictly_inside(Path::new("/app"), Path::new("/app")));
        assert!(!is_strictly_inside(Path::new("/application"), Path::new("/app")));
    }

    #[test]
    fn finds_libraries_in_ldconfig_output() {
        let listing = "1234 libs found in cache `/etc/ld.so.cache'
\tlibwebp.so.7 (libc6,x86-64) => /usr/lib/libwebp.so.7
\tlibturbojpeg.so.0 (libc6) => /usr/lib32/libturbojpeg.so.0
\tlibturbojpeg.so.0 (libc6,x86-64) => /usr/lib/libturbojpeg.so.0
\tlibSDL3.so.0 (libc6) => /usr/lib32/libSDL3.so.0
";
        assert_eq!(find_in_ldconfig(listing, "libwebp.so.7").as_deref(), Some("/usr/lib/libwebp.so.7"));
        assert_eq!(find_in_ldconfig(listing, "libturbojpeg.so.0").as_deref(), Some("/usr/lib/libturbojpeg.so.0"));
        assert_eq!(find_in_ldconfig(listing, "libSDL3.so.0"), None, "32-bit only");
        assert_eq!(find_in_ldconfig(listing, "libwebp.so"), None);
    }

    #[test]
    fn detects_wayland_sessions() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(key, _)| *key == k).map(|(_, v)| v.to_string())
        };
        assert!(is_wayland(env(&[("WAYLAND_DISPLAY", "wayland-0")])));
        assert!(is_wayland(env(&[("XDG_SESSION_TYPE", "wayland")])));
        assert!(!is_wayland(env(&[("XDG_SESSION_TYPE", "x11"), ("WAYLAND_DISPLAY", "")])));
        assert!(!is_wayland(env(&[])));
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1_203_000_000), "1.1 GiB");
    }

    #[test]
    fn lists_files_and_sizes_without_following_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        fs::write(dir.path().join("a/one"), [0u8; 10]).unwrap();
        fs::write(dir.path().join("a/b/two"), [0u8; 5]).unwrap();
        std::os::unix::fs::symlink("b/two", dir.path().join("a/link")).unwrap();
        std::os::unix::fs::symlink("/", dir.path().join("a/root")).unwrap();

        let files = regular_files(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
        // Symlink sizes are the link text length, never the target's size.
        assert_eq!(dir_size(dir.path()), 10 + 5 + "b/two".len() as u64 + 1);
    }

    #[test]
    fn finds_executables_in_path() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("tool");
        fs::write(&exe, "#!/bin/sh\n").unwrap();
        assert_eq!(which_in("tool", Some(dir.path().as_os_str())), None);
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(which_in("tool", Some(dir.path().as_os_str())), Some(exe));
    }
}
