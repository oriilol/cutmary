//! Runtime dependency preflight: host libraries that Rosemary's ImageIO/GPU
//! paths load at run time. Missing ones are reported, never fatal.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::util;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Distro {
    Arch,
    Debian,
    Fedora,
    Other,
}

impl Distro {
    /// From `/etc/os-release` (`ID` and `ID_LIKE`).
    pub fn from_os_release(text: &str) -> Self {
        let ids: Vec<String> = text
            .lines()
            .filter_map(|l| l.strip_prefix("ID=").or_else(|| l.strip_prefix("ID_LIKE=")))
            .flat_map(|v| v.trim_matches('"').split_whitespace().map(str::to_ascii_lowercase).collect::<Vec<_>>())
            .collect();
        let has = |id: &str| ids.iter().any(|i| i == id);
        if has("arch") || has("cachyos") || has("manjaro") || has("endeavouros") {
            Self::Arch
        } else if has("debian") || has("ubuntu") {
            Self::Debian
        } else if has("fedora") || has("rhel") {
            Self::Fedora
        } else {
            Self::Other
        }
    }

    fn install_prefix(self) -> Option<&'static str> {
        match self {
            Self::Arch => Some("sudo pacman -S --needed"),
            Self::Debian => Some("sudo apt install"),
            Self::Fedora => Some("sudo dnf install"),
            Self::Other => None,
        }
    }
}

/// What a check looks for.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// Any of these sonames.
    Library(&'static [&'static str]),
    /// A Vulkan ICD manifest (the GPU driver).
    VulkanIcd,
}

