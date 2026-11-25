#!/usr/bin/env bash
set -euo pipefail

# Builds Cutmary-x86_64.AppImage from launcher/.
#
# GTK 4 and libadwaita come from the host (gtk4 >= 4.12, libadwaita >= 1.5),
# which is present on every current desktop and avoids the theme/GL breakage of
# bundled GTK stacks. Optional extras are bundled next to the launcher, where it
# looks for them first:
#   SEVENZZ_BIN=/path/to/7zz      7-Zip for DMG extraction (default: 7zz in $PATH)
#   ROSEMARY_BIN=/path/to/rosemary  a patched Rosemary runtime (default: not bundled)
#   APPIMAGETOOL=/path/to/appimagetool  (default: $PATH, else downloaded)
#
# Usage: packaging/appimage/build-appimage.sh [out_dir]

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="$(realpath -m "${1:-$REPO_ROOT/dist}")"
BUILD_DIR="$REPO_ROOT/packaging/appimage/build"
APPDIR="$BUILD_DIR/Cutmary.AppDir"
ID="org.cutmary.Cutmary"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO_ROOT/launcher/Cargo.toml" | head -n1)"

echo "==> Building cutmary $VERSION (release)..."
cargo build --release --locked --manifest-path "$REPO_ROOT/launcher/Cargo.toml"

echo "==> Assembling AppDir..."
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/metainfo"
install -m755 "$REPO_ROOT/launcher/target/release/cutmary" "$APPDIR/usr/bin/cutmary"
install -m644 "$REPO_ROOT/packaging/$ID.desktop" "$APPDIR/usr/share/applications/$ID.desktop"
install -m644 "$REPO_ROOT/packaging/$ID.metainfo.xml" "$APPDIR/usr/share/metainfo/$ID.appdata.xml"
for size in 16 24 32 48 64 128 256 512; do
    install -Dm644 "$REPO_ROOT/launcher/assets/icons/hicolor/${size}x${size}/apps/$ID.png" \
        "$APPDIR/usr/share/icons/hicolor/${size}x${size}/apps/$ID.png"
done
ln -sf "usr/share/applications/$ID.desktop" "$APPDIR/$ID.desktop"
cp "$REPO_ROOT/launcher/assets/icons/hicolor/256x256/apps/$ID.png" "$APPDIR/$ID.png"
ln -sf "$ID.png" "$APPDIR/.DirIcon"

SEVENZZ_BIN="${SEVENZZ_BIN:-$(command -v 7zz || true)}"
if [[ -n "$SEVENZZ_BIN" ]]; then
    echo "    bundling 7zz from $SEVENZZ_BIN"
    install -m755 "$SEVENZZ_BIN" "$APPDIR/usr/bin/7zz"
else
    echo "    note: 7zz not found; the AppImage will use the host's 7-Zip"
fi
if [[ -n "${ROSEMARY_BIN:-}" ]]; then
    echo "    bundling rosemary from $ROSEMARY_BIN"
    install -m755 "$ROSEMARY_BIN" "$APPDIR/usr/bin/rosemary"
fi

cat > "$APPDIR/AppRun" <<'APPRUN'
#!/usr/bin/env bash
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/cutmary" "$@"
APPRUN
chmod +x "$APPDIR/AppRun"

APPIMAGETOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
if [[ -z "$APPIMAGETOOL" ]]; then
    APPIMAGETOOL="$BUILD_DIR/appimagetool-x86_64.AppImage"
    if [[ ! -x "$APPIMAGETOOL" ]]; then
        echo "==> Downloading appimagetool..."
        curl -fL --retry 3 -o "$APPIMAGETOOL" \
            https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
        chmod +x "$APPIMAGETOOL"
    fi
fi

mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/Cutmary-$VERSION-x86_64.AppImage"
echo "==> Packing $OUT..."
# Extract-and-run so the build also works without FUSE (containers, CI).
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --no-appstream "$APPDIR" "$OUT"
echo "==> AppImage ready: $OUT"
