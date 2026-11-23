#!/usr/bin/env bash
# Build dist/MacMischer-<ver>-macos-arm64.dmg (MacMischer.app at its top level), ad hoc signed.
# Needs cargo-bundle (`cargo install cargo-bundle`) and an Apple Silicon Mac. Run from the repo root.
set -euo pipefail
cd "$(dirname "$0")/../.."
VER="${VER:-$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')}"
cargo bundle --release -p macmischer
APP=target/release/bundle/osx/MacMischer.app
codesign --force --deep -s - "$APP"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
mkdir -p dist
hdiutil create -volname MacMischer -srcfolder "$STAGE" -ov -format UDZO "dist/MacMischer-$VER-macos-arm64.dmg"
