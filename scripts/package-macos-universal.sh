#!/bin/sh
set -eu

if [ "$(uname -s)" != Darwin ]; then
  echo "error: packaging requires a macOS host" >&2
  exit 1
fi

cd "$(dirname "$0")/.."
VERSION=${FWP_VERSION:-0.1.0}
./scripts/package-macos-arm64.sh

cargo build --release --target x86_64-apple-darwin --locked
APP=target/aarch64-apple-darwin/release/bundle/osx/FreeWheeling.app
EXECUTABLE="$APP/Contents/MacOS/freewheeling-plus"
UNIVERSAL="$EXECUTABLE.universal"
lipo -create "$EXECUTABLE" target/x86_64-apple-darwin/release/freewheeling-plus -output "$UNIVERSAL"
mv "$UNIVERSAL" "$EXECUTABLE"
codesign --force --sign - "$APP"
python3 scripts/verify_macos_bundle.py "$APP" --architectures arm64 x86_64

DMG=target/aarch64-apple-darwin/release/bundle/osx/FreeWheeling-$VERSION-universal.dmg
rm -f "$DMG"
hdiutil create -volname FreeWheeling -srcfolder "$APP" -ov -format UDZO "$DMG"
printf '%s\n' "$DMG"
