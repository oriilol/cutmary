//! First-run setup: fetch the official macOS CapCut DMG and prepare `CapCut.app`.
//!
//! Matches `scripts/fetch-capcut-dmg.sh` + `scripts/thin-x86_64.py`:
//! 1. Resolve the latest DMG URL from Homebrew's `capcut` cask metadata.
//! 2. Download it with a macOS User-Agent (resumable, sha256-verified).
//! 3. Extract with 7-Zip (`7zz x`), then locate `CapCut.app`.
//! 4. Restore the internal relative symlinks (QML modules, framework
//!    `Versions/Current`) that 7-Zip skips, from `7zz l -slt`.
//! 5. Optionally thin universal Mach-O binaries to `x86_64` (~1.12 GB saved).
//! 6. Swap the result into `~/.local/share/cutmary/CapCut.app`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{Paths, VERSION};
use crate::logging::Log;
use crate::macho::{self, ThinReport};
use crate::util;

pub const BREW_CASK_URL: &str = "https://formulae.brew.sh/api/cask/capcut.json";
/// ByteDance's CDN serves the DMG to macOS browsers.
pub const MAC_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_7) AppleWebKit/605.1.15 \
     (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
const APP_NAME: &str = "CapCut.app";

// ---------------------------------------------------------------------------
// Errors, steps and progress events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("installation cancelled"),
            Self::Failed(msg) => f.write_str(msg),
        }
    }
}

impl From<io::Error> for InstallError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::Interrupted {
            Self::Cancelled
        } else {
            Self::Failed(e.to_string())
        }
    }
}

fn fail(msg: impl Into<String>) -> InstallError {
    InstallError::Failed(msg.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Resolve,
    Download,
    Verify,
    Extract,
    Symlinks,
    Thin,
    Finalize,
}

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Self::Resolve => "Finding the latest CapCut release…",
            Self::Download => "Downloading CapCut for macOS…",
            Self::Verify => "Verifying download…",
            Self::Extract => "Extracting CapCut.app…",
            Self::Symlinks => "Restoring QML and framework symlinks…",
            Self::Thin => "Thinning binaries to x86_64…",
            Self::Finalize => "Finishing up…",
        }
    }

    /// The slice of the overall progress bar this step fills.
    fn span(self) -> (f64, f64) {
        match self {
            Self::Resolve => (0.0, 0.02),
            Self::Download => (0.02, 0.55),
            Self::Verify => (0.55, 0.60),
            Self::Extract => (0.60, 0.80),
            Self::Symlinks => (0.80, 0.82),
            Self::Thin => (0.82, 0.98),
            Self::Finalize => (0.98, 1.0),
        }
    }

    pub fn overall(self, fraction: f64) -> f64 {
        let (start, end) = self.span();
        start + (end - start) * fraction.clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub step: Step,
    /// Overall pipeline progress in `0.0..=1.0`.
    pub fraction: f64,
    pub detail: String,
}

/// Throttles progress so the UI is updated at most ~12 times per second.
struct Reporter<'a> {
    emit: &'a (dyn Fn(Progress) + Sync),
    last: Option<(Step, Instant)>,
}

impl<'a> Reporter<'a> {
    fn new(emit: &'a (dyn Fn(Progress) + Sync)) -> Self {
        Self { emit, last: None }
    }

    fn step(&mut self, step: Step, detail: impl Into<String>) {
        self.force(step, 0.0, detail.into());
    }

    fn update(&mut self, step: Step, fraction: f64, detail: impl FnOnce() -> String) {
        let due = match self.last {
            Some((last_step, at)) => last_step != step || at.elapsed() >= Duration::from_millis(80),
            None => true,
        };
        if due || fraction >= 1.0 {
            self.force(step, fraction, detail());
        }
    }

    fn force(&mut self, step: Step, fraction: f64, detail: String) {
        self.last = Some((step, Instant::now()));
        (self.emit)(Progress { step, fraction: step.overall(fraction), detail });
    }
}

// ---------------------------------------------------------------------------
// Release metadata (Homebrew cask)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
    /// `None` when the cask says `no_check`.
    pub sha256: Option<String>,
}