#[derive(Clone, Copy, Debug)]
struct Requirement {
    name: &'static str,
    purpose: &'static str,
    target: Target,
    /// Package names for Arch, Debian/Ubuntu, Fedora.
    packages: [&'static str; 3],
}

const REQUIREMENTS: &[Requirement] = &[
    Requirement {
        name: "libjpeg-turbo",
        purpose: "JPEG import and thumbnails (libturbojpeg.so.0)",
        target: Target::Library(&["libturbojpeg.so.0"]),
        packages: ["libjpeg-turbo", "libturbojpeg0", "turbojpeg"],
    },
    Requirement {
        name: "libwebp",
        purpose: "WebP images and stickers (libwebpdecoder.so.3 or libwebp.so.7)",
        target: Target::Library(&["libwebpdecoder.so.3", "libwebp.so.7"]),
        packages: ["libwebp", "libwebp7", "libwebp"],
    },
    Requirement {
        name: "SDL3",
        purpose: "Windows, input and audio (libSDL3.so.0)",
        target: Target::Library(&["libSDL3.so.0"]),
        packages: ["sdl3", "libsdl3-0", "SDL3"],
    },
    Requirement {
        name: "Vulkan loader",
        purpose: "GPU rendering through Metal-on-Vulkan (libvulkan.so.1)",
        target: Target::Library(&["libvulkan.so.1"]),
        packages: ["vulkan-icd-loader", "libvulkan1", "vulkan-loader"],
    },
    Requirement {
        name: "Vulkan driver",
        purpose: "A Vulkan ICD for your GPU (nvidia-utils, vulkan-radeon or vulkan-intel)",
        target: Target::VulkanIcd,
        packages: ["", "mesa-vulkan-drivers", "mesa-vulkan-drivers"],
    },
];

pub const ICD_DIRS: &[&str] = &["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d", "/usr/local/share/vulkan/icd.d"];

/// Read-only access to the host that runs Rosemary.
pub trait Probe {
    /// `ldconfig -p` output, if available.
    fn ldconfig(&self) -> Option<String>;
    /// File names in `dir` (empty if missing).
    fn list_dir(&self, dir: &str) -> Vec<String>;
    fn os_release(&self) -> Option<String>;
}

/// Probes this machine, or the host through `flatpak-spawn` inside a Flatpak
/// (Rosemary runs on the host, so its libraries are what matter).
pub struct HostProbe {
    pub in_flatpak: bool,
}

impl HostProbe {
    fn run(&self, args: &[&str]) -> Option<String> {
        let mut cmd = if self.in_flatpak {
            let mut c = Command::new("flatpak-spawn");
            c.arg("--host").args(args);
            c
        } else {
            let mut c = Command::new(args[0]);
            c.args(&args[1..]);
            c
        };
        let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl Probe for HostProbe {
    fn ldconfig(&self) -> Option<String> {
        // Debian keeps ldconfig in /sbin, outside a user's PATH.
        self.run(&["sh", "-c", "ldconfig -p 2>/dev/null || /sbin/ldconfig -p 2>/dev/null || /usr/sbin/ldconfig -p"])
            .filter(|s| !s.trim().is_empty())
    }

    fn list_dir(&self, dir: &str) -> Vec<String> {
        if self.in_flatpak {
            return self.run(&["ls", "-1", dir]).map(|s| s.lines().map(str::to_owned).collect()).unwrap_or_default();
        }
        std::fs::read_dir(dir)
            .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    }

    fn os_release(&self) -> Option<String> {
        let path = if self.in_flatpak { "/run/host/os-release" } else { "/etc/os-release" };
        std::fs::read_to_string(path).ok().or_else(|| std::fs::read_to_string("/usr/lib/os-release").ok())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckResult {
    pub name: &'static str,
    pub purpose: &'static str,
    /// Where it was found, or `None` when missing.
    pub found: Option<String>,
    package: &'static str,
}

impl CheckResult {
    pub fn ok(&self) -> bool {
        self.found.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<CheckResult>,
    pub distro: Distro,
}

impl Report {
    pub fn missing(&self) -> Vec<&CheckResult> {
        self.checks.iter().filter(|c| !c.ok()).collect()
    }

    pub fn all_ok(&self) -> bool {
        self.checks.iter().all(CheckResult::ok)
    }

    /// e.g. `sudo pacman -S --needed libjpeg-turbo libwebp sdl3`, when the
    /// missing pieces map to packages on this distro.
    pub fn install_command(&self) -> Option<String> {
        let prefix = self.distro.install_prefix()?;
        let packages: Vec<&str> = self.missing().iter().map(|c| c.package).filter(|p| !p.is_empty()).collect();
        (!packages.is_empty()).then(|| format!("{prefix} {}", packages.join(" ")))
    }

    /// One line for the log.
    pub fn summary(&self) -> String {
        if self.all_ok() {
            return "all runtime libraries found".to_owned();
        }
        let names: Vec<&str> = self.missing().iter().map(|c| c.name).collect();
        let mut line = format!("missing runtime libraries: {}", names.join(", "));
        if let Some(cmd) = self.install_command() {
            line.push_str(&format!(" (install with: {cmd})"));
        }
        line
    }
}

fn find_library(probe: &dyn Probe, ldconfig: Option<&str>, sonames: &[&str]) -> Option<String> {
    sonames.iter().find_map(|soname| {
        ldconfig.and_then(|listing| util::find_in_ldconfig(listing, soname)).or_else(|| {
            util::LIBRARY_DIRS
                .iter()
                .find(|dir| probe.list_dir(dir).iter().any(|f| f == soname))
                .map(|dir| Path::new(dir).join(soname).display().to_string())
        })
    })
}

pub fn run(probe: &dyn Probe) -> Report {
    let ldconfig = probe.ldconfig();
    let distro = probe.os_release().map(|t| Distro::from_os_release(&t)).unwrap_or(Distro::Other);
    let package_index = match distro {
        Distro::Debian => 1,
        Distro::Fedora => 2,
        Distro::Arch | Distro::Other => 0,
    };
    let checks = REQUIREMENTS
        .iter()
        .map(|req| {
            let found = match req.target {
                Target::Library(sonames) => find_library(probe, ldconfig.as_deref(), sonames),
                Target::VulkanIcd => {
                    let icds: Vec<String> =
                        ICD_DIRS.iter().flat_map(|dir| probe.list_dir(dir)).filter(|f| f.ends_with(".json")).collect();
                    (!icds.is_empty()).then(|| icds.join(", "))
                }
            };
            CheckResult { name: req.name, purpose: req.purpose, found, package: req.packages[package_index] }
        })
        .collect();
    Report { checks, distro }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeHost {
        ldconfig: Option<String>,
        dirs: HashMap<&'static str, Vec<&'static str>>,
        os_release: Option<&'static str>,
    }

    impl Probe for FakeHost {
        fn ldconfig(&self) -> Option<String> {
            self.ldconfig.clone()
        }
        fn list_dir(&self, dir: &str) -> Vec<String> {
            self.dirs.get(dir).map(|v| v.iter().map(|s| s.to_string()).collect()).unwrap_or_default()
        }
        fn os_release(&self) -> Option<String> {
            self.os_release.map(str::to_owned)
        }
    }

    const CACHYOS: &str = "NAME=\"CachyOS Linux\"\nID=cachyos\nID_LIKE=arch\n";

    #[test]
    fn everything_present_on_a_complete_host() {
        let host = FakeHost {
            ldconfig: Some(
                "\tlibturbojpeg.so.0 (libc6,x86-64) => /usr/lib/libturbojpeg.so.0\n\
                 \tlibwebp.so.7 (libc6,x86-64) => /usr/lib/libwebp.so.7\n\
                 \tlibSDL3.so.0 (libc6,x86-64) => /usr/lib/libSDL3.so.0\n\
                 \tlibvulkan.so.1 (libc6,x86-64) => /usr/lib/libvulkan.so.1\n"
                    .into(),
            ),
            dirs: HashMap::from([("/usr/share/vulkan/icd.d", vec!["nvidia_icd.json"])]),
            os_release: Some(CACHYOS),
        };
        let report = run(&host);
        assert!(report.all_ok(), "{report:?}");
        assert_eq!(report.distro, Distro::Arch);
        assert_eq!(report.install_command(), None);
        assert_eq!(report.checks[1].found.as_deref(), Some("/usr/lib/libwebp.so.7"));
        assert_eq!(report.checks[4].found.as_deref(), Some("nvidia_icd.json"));
        assert_eq!(report.summary(), "all runtime libraries found");
    }

    #[test]
    fn missing_libraries_produce_a_pacman_command() {
        let host = FakeHost {
            ldconfig: Some("\tlibvulkan.so.1 (libc6,x86-64) => /usr/lib/libvulkan.so.1\n".into()),
            dirs: HashMap::from([("/etc/vulkan/icd.d", vec!["radeon_icd.x86_64.json"])]),
            os_release: Some(CACHYOS),
        };
        let report = run(&host);
        let missing: Vec<_> = report.missing().iter().map(|c| c.name).collect();
        assert_eq!(missing, ["libjpeg-turbo", "libwebp", "SDL3"]);
        assert_eq!(report.install_command().as_deref(), Some("sudo pacman -S --needed libjpeg-turbo libwebp sdl3"));
        assert!(report.summary().contains("install with: sudo pacman"));
    }

    #[test]
    fn falls_back_to_library_dirs_and_reports_missing_driver() {
        let host = FakeHost {
            ldconfig: None,
            dirs: HashMap::from([
                ("/usr/lib/x86_64-linux-gnu", vec!["libturbojpeg.so.0", "libwebpdecoder.so.3", "libSDL3.so.0"]),
                ("/usr/lib/x86_64-linux-gnu/", vec![]),
            ]),
            os_release: Some("ID=ubuntu\nID_LIKE=debian\n"),
        };
        let report = run(&host);
        assert_eq!(report.distro, Distro::Debian);
        assert_eq!(report.checks[1].found.as_deref(), Some("/usr/lib/x86_64-linux-gnu/libwebpdecoder.so.3"));
        let missing: Vec<_> = report.missing().iter().map(|c| c.name).collect();
        assert_eq!(missing, ["Vulkan loader", "Vulkan driver"]);
        assert_eq!(report.install_command().as_deref(), Some("sudo apt install libvulkan1 mesa-vulkan-drivers"));
    }

    #[test]
    fn unknown_distros_get_no_command() {
        let report = run(&FakeHost { os_release: Some("ID=gentoo\n"), ..FakeHost::default() });
        assert_eq!(report.distro, Distro::Other);
        assert_eq!(report.missing().len(), 5);
        assert_eq!(report.install_command(), None);
        assert_eq!(Distro::from_os_release("ID=\"fedora\"\n"), Distro::Fedora);
    }

    /// Prints the real host's report: `cargo test -- --ignored preflight_on_this_host --nocapture`
    #[test]
    #[ignore]
    fn preflight_on_this_host() {
        let report = run(&HostProbe { in_flatpak: std::path::Path::new("/.flatpak-info").exists() });
        for check in &report.checks {
            eprintln!("{:<14} {}", check.name, check.found.as_deref().unwrap_or("MISSING"));
        }
        eprintln!("{:?}: {}", report.distro, report.summary());
    }
}
