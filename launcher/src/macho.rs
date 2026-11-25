//! Native universal (FAT) Mach-O thinner: keeps only the `x86_64` slice.
//!
//! Mirrors `scripts/thin-x86_64.py`: symlinks, non-regular files, thin
//! binaries and FAT files without an `x86_64` slice are left untouched; file
//! permissions are preserved; the replacement is written next to the original
//! and renamed over it so a crash never leaves a half-written binary.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const FAT_MAGIC: u32 = 0xcafe_babe;
pub const FAT_MAGIC_64: u32 = 0xcafe_babf;
pub const CPU_TYPE_X86_64: u32 = 0x0100_0007;
/// `CPU_SUBTYPE_X86_64_H` (Haswell). Plain `x86_64` slices are preferred over it.
const CPU_SUBTYPE_X86_64_H: u32 = 8;
const CPU_SUBTYPE_MASK: u32 = 0xff00_0000;
/// Real universal binaries have a handful of slices; Java class files share
/// `0xcafebabe` but carry their version (>= 45) where `nfat_arch` would be.
pub const MAX_FAT_ARCHS: u32 = 16;
/// Smallest file worth inspecting (FAT header + one 20-byte entry).
const MIN_FAT_SIZE: u64 = 28;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FatArch {
    pub cputype: u32,
    pub cpusubtype: u32,
    pub offset: u64,
    pub size: u64,
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

fn be64(bytes: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(bytes[at..at + 8].try_into().expect("8 bytes"))
}

/// Parses the FAT header at the start of `file`. Returns `None` for anything
/// that is not a plausible universal binary.
pub fn read_fat_archs(file: &mut File) -> io::Result<Option<Vec<FatArch>>> {
    let mut header = [0u8; 8];
    if read_fully(file, &mut header)? < header.len() {
        return Ok(None);
    }
    let magic = be32(&header, 0);
    let nfat = be32(&header, 4);
    let entry_size = match magic {
        FAT_MAGIC => 20,
        FAT_MAGIC_64 => 32,
        _ => return Ok(None),
    };
    if nfat == 0 || nfat > MAX_FAT_ARCHS {
        return Ok(None);
    }
    let mut table = vec![0u8; nfat as usize * entry_size];
    if read_fully(file, &mut table)? < table.len() {
        return Ok(None);
    }
    let archs = table
        .chunks_exact(entry_size)
        .map(|e| {
            if magic == FAT_MAGIC {
                FatArch {
                    cputype: be32(e, 0),
                    cpusubtype: be32(e, 4),
                    offset: u64::from(be32(e, 8)),
                    size: u64::from(be32(e, 12)),
                }
            } else {
                FatArch { cputype: be32(e, 0), cpusubtype: be32(e, 4), offset: be64(e, 8), size: be64(e, 16) }
            }
        })
        .collect();
    Ok(Some(archs))
}

fn read_fully(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Picks the `x86_64` slice, preferring generic `x86_64` over `x86_64h`.
pub fn select_x86_64(archs: &[FatArch]) -> Option<FatArch> {
    let mut x86 = archs.iter().filter(|a| a.cputype == CPU_TYPE_X86_64);
    let first = x86.clone().next().copied();
    x86.find(|a| a.cpusubtype & !CPU_SUBTYPE_MASK != CPU_SUBTYPE_X86_64_H)
        .copied()
        .or(first)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThinOutcome {
    /// The file was (or, in a dry run, would be) replaced by its x86_64 slice.
    Thinned { before: u64, after: u64 },
    Skipped,
}

/// Replaces a universal Mach-O at `path` with its `x86_64` slice.
pub fn thin_file(path: &Path, dry_run: bool) -> io::Result<ThinOutcome> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(ThinOutcome::Skipped),
        Err(e) => return Err(e),
    };
    if !meta.file_type().is_file() || meta.len() < MIN_FAT_SIZE {
        return Ok(ThinOutcome::Skipped);
    }
    let mut file = File::open(path)?;
    let Some(archs) = read_fat_archs(&mut file)? else {
        return Ok(ThinOutcome::Skipped);
    };
    let Some(slice) = select_x86_64(&archs) else {
        return Ok(ThinOutcome::Skipped);
    };
    let in_bounds = slice.size > 0
        && slice.offset.checked_add(slice.size).is_some_and(|end| end <= meta.len());
    if !in_bounds {
        return Ok(ThinOutcome::Skipped);
    }
    let outcome = ThinOutcome::Thinned { before: meta.len(), after: slice.size };
    if dry_run {
        return Ok(outcome);
    }

    let tmp = tmp_path(path);
    let result = (|| {
        file.seek(SeekFrom::Start(slice.offset))?;
        let mut out = File::create(&tmp)?;
        let copied = io::copy(&mut (&mut file).take(slice.size), &mut out)?;
        if copied != slice.size {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "x86_64 slice truncated"));
        }
        out.sync_all()?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(meta.permissions().mode() & 0o7777))?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map(|()| outcome)
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".thin.tmp");
    path.with_file_name(name)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThinReport {
    pub files_scanned: usize,
    pub binaries_thinned: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

impl ThinReport {
    pub fn saved(&self) -> u64 {
        self.bytes_before.saturating_sub(self.bytes_after)
    }
}

/// Thins every universal binary under `root`. `progress(done, total, path)` is
/// called after each file; returns `ErrorKind::Interrupted` if `cancel` is set.
pub fn thin_bundle(
    root: &Path,
    dry_run: bool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize, &Path),
) -> io::Result<ThinReport> {
    let files = crate::util::regular_files(root)?;
    let mut report = ThinReport { files_scanned: files.len(), ..ThinReport::default() };
    for (i, path) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let outcome = thin_file(path, dry_run)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        if let ThinOutcome::Thinned { before, after } = outcome {
            report.binaries_thinned += 1;
            report.bytes_before += before;
            report.bytes_after += after;
        }
        progress(i + 1, files.len(), path);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MH_MAGIC_64: u32 = 0xfeed_facf;
    const CPU_TYPE_ARM64: u32 = 0x0100_000c;

    /// A fake thin Mach-O slice: magic, cputype, then a recognizable fill byte.
    fn slice(cputype: u32, fill: u8, len: usize) -> Vec<u8> {
        let mut s = Vec::with_capacity(len);
        s.extend(MH_MAGIC_64.to_le_bytes());
        s.extend(cputype.to_le_bytes());
        s.resize(len, fill);
        s
    }

    /// Builds a universal binary with `slices` placed at 4 KiB alignment.
    fn fat(magic: u32, slices: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
        let entry = if magic == FAT_MAGIC { 20 } else { 32 };
        let mut offset = 0x1000u64;
        let mut header = Vec::new();
        header.extend(magic.to_be_bytes());
        header.extend((slices.len() as u32).to_be_bytes());
        let mut layout = Vec::new();
        for (cputype, subtype, data) in slices {
            header.extend(cputype.to_be_bytes());
            header.extend(subtype.to_be_bytes());
            if magic == FAT_MAGIC {
                header.extend((offset as u32).to_be_bytes());
                header.extend((data.len() as u32).to_be_bytes());
                header.extend(12u32.to_be_bytes());
            } else {
                header.extend(offset.to_be_bytes());
                header.extend((data.len() as u64).to_be_bytes());
                header.extend(12u32.to_be_bytes());
                header.extend(0u32.to_be_bytes());
            }
            layout.push(offset);
            offset = (offset + data.len() as u64).div_ceil(0x1000) * 0x1000;
        }
        assert_eq!(header.len(), 8 + entry * slices.len());
        let mut out = header;
        for ((_, _, data), at) in slices.iter().zip(layout) {
            out.resize(at as usize, 0);
            out.extend(data);
        }
        out
    }

    fn write(path: &Path, bytes: &[u8], mode: u32) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn thins_arm64_plus_x86_64_to_x86_64() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CapCut");
        let arm = slice(CPU_TYPE_ARM64, 0xAA, 5000);
        let x86 = slice(CPU_TYPE_X86_64, 0x86, 3000);
        let universal = fat(FAT_MAGIC, &[(CPU_TYPE_ARM64, 0, arm), (CPU_TYPE_X86_64, 3, x86.clone())]);
        write(&path, &universal, 0o755);

        let outcome = thin_file(&path, false).unwrap();
        assert_eq!(outcome, ThinOutcome::Thinned { before: universal.len() as u64, after: 3000 });
        assert_eq!(fs::read(&path).unwrap(), x86);
        assert_eq!(mode(&path), 0o755, "permissions preserved");
        assert!(!dir.path().join("CapCut.thin.tmp").exists());

        // A second pass finds a thin binary and leaves it alone.
        assert_eq!(thin_file(&path, false).unwrap(), ThinOutcome::Skipped);
    }

    #[test]
    fn handles_fat_magic_64_and_unusual_modes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lib.dylib");
        let x86 = slice(CPU_TYPE_X86_64, 0x11, 1234);
        let universal = fat(
            FAT_MAGIC_64,
            &[(CPU_TYPE_X86_64, 3, x86.clone()), (CPU_TYPE_ARM64, 0, slice(CPU_TYPE_ARM64, 0x22, 999))],
        );
        write(&path, &universal, 0o640);
        assert!(matches!(thin_file(&path, false).unwrap(), ThinOutcome::Thinned { after: 1234, .. }));
        assert_eq!(fs::read(&path).unwrap(), x86);
        assert_eq!(mode(&path), 0o640);
    }

    #[test]
    fn prefers_generic_x86_64_over_haswell_slice() {
        let archs = [
            FatArch { cputype: CPU_TYPE_X86_64, cpusubtype: CPU_SUBTYPE_X86_64_H, offset: 0x1000, size: 1 },
            FatArch { cputype: CPU_TYPE_ARM64, cpusubtype: 0, offset: 0x2000, size: 1 },
            FatArch { cputype: CPU_TYPE_X86_64, cpusubtype: 3 | 0x8000_0000, offset: 0x3000, size: 1 },
        ];
        assert_eq!(select_x86_64(&archs).unwrap().offset, 0x3000);
        assert_eq!(select_x86_64(&archs[..2]).unwrap().offset, 0x1000);
        assert_eq!(select_x86_64(&archs[1..2]), None);
    }

    #[test]
    fn dry_run_reports_without_modifying() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin");
        let universal = fat(
            FAT_MAGIC,
            &[(CPU_TYPE_X86_64, 3, slice(CPU_TYPE_X86_64, 1, 100)), (CPU_TYPE_ARM64, 0, slice(CPU_TYPE_ARM64, 2, 100))],
        );
        write(&path, &universal, 0o755);
        assert!(matches!(thin_file(&path, true).unwrap(), ThinOutcome::Thinned { after: 100, .. }));
        assert_eq!(fs::read(&path).unwrap(), universal);
    }

    #[test]
    fn skips_files_that_must_not_be_touched() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();

        // arm64-only universal binary: no x86_64 slice to keep.
        let arm_only = fat(FAT_MAGIC, &[(CPU_TYPE_ARM64, 0, slice(CPU_TYPE_ARM64, 3, 64))]);
        write(&d.join("arm_only"), &arm_only, 0o755);
        // Java class file: same magic, but "nfat" is the class version (52).
        let mut class = vec![0xca, 0xfe, 0xba, 0xbe, 0x00, 0x00, 0x00, 0x34];
        class.resize(64, 0);
        write(&d.join("Main.class"), &class, 0o644);
        // Thin Mach-O, text file, tiny file.
        write(&d.join("thin"), &slice(CPU_TYPE_X86_64, 0, 64), 0o755);
        write(&d.join("readme.txt"), b"hello, this is definitely not a binary file", 0o644);
        write(&d.join("tiny"), &FAT_MAGIC.to_be_bytes(), 0o644);
        // Slice table pointing past the end of the file.
        let mut truncated = fat(FAT_MAGIC, &[(CPU_TYPE_X86_64, 3, slice(CPU_TYPE_X86_64, 4, 4096))]);
        truncated.truncate(truncated.len() - 100);
        write(&d.join("truncated"), &truncated, 0o755);
        // Header claims more slices than the file holds.
        let mut short_table = Vec::new();
        short_table.extend(FAT_MAGIC.to_be_bytes());
        short_table.extend(4u32.to_be_bytes());
        short_table.resize(30, 0);
        write(&d.join("short_table"), &short_table, 0o755);

        for name in ["arm_only", "Main.class", "thin", "readme.txt", "tiny", "truncated", "short_table", "missing"] {
            let path = d.join(name);
            let before = fs::read(&path).ok();
            assert_eq!(thin_file(&path, false).unwrap(), ThinOutcome::Skipped, "{name}");
            assert_eq!(fs::read(&path).ok(), before, "{name} must be unchanged");
        }
    }

    #[test]
    fn symlinks_are_skipped_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real");
        let universal = fat(
            FAT_MAGIC,
            &[(CPU_TYPE_ARM64, 0, slice(CPU_TYPE_ARM64, 9, 200)), (CPU_TYPE_X86_64, 3, slice(CPU_TYPE_X86_64, 8, 200))],
        );
        write(&target, &universal, 0o755);
        let link = dir.path().join("link");
        std::os::unix::fs::symlink("real", &link).unwrap();
        assert_eq!(thin_file(&link, false).unwrap(), ThinOutcome::Skipped);
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), universal);
    }

    #[test]
    fn thins_a_whole_bundle_and_reports_savings() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("CapCut.app");
        let macos = app.join("Contents/MacOS");
        let fw = app.join("Contents/Frameworks/Qt.framework/Versions/A");
        fs::create_dir_all(&macos).unwrap();
        fs::create_dir_all(&fw).unwrap();
        let make = |fill| {
            fat(
                FAT_MAGIC,
                &[(CPU_TYPE_ARM64, 0, slice(CPU_TYPE_ARM64, fill, 6000)), (CPU_TYPE_X86_64, 3, slice(CPU_TYPE_X86_64, fill, 4000))],
            )
        };
        write(&macos.join("CapCut"), &make(1), 0o755);
        write(&fw.join("Qt"), &make(2), 0o755);
        write(&app.join("Contents/Info.plist"), b"<plist/>", 0o644);
        std::os::unix::fs::symlink("A", app.join("Contents/Frameworks/Qt.framework/Versions/Current")).unwrap();
        std::os::unix::fs::symlink("Versions/Current/Qt", app.join("Contents/Frameworks/Qt.framework/Qt")).unwrap();
        let before = make(1).len() as u64 + make(2).len() as u64;

        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let report = thin_bundle(&app, false, &cancel, |done, total, _| {
            calls += 1;
            assert!(done <= total);
        })
        .unwrap();
        assert_eq!(report.files_scanned, 3);
        assert_eq!(calls, 3);
        assert_eq!(report.binaries_thinned, 2);
        assert_eq!(report.bytes_before, before);
        assert_eq!(report.bytes_after, 8000);
        assert_eq!(report.saved(), before - 8000);
        // Framework symlinks still resolve to the thinned binary.
        assert_eq!(fs::read(app.join("Contents/Frameworks/Qt.framework/Qt")).unwrap().len(), 4000);

        cancel.store(true, Ordering::Relaxed);
        let err = thin_bundle(&app, false, &cancel, |_, _, _| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
    }
}
