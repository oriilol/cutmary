#!/usr/bin/env bash
set -euo pipefail

# Downloads the latest stable macOS CapCut DMG directly from ByteDance's official CDN
# (queried via Homebrew's cask metadata), extracts CapCut.app with 7zz, and restores
# relative QML symlinks that 7zz skips for safety.

OUT_DIR="${1:-./work}"
DMG_PATH="$OUT_DIR/CapCut.dmg"
APP_DIR="$OUT_DIR/extracted"
SEVENZZ="${SEVENZZ:-$(command -v 7zz || command -v 7z || true)}"

mkdir -p "$OUT_DIR"

if [[ -z "$SEVENZZ" ]]; then
    echo "error: 7zz (7-Zip) is required to extract HFS+/APFS DMG archives." >&2
    exit 1
fi

echo "==> Resolving latest macOS CapCut DMG URL..."
DMG_URL="$(curl -fsSL https://formulae.brew.sh/api/cask/capcut.json | python3 -c 'import json,sys; print(json.load(sys.stdin)["url"])')"
VERSION="$(curl -fsSL https://formulae.brew.sh/api/cask/capcut.json | python3 -c 'import json,sys; print(json.load(sys.stdin)["version"])')"
echo "    Version: $VERSION"
echo "    URL:     $DMG_URL"

if [[ ! -f "$DMG_PATH" ]]; then
    echo "==> Downloading CapCut $VERSION DMG with macOS User-Agent..."
    curl -fL --retry 3 \
        -A "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15" \
        -o "$DMG_PATH" "$DMG_URL"
fi

echo "==> Extracting CapCut.app..."
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR"
"$SEVENZZ" x -y -o"$APP_DIR" "$DMG_PATH" >/dev/null || true

# Locate CapCut.app inside extracted tree
FOUND_APP="$(find "$APP_DIR" -maxdepth 3 -name 'CapCut.app' -type d | head -n 1)"
if [[ -z "$FOUND_APP" ]]; then
    echo "error: CapCut.app not found after DMG extraction" >&2
    exit 1
fi
if [[ "$FOUND_APP" != "$APP_DIR/CapCut.app" ]]; then
    mv "$FOUND_APP" "$APP_DIR/CapCut.app"
fi

echo "==> Restoring relative symlinks skipped by 7zz..."
python3 - "$SEVENZZ" "$DMG_PATH" "$APP_DIR/CapCut.app" <<'PY'
import os, subprocess, sys

sevenzz, dmg, app_root = sys.argv[1], sys.argv[2], os.path.abspath(sys.argv[3])
out = subprocess.check_output([sevenzz, "l", "-slt", dmg], text=True, errors="ignore")

path = link = None
restored = 0
for line in out.splitlines() + [""]:
    if line.startswith("Path = "):
        path = line[7:]
    elif line.startswith("Symbolic Link = "):
        link = line[16:]
    elif line == "":
        if path and link and "CapCut.app/" in path:
            rel = path.split("CapCut.app/", 1)[1]
            dst = os.path.join(app_root, rel)
            resolved = os.path.normpath(os.path.join(os.path.dirname(dst), link))
            if resolved.startswith(app_root + os.sep) and not os.path.islink(dst):
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                if os.path.exists(dst):
                    os.unlink(dst)
                os.symlink(link, dst)
                restored += 1
        path = link = None

print(f"    Restored {restored} internal symlinks.")
PY

echo "==> Done: $APP_DIR/CapCut.app"
