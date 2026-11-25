<p align="center"><img src="assets/cutmary-logo.png" alt="Cutmary logo" width="160"></p>

# Cutmary — Running macOS CapCut Natively on Linux (`x86_64`) via Rosemary

[![Platform](https://img.shields.io/badge/platform-Linux%20x86__64-blue.svg)](#requirements)
[![Runtime](https://img.shields.io/badge/runtime-Rosemary%20(Darwin%20Compat)-purple.svg)](https://github.com/microlift/rosemary)
[![Target](https://img.shields.io/badge/target-CapCut%209.8.0%20macOS%20DMG-brightgreen.svg)](#how-it-works)
[![Status](https://img.shields.io/badge/status-Active%20WIP%20%E2%80%94%20GUI%20Boot%20Stage-orange.svg)](#current-status)
[![License](https://img.shields.io/badge/license-MIT-lightgrey.svg)](LICENSE)

**Cutmary** is a clean, reproducible patch series and packaging toolkit to run the official **macOS `x86_64` build of CapCut Desktop** directly on Linux (developed & tested on **CachyOS / Arch Linux**) using [**Rosemary**](https://github.com/microlift/rosemary) — **no Wine, no Proton, no macOS VM, and no CPU emulation overhead**.

Because CapCut's macOS `.dmg` ships as a **Universal Mach-O binary (`x86_64` + `arm64`)**, Rosemary executes the `x86_64` slice natively on Linux `x86_64` while translating Darwin syscalls, Mach IPC, Objective-C, Foundation, CoreFoundation, Security, and AppKit/Qt/CEF calls directly onto Linux primitives (SDL3, Wayland/XWayland, OpenSSL/rustls, etc.).

> **Legal Notice:** This repository contains **only** open-source Rosemary compatibility patches, build/extraction scripts, and a Mach-O `x86_64` thinner. **No proprietary CapCut binaries, assets, or DMG files are included or redistributed here.** The scripts download the official macOS DMG directly from ByteDance's CDN onto your local machine.

---

## Why macOS CapCut via Rosemary?

- **Windows builds on Wine/Proton** suffer from anti-cheat/DRM hooks, DirectComposition/D3D11/WebView2 breakage, and frequent silent crashes on Linux.
- **macOS CapCut (`CapCut_9_8_0_4799_nosandbox.dmg`)** ships with a clean Qt 6 (`libqcocoa.dylib`) + Chromium Embedded Framework (CEF) + Cronet + FFmpeg stack as a native `x86_64` Mach-O universal binary.
- **Universal Binary Thinning (`scripts/thin-x86_64.py`)** strips the unused `arm64` slice from all **236 universal Mach-O binaries** inside `CapCut.app`, shrinking the bundle from **2,295.9 MiB -> 1,173.1 MiB** (**saving ~1.12 GB**).

---

## Current Status

We have taken CapCut 9.8.0.4799 from failing immediately on the very first Mach-O symbol all the way through **90+ live threads** into the **Qt 6 / AppKit main GUI event loop** (`NSApplication -run` / `QApplication::exec()`):

| Milestone | Status | Notes |
| --- | :---: | --- |
| Official macOS DMG fetch & extraction (`7zz` + 34 QML symlinks) | **Done** | Automated in `scripts/fetch-capcut-dmg.sh` |
| `x86_64`-only Mach-O universal binary thinning (-1.12 GB) | **Done** | Automated in `scripts/thin-x86_64.py` |
| `libcccreator.dylib` & `libVECreator.dylib` bootstrap | **Done** | GCD, `OSAtomic*`, `NSString`/`NSData`/`NSBundle`, Mach ports, `kevent64` |
| Cronet HTTPS/TLS certificate chain verification (`libsscronet.dylib`) | **Done** | `SecPolicyCreateSSL`, 48-byte `CSSM_TP_APPLE_EVIDENCE_INFO`, DNS fallback |
| `libvideoeditor.dylib` Mach-O `LC_CODE_SIGNATURE` self-verification | **Done** | Real `SecCodeCopySigningInformation` parsing `identifier`, `teamid`, `flags` |
| Chromium Embedded Framework (CEF) + GPU Helper process spawn | **Done** | Pipe-backed `notify_register_file_descriptor`, `__sandbox_ms`, `_kernelrpc_*_trap` |
| `Parfait.framework` crash handler initialization | **Done** | XNU default entry for `task_get_exception_ports` |
| `bytenn` AI engine, `mobilecv2`, `MMKV` & `EffectSDK` init | **Done** | All 6 MMKV stores, VECodec callback, and `EffectABConfig` loaded cleanly |
| Qt 6 platform plugin (`libqcocoa.dylib`, `libqmacstyle.dylib`) & main event loop | **Done** | Fixed upstream Rosemary `NSRunLoop` per-thread isolation bug |
| Main window rendering at fractional Wayland scale (1.25x), no titlebar band | **Done** | Verified live on KDE Plasma Wayland + RTX 5060 Ti |
| Photo import -> media panel -> timeline (thumbnails) -> effects panel | **Done** | Verified live; see `HANDOFF.md` |
| Player playback, second import, video (`.mp4`) import, performance | **In Progress** | See open items in `HANDOFF.md` |
| Portable AppImage / CachyOS binary build | **Scripted** | `scripts/package-appimage.sh` ready once window stability is verified |

---

## Architecture & Patch Series

All Rosemary modifications live as four clean, auditable patches in [`patches/`](patches/) pinned to upstream Rosemary commit `411f2844cbd44754f727c2ebd22d7a168a506097`:

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

## Quick Start

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

# 5. Launch CapCut under patched Rosemary
./work/rosemary-src/target/release/rosemary ./work/extracted/CapCut.app

# 6. Package as a portable AppDir / AppImage (thinned to x86_64)
./scripts/package-appimage.sh ./work/rosemary-src/target/release/rosemary ./work/extracted/CapCut.app
```

---

## Credits

- [**Rosemary (`microlift/rosemary`)**](https://github.com/microlift/rosemary) — the incredible Rust macOS-on-Linux compatibility layer that makes running native `x86_64` Darwin apps on Linux possible.
- **CapCut** is a trademark and proprietary product of ByteDance Ltd. All rights belong to their respective owners.
