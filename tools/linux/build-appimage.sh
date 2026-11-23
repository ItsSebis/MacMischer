#!/usr/bin/env bash
# Build dist/MacMischer-<ver>-linux-x86_64.AppImage from target/release/macmischer.
# Needs `appimagetool` on PATH (or APPIMAGETOOL set). Run from the repo root after
# `cargo build --release -p macmischer`. VER defaults to the workspace version.
set -euo pipefail
cd "$(dirname "$0")/../.."
VER="${VER:-$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')}"
TOOL="${APPIMAGETOOL:-appimagetool}"
APPDIR="$(mktemp -d)/MacMischer.AppDir"
mkdir -p "$APPDIR/usr/bin" dist
install -m755 target/release/macmischer "$APPDIR/usr/bin/macmischer"
ln -s usr/bin/macmischer "$APPDIR/AppRun"
cp assets/macmischer.desktop "$APPDIR/macmischer.desktop"
cp assets/icon/icon-256.png "$APPDIR/macmischer.png"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" "$APPDIR" "dist/MacMischer-$VER-linux-x86_64.AppImage"
