#!/usr/bin/env bash
set -euo pipefail

# Packages a thinned x86_64 CapCut.app + patched Rosemary binary into a portable
# Linux AppDir (and builds an AppImage if appimagetool is available).

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROSEMARY_BIN="${1:?Usage: $0 <path/to/rosemary> <path/to/CapCut.app> [out_dir]}"
CAPCUT_APP="${2:?Usage: $0 <path/to/rosemary> <path/to/CapCut.app> [out_dir]}"
OUT_DIR="${3:-$REPO_ROOT/dist}"
APPDIR="$OUT_DIR/CapCut.AppDir"

echo "==> Creating AppDir at $APPDIR..."
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/capcut"

cp -a "$ROSEMARY_BIN" "$APPDIR/usr/bin/rosemary"
chmod +x "$APPDIR/usr/bin/rosemary"

echo "==> Copying CapCut.app and thinning to x86_64 only..."
cp -a "$CAPCUT_APP" "$APPDIR/usr/share/capcut/CapCut.app"
python3 "$REPO_ROOT/scripts/thin-x86_64.py" "$APPDIR/usr/share/capcut/CapCut.app"

cat > "$APPDIR/AppRun" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/rosemary" "$HERE/usr/share/capcut/CapCut.app" "$@"
EOF
chmod +x "$APPDIR/AppRun"

cat > "$APPDIR/capcut-rosemary.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=CapCut (Rosemary)
Comment=macOS CapCut running natively on x86_64 Linux via Rosemary
Exec=AppRun
Icon=capcut-rosemary
Categories=AudioVideo;Video;AudioVideoEditing;
Terminal=false
EOF

# Extract PNG icon from AppIcon.icns if available (fallback to empty placeholder)
python3 - "$APPDIR/usr/share/capcut/CapCut.app/Contents/Resources" "$APPDIR/capcut-rosemary.png" <<'PY'
import glob, os, struct, sys

res_dir, out_png = sys.argv[1], sys.argv[2]
icns_files = glob.glob(os.path.join(res_dir, "*.icns"))
png_best = b""
for path in icns_files:
    data = open(path, "rb").read()
    pos = 8
    while pos + 8 <= len(data):
        tag = data[pos:pos+4]
        length = struct.unpack(">I", data[pos+4:pos+8])[0]
        if length < 8:
            break
        payload = data[pos+8:pos+length]
        if payload.startswith(b"\x89PNG\r\n\x1a\n") and len(payload) > len(png_best):
            png_best = payload
        pos += length
if png_best:
    with open(out_png, "wb") as f:
        f.write(png_best)
    print(f"    Extracted PNG icon ({len(png_best)} bytes)")
PY

if command -v appimagetool >/dev/null 2>&1; then
    echo "==> Building AppImage with appimagetool..."
    ARCH=x86_64 appimagetool "$APPDIR" "$OUT_DIR/CapCut-x86_64.AppImage"
    echo "==> AppImage ready: $OUT_DIR/CapCut-x86_64.AppImage"
else
    echo "==> AppDir ready: $APPDIR"
    echo "    Launch directly with: $APPDIR/AppRun"
    echo "    (Install appimagetool to pack into a single .AppImage file)"
fi