pub fn parse_cask(json: &str) -> Result<Release, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("invalid cask JSON: {e}"))?;
    let field = |name: &str| value.get(name).and_then(|v| v.as_str()).map(str::to_owned);
    let url = field("url").ok_or("cask metadata has no `url`")?;
    if !url.starts_with("https://") {
        return Err(format!("refusing non-HTTPS download URL: {url}"));
    }
    let version = field("version").ok_or("cask metadata has no `version`")?;
    let sha256 = field("sha256")
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()));
    Ok(Release { version, url, sha256 })
}

pub fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(format!("Cutmary/{VERSION} (+https://github.com/oriilol/cutmary)"))
        .connect_timeout(Duration::from_secs(30))
        // Downloads take minutes; only the connection has a deadline.
        .timeout(None::<Duration>)
        .build()
        .map_err(|e| format!("could not create HTTP client: {e}"))
}

pub fn resolve_latest(client: &reqwest::blocking::Client) -> Result<Release, String> {
    let body = client
        .get(BREW_CASK_URL)
        .timeout(Duration::from_secs(30))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| format!("could not query {BREW_CASK_URL}: {e}"))?;
    parse_cask(&body)
}

// ---------------------------------------------------------------------------
// Download + verification
// ---------------------------------------------------------------------------

/// Parses the total from a `Content-Range: bytes 100-199/2000` header.
pub fn content_range_total(header: &str) -> Option<u64> {
    header.rsplit_once('/')?.1.trim().parse().ok()
}

fn download(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(), InstallError> {
    let part = dest.with_extension("dmg.part");
    let mut retried = false;
    loop {
        let existing = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let mut request = client.get(url).header(reqwest::header::USER_AGENT, MAC_USER_AGENT);
        if existing > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }
        let mut response = request.send().map_err(|e| fail(format!("download failed: {e}")))?;
        let status = response.status();
        let (mut written, total, append) = match status.as_u16() {
            206 => {
                let total = response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(content_range_total);
                (existing, total, true)
            }
            200 => (0, response.content_length(), false),
            416 if !retried => {
                // The partial file is stale or already complete; start over once.
                fs::remove_file(&part)?;
                retried = true;
                continue;
            }
            _ => return Err(fail(format!("download failed: HTTP {status} from {url}"))),
        };

        let file = OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(&part)?;
        let mut out = BufWriter::with_capacity(1 << 20, file);
        let mut buf = vec![0u8; 256 * 1024];
        on_progress(written, total);
        loop {
            if cancel.load(Ordering::Relaxed) {
                out.flush()?;
                return Err(InstallError::Cancelled);
            }
            let n = match response.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(fail(format!("download interrupted: {e} (it will resume on retry)"))),
            };
            out.write_all(&buf[..n])?;
            written += n as u64;
            on_progress(written, total);
        }
        out.flush()?;
        drop(out);
        if let Some(total) = total
            && written != total
        {
            return Err(fail(format!("download incomplete: got {written} of {total} bytes (it will resume on retry)")));
        }
        fs::rename(&part, dest)?;
        return Ok(());
    }
}

