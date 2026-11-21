#!/usr/bin/env bash
# Build MacMischer.app, install it into /Applications and remove the build bundles
# (so Spotlight/Launchpad show only the installed copy).
#   tools/install-macos.sh            # keep the incremental build cache (fast next time)
#   tools/install-macos.sh --clean    # also delete the whole target/ directory
set -euo pipefail

cd "$(dirname "$0")/.."
APP_NAME="MacMischer"
DEST="/Applications/$APP_NAME.app"
BUNDLE_DIR="target/release/bundle"
APP="$BUNDLE_DIR/osx/$APP_NAME.app"

if ! cargo bundle --version >/dev/null 2>&1; then
  echo "cargo-bundle missing — installing it first"
  cargo install cargo-bundle
fi

echo "==> Building"
cargo bundle --release -p macmischer

echo "==> Signing (ad hoc)"
codesign --force --deep -s - "$APP"

if pgrep -x macmischer >/dev/null; then
  echo "==> Quitting running $APP_NAME"
  pkill -x macmischer || true
  sleep 1
fi

echo "==> Installing to $DEST"
rm -rf "$DEST"
cp -R "$APP" "$DEST"
touch "$DEST"

echo "==> Removing build bundles"
rm -rf "$BUNDLE_DIR"
if [[ "${1:-}" == "--clean" ]]; then
  cargo clean
else
  # Keep the build cache but tell Spotlight not to index it.
  mkdir -p target
  touch target/.metadata_never_index
fi

echo "Done — $APP_NAME is in /Applications."
echo "If the Dock/Launchpad still shows an old icon: killall Dock"
