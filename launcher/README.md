# Cutmary Launcher

A native **GTK4 + Libadwaita** launcher (Rust) that installs the official macOS
CapCut and runs it on Linux through [Rosemary](https://github.com/microlift/rosemary).
It never bundles CapCut: on first launch it downloads the official DMG from
ByteDance's CDN and prepares it locally.

## What it does

**Home**
- **Install CapCut** runs the setup pipeline with live progress:
  resolve the latest DMG (Homebrew cask metadata) → resumable download →
  SHA-256 check → `7zz` extraction → restore the internal QML/framework
  symlinks 7-Zip refuses → thin universal Mach-O binaries to `x86_64`
  (≈1.1 GB saved) → atomic swap into place.
- **Use local .dmg…** installs from a DMG you already have.
- **Launch** runs `rosemary run --new-instance CapCut.app`. While CapCut is
  running the button shows **Running…** next to **Stop**, which sends SIGTERM
  to the process group and SIGKILL after 5 s.
- **Show Logs** opens a live viewer for `latest.log`, with Copy and Open Logs Folder buttons.
- Before every launch, while no CapCut instance is running, the 0-byte
  `*.locked` markers that a crashed CapCut leaves in
  `com.lveditor.draft/<draft>/` are removed and logged. Without this, those
  projects refuse to open.
- A banner appears if host libraries Rosemary needs are missing. It never
  blocks a launch.

**Settings**
- *Cutmary*: version + GitHub Releases update check, theme, advanced mode.
- *CapCut Installation*: status and size, Reinstall / Repair, arm64 thinning,
  projects folder, and **Repair Drafts**. Repair Drafts cleans stale locks,
  restores a missing or empty `draft_info.json` from CapCut's
  `draft_info.json.bak`, and offers to archive drafts that can't be restored
  into `Projects/Cutmary Archived Drafts` (it never deletes them).
- *Runtime (Rosemary)*: runner (auto / bundled / custom), prefix directory,
  prefix status with `rosemary prefix create|repair`.
- *System Requirements*: `libturbojpeg.so.0`, `libwebpdecoder.so.3`/`libwebp.so.7`,
  `libSDL3.so.0`, `libvulkan.so.1` and a Vulkan ICD. Inside the Flatpak these
  are checked on the host. When something is missing, a copyable install
  command is shown, e.g. `sudo pacman -S --needed libjpeg-turbo libwebp sdl3`.
- *Graphics & Window*: decorations, application menu, Vulkan present mode, fault handler,
  Ctrl editing shortcuts, custom environment variables (advanced).

| Setting | Rosemary variable |
| --- | --- |
| Window Decorations: Native / Client | `ROSEMARY_DECORATIONS=server` / `client` |
| Application Menu: Auto (default) / Hidden / Strip / Titlebar | `ROSEMARY_MENU=none` on Wayland (unset elsewhere) / `none` / `strip` / `titlebar` |
| Vulkan Presentation Mode | `ROSEMARY_METAL_PRESENTATION_MODE=fifo\|fifo_relaxed\|mailbox\|immediate` |
| Keep Fault Handler (default on) | `ROSEMARY_KEEP_FAULT_HANDLER=1` |
| Ctrl for Editing Shortcuts (default on) | `ROSEMARY_CONTROL_EDIT_KEYS=1` |
| Prefix Directory | `ROSEMARY_PREFIX=<dir>` |
| Log All Unbound Imports (advanced) | `ROSEMARY_UNBOUND_DETAIL=all` |
| Custom Environment Variables (advanced) | applied last, override the above |

All of these variables exist in the Rosemary revision pinned by `scripts/build-rosemary.sh`.

## Files

| Path | Contents |
| --- | --- |
| `~/.config/cutmary/config.toml` | settings |
| `~/.local/share/cutmary/CapCut.app` | prepared CapCut bundle |
| `~/.local/share/cutmary/install.toml` | installed version, thinning savings |
| `~/.local/share/cutmary/logs/latest.log` | launcher + Rosemary output (`previous.log` = last session) |
| `~/.cache/cutmary/` | in-progress DMG download (resumable, deleted after install) |
| `~/.local/share/rosemary/prefixes/default` | Rosemary prefix (CapCut projects live under `root/Users/<you>/Movies/CapCut`) |

Rosemary's output goes straight into the log file rather than through a pipe,
so CapCut keeps running after the launcher closes. The next launcher start
re-attaches to it through `session.pid`.

## Build

Needs Rust 1.85+, GTK ≥ 4.12, libadwaita ≥ 1.5, and `glib-compile-resources`. At runtime you also need `7zz` (Arch: `7zip`) and `rosemary`.

```bash
cd launcher
cargo build --release
./target/release/cutmary
```

## Tests

```bash
cargo test            # unit tests: config, env/command builder, Mach-O thinner, symlinks, runner, logs
cargo clippy --all-targets
```

The end-to-end install test is opt-in. It downloads about 1.3 GB:

```bash
CUTMARY_E2E_DIR=/tmp/cutmary-e2e cargo test --release -- --ignored e2e --nocapture
# optional: CUTMARY_E2E_DMG=/path/CapCut.dmg  SEVENZZ=/path/7zz
```

## Layout

```
src/main.rs         application setup, resources, CSS, accelerators
src/config.rs       XDG paths + TOML config
src/installer.rs    download, verify, 7-Zip extraction, symlink restoration, pipeline
src/macho.rs        native FAT Mach-O → x86_64 thinner
src/runner.rs       runner discovery, command/env builder, process sessions, prefix status
src/drafts.rs       stale draft lock cleanup, damaged-draft restore/archive, running-CapCut check
src/preflight.rs    host library / Vulkan ICD checks with distro install hints
src/update.rs       GitHub Releases check
src/logging.rs      shared log file
src/ui/             controller (state + background work), window, home, settings, logs
assets/             app icons, symbolic icons, stylesheet (compiled into a GResource)
```

Packaging lives in `../packaging/`: the desktop entry, AppStream metainfo, a Flatpak manifest, the AUR `PKGBUILD`, and the AppImage script.