pub fn sha256_file(path: &Path, cancel: &AtomicBool, mut on_progress: impl FnMut(u64, u64)) -> io::Result<String> {
    let total = fs::metadata(path)?.len();
    let mut reader = BufReader::with_capacity(1 << 20, File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        on_progress(done, total);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

// ---------------------------------------------------------------------------
// 7-Zip extraction and symlink restoration
// ---------------------------------------------------------------------------

/// Finds 7-Zip: `$SEVENZZ`, a copy bundled next to the launcher, then `$PATH`.
/// `7zz` (7-Zip 21+) is preferred; legacy p7zip `7z` may lack APFS support.
pub fn find_sevenzip() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("SEVENZZ").map(PathBuf::from)
        && util::is_executable(&explicit)
    {
        return Some(explicit);
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let bundled = exe_dir
        .into_iter()
        .flat_map(|d| [d.join("7zz"), d.join("../lib/cutmary/7zz")])
        .find(|p| util::is_executable(p));
    bundled.or_else(|| util::which("7zz")).or_else(|| util::which("7z"))
}

/// Extracts the last `NN%` value from a chunk of `7zz -bsp1` progress output.
pub fn parse_percent(chunk: &str) -> Option<u8> {
    chunk
        .split(|c: char| c.is_whitespace() || c == '\u{8}')
        .filter_map(|token| token.strip_suffix('%'))
        .filter_map(|digits| digits.parse::<u8>().ok())
        .rfind(|p| *p <= 100)
}

fn extract(
    sevenzip: &Path,
    dmg: &Path,
    out_dir: &Path,
    cancel: &Arc<AtomicBool>,
    log: &Log,
    mut on_percent: impl FnMut(u8),
) -> Result<(), InstallError> {
    let mut child = Command::new(sevenzip)
        .arg("x")
        .arg("-y")
        .arg("-bsp1") // progress to stdout
        .arg("-bso0") // no per-file chatter
        .arg(format!("-o{}", out_dir.display()))
        .arg(dmg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| fail(format!("could not run {}: {e}", sevenzip.display())))?;

    // 7-Zip can sit silently on a large file; a watchdog kills it on cancel.
    let pid = child.id() as libc::pid_t;
    let finished = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let (cancel, finished) = (cancel.clone(), finished.clone());
        std::thread::spawn(move || {
            while !finished.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    // SAFETY: plain kill(2) on our own child, which has not been reaped yet.
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                    return;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        })
    };
    let stderr = child.stderr.take().map(|stderr| {
        let log = log.clone();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut text);
            // 7-Zip refuses relative symlinks that climb directories (the QML
            // plugin links); they are expected and restored in the next step.
            let mut skipped_links = 0;
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                if line.contains("Dangerous link path was ignored") {
                    skipped_links += 1;
                } else {
                    log.raw(&format!("7zz: {line}"));
                }
            }
            if skipped_links > 0 {
                log.info(format!("7-Zip skipped {skipped_links} relative symlinks (expected; restored next)"));
            }
        })
    });
    if let Some(mut stdout) = child.stdout.take() {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            if let Some(p) = parse_percent(&String::from_utf8_lossy(&buf[..n])) {
                on_percent(p);
            }
        }
    }
    let status = child.wait()?;
    finished.store(true, Ordering::Relaxed);
    let _ = watchdog.join();
    if let Some(handle) = stderr {
        let _ = handle.join();
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(InstallError::Cancelled);
    }
    if !status.success() {
        // Like the shell script, tolerate warnings (e.g. skipped symlinks) and
        // judge success by whether CapCut.app came out.
        log.info(format!("7-Zip exited with {status}; checking the extracted tree anyway"));
    }
    Ok(())
}

/// Finds `CapCut.app` up to three levels below `root` (breadth-first).
pub fn find_app(root: &Path) -> Option<PathBuf> {
    let mut level = vec![root.to_path_buf()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for dir in level {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                if entry.file_name() == APP_NAME {
                    return Some(entry.path());
                }
                next.push(entry.path());
            }
        }
        level = next;
    }
    None
}

/// Extracts `(path, link target)` pairs from `7zz l -slt` output.
pub fn parse_slt_symlinks(listing: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let (mut path, mut link) = (None, None);
    for line in listing.lines().chain(std::iter::once("")) {
        if let Some(p) = line.strip_prefix("Path = ") {
            path = Some(p.to_owned());
        } else if let Some(l) = line.strip_prefix("Symbolic Link = ") {
            link = Some(l.to_owned());
        } else if line.is_empty() {
            if let (Some(p), Some(l)) = (path.take(), link.take())
                && !l.is_empty()
            {
                out.push((p, l));
            }
            path = None;
            link = None;
        }
    }
    out
}

/// Decides where an archived symlink belongs inside `app_root`. Returns `None`
/// for links outside `CapCut.app`, or whose location or target would escape it
/// (absolute links like `Applications -> /Applications` are dropped).
pub fn plan_symlink(app_root: &Path, archive_path: &str, target: &str) -> Option<PathBuf> {
    let (_, rel) = archive_path.split_once("CapCut.app/")?;
    if rel.is_empty() {
        return None;
    }
    let dst = app_root.join(rel);
    if !util::is_strictly_inside(&dst, app_root) {
        return None;
    }
    let resolved = dst.parent()?.join(target);
    util::is_strictly_inside(&resolved, app_root).then(|| util::normalize_lexically(&dst))
}

