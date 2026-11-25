<p align="center"><img src="assets/cutmary-logo.png" alt="Cutmary logo" width="160"></p>

# Cutmary

Cutmary is a Linux launcher for the macOS version of CapCut. It runs the app’s x86_64 code directly on Linux x86_64 through a patched Rosemary runtime, without Wine or a virtual machine. The GTK4 / Libadwaita launcher installs the official DMG locally, manages the runtime and project settings, and shows session logs.

## Get the launcher

The planned distribution channels are AppImage and Flatpak bundles on GitHub Releases, and `cutmary-git` on the Arch User Repository. As of 25 November 2026, the public releases list is empty and the AUR package is pending publication. The packaging recipes are already in the repository. [GitHub Releases](https://github.com/oriilol/cutmary/releases) · [AppImage build script](packaging/appimage/build-appimage.sh) · [Flatpak manifest](packaging/flatpak/org.cutmary.Cutmary.yml) · [Arch PKGBUILD](packaging/aur/PKGBUILD)

To use the launcher today, build it from source with Rust, GTK 4.12+, Libadwaita 1.5+ and GLib development tools. Install 7-Zip and a Vulkan driver for your GPU. The build script below prepares the patched runtime in a dedicated work directory.

```sh
git clone https://github.com/oriilol/cutmary.git
cd cutmary
./scripts/build-rosemary.sh
cargo run --release --locked --manifest-path launcher/Cargo.toml
```

1. In **Settings → Runtime (Rosemary)**, select the custom runner and choose `work/rosemary-src/target/release/rosemary` from the checkout.
2. From **Home**, choose **Install CapCut** to download the official DMG, or **Use local .dmg…** to select your own. The version tested below is 9.8.0.4799. The online installer follows the current download.
3. Choose **Launch**. The launcher removes stale empty project locks before startup. **Show Logs** opens the current session log. **Repair Drafts** in Settings can restore a damaged draft from its backup.

The Flatpak recipe uses Rosemary on the host. The AppImage builder can include a patched runtime through `ROSEMARY_BIN`. The Arch recipe packages the launcher. [Launcher settings, paths and build instructions](launcher/README.md)

## Current status

CapCut 9.8.0.4799 was tested on CachyOS x86_64 with KDE Plasma Wayland and an NVIDIA GeForce RTX 5060 Ti.

| Workflow | Result on this system |
| --- | --- |
| Start CapCut and open an existing project | Home and editor render at 1.25× scale. Maximizing the editor fills the window. |
| Import photos | Several JPGs imported consecutively. Four clips showed thumbnails on the timeline. The Effects panel opened and the Export button became active. Exporting a file still needs a live test. |
| Open the context menu | Position and transparency verified. Opening took about 150–190 ms in the recorded run, with intermittent opacity on the OpenGL path. |
| Play the timeline | A live Wayland test on the RTX 5060 Ti reproduced a blank preview and playback stalls in an existing project. |
| Import MP4 video | Nine CoreVideo APIs are implemented and tested. The full import and playback workflow still needs verification. |

The handoff records the test conditions, measurements and outstanding work. [Verification notes](HANDOFF.md#resultados-cachyos)

## Architecture & Patch Series

All Rosemary modifications live as five clean, auditable patches in [`patches/`](patches/) pinned to upstream Rosemary commit `411f2844cbd44754f727c2ebd22d7a168a506097`:

```
patches/
├── 01-darwin-gcd-mach-net-sandbox.patch   # GCD barriers/queues/sources, OSAtomic, kevent64, Mach traps/exception ports, DNS/notifyd, __sandbox_ms
├── 02-cf-foundation-iokit-coretext.patch  # Per-thread NSRunLoop fix, NSString/NSData/NSBundle/NSTimeZone, CFNotificationCenter, DiskArbitration, CoreText, IOKit
├── 03-security-tls-codesign.patch         # Real LC_CODE_SIGNATURE CodeDirectory parser (SecCodeCopySigningInformation), SSL SecTrust + 48-byte CSSM evidence
├── 04-appkit-coregraphics-globals.patch   # NSColor/NSWorkspace/CoreGraphics/ImageIO/LaunchServices/Carbon HotKeys, 445 framework globals, fractional Wayland scale, titlebar accessories, QuickLook
└── 05-media-pipeline-metal-corevideo-runtime.patch  # CVMetal/CVOpenGL texture caches, Accelerate (vImage/vDSP), AVFoundation/AudioToolbox, Metal reflection forms, MSL implicit argument locations, C++ unwinding for dlopen'ed images
```

### Technical Highlights

1. **Per-thread `NSRunLoop` Isolation (`crates/foundation/src/timer.rs`)**
   Upstream Rosemary mapped both `+currentRunLoop` and `+mainRunLoop` to a single main-thread `CFRunLoop`. Under CEF + Qt 6, Chromium worker threads calling `[[NSRunLoop currentRunLoop] runMode:beforeDate:]` pumped Qt's main-thread `QCocoaEventDispatcher` timers concurrently with the main thread, causing cross-thread `QObject` races. Each thread now owns an isolated `NSRunLoop` backed by `corefoundation::runloop::current()`.
2. **Real Mach-O `LC_CODE_SIGNATURE` Inspection (`crates/security/src/codesign.rs`)**
   `libvideoeditor.dylib` validates the host executable's Apple code signature (`kSecCodeInfoIdentifier`, `kSecCodeInfoTeamIdentifier`, `kSecCodeInfoFlags`) via `SecCodeCopySigningInformation` before initializing its internal Lyra RPC services. Instead of bypassing the check, Rosemary now parses the binary's actual `CS_CodeDirectory` blob directly from `LC_CODE_SIGNATURE`.
3. **Exact 48-Byte `CSSM_TP_APPLE_EVIDENCE_INFO` & `notify_register_file_descriptor` Pipe Lifetime**
   Fixed Cronet TLS verification (`SecTrustGetResult` struct layout matching macOS LP64) and Chromium's `FileDescriptorWatcherPosix` crash by keeping the write end of `notifyd` pipes open in a global table until `notify_cancel`.
4. **XNU-Accurate `task_get_exception_ports` Default Entry**
   ByteDance's `Parfait.framework` indexes `ports[count - 1]` unconditionally after `KERN_SUCCESS`. Matching XNU's behavior (returning 1 entry with `MACH_PORT_NULL` when no custom handler is registered) prevents an unsigned underflow (`0xFFFFFFFF`) `SIGBUS`.

---

## Requirements

- **OS:** Linux `x86_64` (tested on CachyOS / Arch Linux with KDE Plasma Wayland + XWayland)
- **Toolchain:** Rust (`cargo` / `rustc`), `git`, `python3`, `curl`, `7zz` (7-Zip)
- **System Libraries:** `sdl3`, `libcurl`, `zlib`, `fuse3` (optional: `appimagetool` for `.AppImage` output)

On Arch Linux / CachyOS:
```bash
sudo pacman -S --needed git rust curl 7zip python sdl3 zlib fuse3
```

---

## Manual runtime workflow

```bash
# 1. Clone this repository
git clone https://github.com/oriilol/cutmary.git
cd cutmary

# 2. Clone upstream Rosemary, apply our patch series, and build (CPU-friendly: nice -n 10, -j 6)
./scripts/build-rosemary.sh

# 3. Download the latest macOS CapCut DMG directly from ByteDance's CDN & restore QML symlinks
./scripts/fetch-capcut-dmg.sh

# 4. (Optional) Strip arm64 slices to save ~1.12 GB
python3 ./scripts/thin-x86_64.py ./work/extracted/CapCut.app

# 5. With CapCut closed, remove stale empty locks before a manual launch
drafts="$HOME/.local/share/rosemary/prefixes/default/root/Users/$USER/Movies/CapCut/User Data/Projects/com.lveditor.draft"
if [ -d "$drafts" ]; then
    find "$drafts" -mindepth 2 -maxdepth 2 -type f -name '*.locked' -size 0c -delete
fi
./work/rosemary-src/target/release/rosemary run --new-instance ./work/extracted/CapCut.app

# 6. Package as a portable AppDir / AppImage (thinned to x86_64)
./scripts/package-appimage.sh ./work/rosemary-src/target/release/rosemary ./work/extracted/CapCut.app
```

---

## Credits

- [**Rosemary (`microlift/rosemary`)**](https://github.com/microlift/rosemary) — the incredible Rust macOS-on-Linux compatibility layer that makes running native `x86_64` Darwin apps on Linux possible.
- **CapCut** is a trademark and proprietary product of ByteDance Ltd. All rights belong to their respective owners.
