#!/usr/bin/env bash
set -euo pipefail

# Clones upstream Rosemary at the tested revision, applies the CapCut compatibility
# patch series in patches/, and compiles the release binary with polite CPU settings.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${1:-$REPO_ROOT/work}"
ROSEMARY_DIR="$WORK_DIR/rosemary-src"
ROSEMARY_REV="411f2844cbd44754f727c2ebd22d7a168a506097"
JOBS="${JOBS:-6}"

mkdir -p "$WORK_DIR"

if [[ ! -d "$ROSEMARY_DIR/.git" ]]; then
    echo "==> Cloning microlift/rosemary..."
    git clone https://github.com/microlift/rosemary.git "$ROSEMARY_DIR"
fi

echo "==> Checking out base revision $ROSEMARY_REV..."
git -C "$ROSEMARY_DIR" fetch --quiet origin || true
git -C "$ROSEMARY_DIR" checkout --force "$ROSEMARY_REV"
git -C "$ROSEMARY_DIR" clean -fd

echo "==> Applying CapCut compatibility patch series..."
for patch in "$REPO_ROOT"/patches/*.patch; do
    echo "    Applying $(basename "$patch")"
    git -C "$ROSEMARY_DIR" apply "$patch"
done

echo "==> Building rosemary (nice -n 10, -j $JOBS)..."
(cd "$ROSEMARY_DIR" && nice -n 10 cargo build -j "$JOBS" --release --bin rosemary)

echo "==> Built: $ROSEMARY_DIR/target/release/rosemary"