/// Recreates internal relative symlinks; returns how many were created.
pub fn apply_symlinks(app_root: &Path, links: &[(String, String)]) -> io::Result<usize> {
    let mut restored = 0;
    for (archive_path, target) in links {
        let Some(dst) = plan_symlink(app_root, archive_path, target) else { continue };
        if fs::symlink_metadata(&dst).is_ok_and(|m| m.file_type().is_symlink()) {
            continue;
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        util::remove_any(&dst)?;
        std::os::unix::fs::symlink(target, &dst)?;
        restored += 1;
    }
    Ok(restored)
}

fn restore_symlinks(sevenzip: &Path, dmg: &Path, app_root: &Path) -> Result<usize, InstallError> {
    let output = Command::new(sevenzip)
        .args(["l", "-slt"])
        .arg(dmg)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| fail(format!("could not list the DMG: {e}")))?;
    let links = parse_slt_symlinks(&String::from_utf8_lossy(&output.stdout));
    Ok(apply_symlinks(app_root, &links)?)
}

// ---------------------------------------------------------------------------
// Installed app metadata
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstallRecord {
    pub version: Option<String>,
    pub source: String,
    pub thinned: bool,
    pub saved_bytes: u64,
    pub installed_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledApp {
    pub path: PathBuf,
    pub version: Option<String>,
}

/// Prefers the full build number when it extends the marketing version
/// (`9.8.0` + `9.8.0.4799` → `9.8.0.4799`).
pub fn display_version(short: Option<&str>, build: Option<&str>) -> Option<String> {
    match (short, build) {
        (Some(s), Some(b)) if b.starts_with(s) => Some(b.to_owned()),
        (Some(s), _) => Some(s.to_owned()),
        (None, Some(b)) => Some(b.to_owned()),
        (None, None) => None,
    }
}

pub fn read_bundle_version(app: &Path) -> Option<String> {
    let plist = plist::Value::from_file(app.join("Contents/Info.plist")).ok()?;
    let dict = plist.as_dictionary()?;
    let get = |key: &str| dict.get(key).and_then(|v| v.as_string()).map(str::trim).filter(|s| !s.is_empty());
    display_version(get("CFBundleShortVersionString"), get("CFBundleVersion"))
}

/// CapCut's Info.plist says `9.9.0` while the release is `9.9.0.4841`:
/// prefer the release version when it extends the bundle's.
pub fn best_version(bundle: Option<String>, release: Option<String>) -> Option<String> {
    match (bundle, release) {
        (Some(b), Some(r)) if r.starts_with(&b) => Some(r),
        (b, r) => b.or(r),
    }
}

pub fn installed(paths: &Paths) -> Option<InstalledApp> {
    let path = paths.app_bundle();
    if !path.join("Contents/Info.plist").is_file() {
        return None;
    }
    let recorded = fs::read_to_string(paths.install_record())
        .ok()
        .and_then(|t| toml::from_str::<InstallRecord>(&t).ok())
        .and_then(|r| r.version);
    let version = best_version(read_bundle_version(&path), recorded);
    Some(InstalledApp { path, version })
}

/// Atomically replaces `target` with `staged` (keeping the old copy until the
/// new one is in place).
fn swap_into_place(staged: &Path, target: &Path) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let old = target.with_extension("app.old");
    util::remove_any(&old)?;
    let had_old = fs::symlink_metadata(target).is_ok();
    if had_old {
        fs::rename(target, &old)?;
    }
    if let Err(e) = fs::rename(staged, target) {
        if had_old {
            let _ = fs::rename(&old, target);
        }
        return Err(e);
    }
    util::remove_any(&old)
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Latest official DMG from ByteDance's CDN.
    Download,
    /// A DMG the user already has.
    LocalDmg(PathBuf),
}

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub source: Source,
    pub thin: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallSummary {
    pub version: Option<String>,
    pub symlinks_restored: usize,
    pub thin: Option<ThinReport>,
}

fn safe_file_component(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect()
}

/// Runs the whole pipeline on the calling (worker) thread.
pub fn run_install(
    paths: &Paths,
    options: &InstallOptions,
    cancel: &Arc<AtomicBool>,
    log: &Log,
    emit: &(dyn Fn(Progress) + Sync),
) -> Result<InstallSummary, InstallError> {
    let staging = paths.staging_dir();
    let result = install_inner(paths, options, cancel, log, emit, &staging);
    let _ = util::remove_any(&staging);
    result
}

fn install_inner(
    paths: &Paths,
    options: &InstallOptions,
    cancel: &Arc<AtomicBool>,
    log: &Log,
    emit: &(dyn Fn(Progress) + Sync),
    staging: &Path,
) -> Result<InstallSummary, InstallError> {
    let mut report = Reporter::new(emit);
    let check_cancel = || if cancel.load(Ordering::Relaxed) { Err(InstallError::Cancelled) } else { Ok(()) };
    fs::create_dir_all(&paths.data_dir)?;
    fs::create_dir_all(&paths.cache_dir)?;

    let sevenzip = find_sevenzip().ok_or_else(|| {
        fail("7-Zip is required to extract the DMG. Install it (e.g. `sudo pacman -S 7zip`) and try again.")
    })?;
    log.info(format!("using 7-Zip at {}", sevenzip.display()));

    // 1-2. Obtain the DMG.
    let (dmg, version, downloaded) = match &options.source {
        Source::LocalDmg(path) => {
            if !path.is_file() {
                return Err(fail(format!("{} does not exist", path.display())));
            }
            log.info(format!("installing from local DMG {}", path.display()));
            (path.clone(), None, false)
        }
        Source::Download => {
            report.step(Step::Resolve, "Querying Homebrew cask metadata");
            let client = http_client().map_err(fail)?;
            let release = resolve_latest(&client).map_err(fail)?;
            log.info(format!("latest CapCut {} at {}", release.version, release.url));
            let dmg = paths.cache_dir.join(format!("CapCut-{}.dmg", safe_file_component(&release.version)));
            if dmg.is_file() {
                log.info(format!("reusing cached {}", dmg.display()));
            } else {
                report.step(Step::Download, "Connecting…");
                let started = Instant::now();
                download(&client, &release.url, &dmg, cancel, |done, total| {
                    let fraction = total.map(|t| done as f64 / t.max(1) as f64).unwrap_or(0.0);
                    report.update(Step::Download, fraction, || {
                        let rate = done as f64 / started.elapsed().as_secs_f64().max(0.001);
                        match total {
                            Some(t) => format!(
                                "{} of {} ({}/s)",
                                util::format_bytes(done),
                                util::format_bytes(t),
                                util::format_bytes(rate as u64)
                            ),
                            None => format!("{} downloaded", util::format_bytes(done)),
                        }
                    });
                })?;
                log.info(format!("downloaded {}", util::format_bytes(fs::metadata(&dmg)?.len())));
            }
            if let Some(expected) = &release.sha256 {
                report.step(Step::Verify, "Checking SHA-256");
                let actual = sha256_file(&dmg, cancel, |done, total| {
                    report.update(Step::Verify, done as f64 / total.max(1) as f64, || {
                        format!("{} of {}", util::format_bytes(done), util::format_bytes(total))
                    });
                })?;
                if &actual != expected {
                    let _ = fs::remove_file(&dmg);
                    return Err(fail(format!("checksum mismatch: expected {expected}, got {actual}")));
                }
                log.info("SHA-256 verified");
            } else {
                log.info("cask has no checksum (no_check); skipping verification");
            }
            (dmg, Some(release.version), true)
        }
    };
    check_cancel()?;

    // 3. Extract.
    report.step(Step::Extract, "Running 7-Zip");
    util::remove_any(staging)?;
    let extracted = staging.join("extracted");
    fs::create_dir_all(&extracted)?;
    extract(&sevenzip, &dmg, &extracted, cancel, log, |p| {
        report.update(Step::Extract, f64::from(p) / 100.0, || format!("{p}%"));
    })?;
    let found = find_app(&extracted)
        .ok_or_else(|| fail("CapCut.app was not found in the DMG (is this the macOS CapCut installer?)"))?;
    let app = staging.join(APP_NAME);
    fs::rename(&found, &app)?;
    check_cancel()?;

    // 4. Symlinks.
    report.step(Step::Symlinks, "Reading the DMG catalog");
    let restored = restore_symlinks(&sevenzip, &dmg, &app)?;
    log.info(format!("restored {restored} internal symlinks"));
    check_cancel()?;

    // 5. Thin.
    let thin = if options.thin {
        report.step(Step::Thin, "Scanning for universal binaries");
        let r = macho::thin_bundle(&app, false, cancel, |done, total, path| {
            report.update(Step::Thin, done as f64 / total.max(1) as f64, || {
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            });
        })?;
        log.info(format!(
            "thinned {} universal Mach-O binaries: {} -> {} (saved {})",
            r.binaries_thinned,
            util::format_bytes(r.bytes_before),
            util::format_bytes(r.bytes_after),
            util::format_bytes(r.saved())
        ));
        Some(r)
    } else {
        log.info("keeping universal binaries (thinning disabled)");
        None
    };
    check_cancel()?;

    // 6. Finalize.
    report.step(Step::Finalize, "Moving CapCut.app into place");
    swap_into_place(&app, &paths.app_bundle())?;
    let version = best_version(read_bundle_version(&paths.app_bundle()), version);
    let record = InstallRecord {
        version: version.clone(),
        source: match &options.source {
            Source::Download => "download".into(),
            Source::LocalDmg(p) => p.display().to_string(),
        },
        thinned: thin.is_some(),
        saved_bytes: thin.map(|t| t.saved()).unwrap_or(0),
        installed_at: util::unix_now(),
    };
    let text = toml::to_string_pretty(&record).map_err(|e| fail(e.to_string()))?;
    fs::write(paths.install_record(), text)?;
    if downloaded {
        let _ = fs::remove_file(&dmg);
    }
    report.force(Step::Finalize, 1.0, "Done".into());
    log.info(format!("CapCut {} installed at {}", version.as_deref().unwrap_or("(unknown version)"), paths.app_bundle().display()));
    Ok(InstallSummary { version, symlinks_restored: restored, thin })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_homebrew_cask_metadata() {
        let json = r#"{"token":"capcut","version":"9.8.0.4799",
            "url":"https://lf16-capcut.faceulv.com/obj/capcutpc-packages-us/packages/CapCut_9_8_0_4799_nosandbox.dmg",
            "sha256":"ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789"}"#;
        let r = parse_cask(json).unwrap();
        assert_eq!(r.version, "9.8.0.4799");
        assert!(r.url.ends_with("CapCut_9_8_0_4799_nosandbox.dmg"));
        assert_eq!(r.sha256.as_deref(), Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"));

        let no_check = r#"{"version":"1","url":"https://x/y.dmg","sha256":"no_check"}"#;
        assert_eq!(parse_cask(no_check).unwrap().sha256, None);
        assert!(parse_cask(r#"{"version":"1","url":"http://x/y.dmg"}"#).is_err());
        assert!(parse_cask(r#"{"version":"1"}"#).is_err());
        assert!(parse_cask("not json").is_err());
    }

    #[test]
    fn parses_content_range_and_7zip_progress() {
        assert_eq!(content_range_total("bytes 100-199/2000"), Some(2000));
        assert_eq!(content_range_total("bytes */*"), None);
        assert_eq!(parse_percent("  0%\u{8}\u{8}\u{8}\u{8}  7% 12 - CapCut.app/Contents"), Some(7));
        assert_eq!(parse_percent(" 42%\r 43%"), Some(43));
        assert_eq!(parse_percent("Everything is Ok"), None);
        assert_eq!(parse_percent("250%"), None);
    }

    #[test]
    fn step_spans_cover_the_bar_monotonically() {
        let steps = [Step::Resolve, Step::Download, Step::Verify, Step::Extract, Step::Symlinks, Step::Thin, Step::Finalize];
        let mut last = 0.0;
        for s in steps {
            assert!(s.overall(0.0) >= last);
            assert!(s.overall(1.0) > s.overall(0.0));
            last = s.overall(1.0);
        }
        assert_eq!(Step::Finalize.overall(1.0), 1.0);
        assert_eq!(Step::Download.overall(2.0), Step::Download.overall(1.0));
    }

    const SLT: &str = "\
Listing archive: CapCut.dmg

--
Path = CapCut.dmg
Type = Dmg

----------
Path = CapCut/Applications
Symbolic Link = /Applications

Path = CapCut/CapCut.app/Contents/Resources/qml/QtQuick/Controls/libqtquickcontrols2plugin.dylib
Symbolic Link = ../../../../Frameworks/libqtquickcontrols2plugin.dylib

Path = CapCut/CapCut.app/Contents/Frameworks/Qt.framework/Versions/Current
Symbolic Link = A

Path = CapCut/CapCut.app/Contents/Frameworks/Qt.framework/Qt
Symbolic Link = Versions/Current/Qt

Path = CapCut/CapCut.app/Contents/Frameworks/Evil.framework/Escape
Symbolic Link = ../../../../../../etc/passwd

Path = CapCut/CapCut.app/../outside
Symbolic Link = CapCut.app/Contents

Path = CapCut/CapCut.app/Contents/Info.plist
Size = 1234
";

    #[test]
    fn parses_7zip_symlink_listing() {
        let links = parse_slt_symlinks(SLT);
        assert_eq!(links.len(), 6);
        assert_eq!(links[0], ("CapCut/Applications".into(), "/Applications".into()));
        assert_eq!(links[2].1, "A");
    }

    #[test]
    fn plans_only_internal_symlinks() {
        let root = Path::new("/data/staging/CapCut.app");
        let plan = |p: &str, l: &str| plan_symlink(root, p, l);
        assert_eq!(
            plan("CapCut/CapCut.app/Contents/Frameworks/Qt.framework/Versions/Current", "A"),
            Some(root.join("Contents/Frameworks/Qt.framework/Versions/Current"))
        );
        assert!(plan("CapCut/CapCut.app/Contents/Resources/qml/QtQuick/Controls/lib.dylib", "../../../../Frameworks/lib.dylib").is_some());
        assert_eq!(plan("CapCut/Applications", "/Applications"), None, "outside the app");
        assert_eq!(plan("CapCut/CapCut.app/Contents/x", "/usr/lib/libc.dylib"), None, "absolute target");
        assert_eq!(plan("CapCut/CapCut.app/Contents/Frameworks/E.framework/Escape", "../../../../../etc"), None);
        assert_eq!(plan("CapCut/CapCut.app/../outside", "CapCut.app/Contents"), None, "location escapes");
        assert_eq!(plan("CapCut/CapCut.app/", "x"), None);
    }

    #[test]
    fn applies_symlinks_replacing_materialized_copies() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("CapCut.app");
        let versions = app.join("Contents/Frameworks/Qt.framework/Versions");
        fs::create_dir_all(versions.join("A")).unwrap();
        fs::write(versions.join("A/Qt"), b"qt").unwrap();
        // 7-Zip sometimes materializes a link as a directory or a small file.
        fs::create_dir_all(versions.join("Current")).unwrap();
        fs::write(versions.join("Current/Qt"), b"copy").unwrap();
        fs::write(app.join("Contents/Frameworks/Qt.framework/Qt"), b"Versions/Current/Qt").unwrap();

        let links = parse_slt_symlinks(SLT);
        let restored = apply_symlinks(&app, &links).unwrap();
        assert_eq!(restored, 3);
        assert_eq!(fs::read_link(versions.join("Current")).unwrap(), PathBuf::from("A"));
        assert_eq!(fs::read(app.join("Contents/Frameworks/Qt.framework/Qt")).unwrap(), b"qt");
        assert!(fs::symlink_metadata(app.join("Contents/Resources/qml/QtQuick/Controls/libqtquickcontrols2plugin.dylib")).is_ok());
        assert!(!dir.path().join("outside").exists());
        assert!(fs::symlink_metadata(app.join("Contents/Frameworks/Evil.framework/Escape")).is_err());
        // Idempotent.
        assert_eq!(apply_symlinks(&app, &links).unwrap(), 0);
    }

    #[test]
    fn finds_app_bundle_in_extracted_tree() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_app(dir.path()), None);
        fs::create_dir_all(dir.path().join("CapCut/CapCut.app/Contents")).unwrap();
        assert_eq!(find_app(dir.path()), Some(dir.path().join("CapCut/CapCut.app")));
    }

    #[test]
    fn reads_versions_from_info_plist() {
        assert_eq!(display_version(Some("9.8.0"), Some("9.8.0.4799")).as_deref(), Some("9.8.0.4799"));
        assert_eq!(display_version(Some("9.8.0"), Some("4799")).as_deref(), Some("9.8.0"));
        assert_eq!(display_version(None, Some("4799")).as_deref(), Some("4799"));
        assert_eq!(display_version(None, None), None);
        let s = |v: &str| Some(v.to_owned());
        assert_eq!(best_version(s("9.9.0"), s("9.9.0.4841")), s("9.9.0.4841"));
        assert_eq!(best_version(s("9.9.0"), s("10.0.0")), s("9.9.0"));
        assert_eq!(best_version(None, s("9.9.0.4841")), s("9.9.0.4841"));

        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(
            |k| (k == "HOME").then(|| dir.path().as_os_str().to_owned()),
            false,
        );
        assert_eq!(installed(&paths), None);
        let contents = paths.app_bundle().join("Contents");
        fs::create_dir_all(&contents).unwrap();
        fs::write(
            contents.join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleShortVersionString</key><string>9.8.0</string>
<key>CFBundleVersion</key><string>9.8.0.4799</string>
</dict></plist>"#,
        )
        .unwrap();
        let app = installed(&paths).unwrap();
        assert_eq!(app.version.as_deref(), Some("9.8.0.4799"));
    }

    #[test]
    fn swaps_bundles_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("CapCut.app");
        let staged = dir.path().join("staging/CapCut.app");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("old"), b"").unwrap();
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("new"), b"").unwrap();
        swap_into_place(&staged, &target).unwrap();
        assert!(target.join("new").exists());
        assert!(!target.join("old").exists());
        assert!(!dir.path().join("CapCut.app.old").exists());
        assert!(!staged.exists());
    }

    #[test]
    fn sanitizes_cache_names() {
        assert_eq!(safe_file_component("9.8.0.4799"), "9.8.0.4799");
        assert_eq!(safe_file_component("9.8,abc/../x"), "9.8_abc_.._x");
    }

    /// Full pipeline against the real CDN. Opt-in, downloads ~1 GB:
    /// `CUTMARY_E2E_DIR=/tmp/x cargo test --release -- --ignored e2e --nocapture`
    /// (optionally with `CUTMARY_E2E_DMG=/path/CapCut.dmg` and `SEVENZZ=...`).
    #[test]
    #[ignore]
    fn e2e_install_real_capcut() {
        let root = PathBuf::from(std::env::var("CUTMARY_E2E_DIR").expect("set CUTMARY_E2E_DIR"));
        let paths = Paths::resolve(
            |k| match k {
                "HOME" => Some(root.clone().into_os_string()),
                _ => None,
            },
            false,
        );
        let log = Log::open(paths.latest_log());
        let source = match std::env::var_os("CUTMARY_E2E_DMG") {
            Some(dmg) => Source::LocalDmg(PathBuf::from(dmg)),
            None => Source::Download,
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let last = std::sync::Mutex::new(String::new());
        let emit = |p: Progress| {
            let line = format!("{:?}", p.step);
            let mut last = last.lock().unwrap();
            if *last != line {
                eprintln!("[{:>5.1}%] {} {}", p.fraction * 100.0, p.step.label(), p.detail);
                *last = line;
            }
        };
        let summary = run_install(&paths, &InstallOptions { source, thin: true }, &cancel, &log, &emit).unwrap();
        eprintln!("{summary:#?}");
        let app = installed(&paths).expect("installed");
        assert!(app.path.join("Contents/MacOS").is_dir());
        assert!(summary.symlinks_restored > 0, "QML/framework symlinks restored");
        assert!(summary.thin.unwrap().binaries_thinned > 0);
        assert!(!paths.staging_dir().exists());
    }
}
