#!/bin/sh
# Build a universal (arm64 + x86_64) FreeWheeling.app and wrap it in a DMG.
#
# The result is ad-hoc signed (`codesign --sign -`): it is a local/CI artifact
# and Gatekeeper will warn on other machines. Distribution requires a Developer
# ID identity plus notarization; set CODESIGN_IDENTITY (and optionally
# NOTARY_PROFILE) to sign and staple properly.
set -eu

if [ "$(uname -s)" != Darwin ]; then
  echo "error: packaging requires a macOS host" >&2
  exit 1
fi

cd "$(dirname "$0")/.."
# Same rule as scripts/linux/package-release.sh: the version names the artifact
# that wraps the bundle, so it comes from the manifest and an override must agree
# with it.
MANIFEST_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$MANIFEST_VERSION" ] || {
  echo "error: cannot read the version from Cargo.toml" >&2
  exit 2
}
VERSION=${FWP_VERSION:-$MANIFEST_VERSION}
[ "$VERSION" = "$MANIFEST_VERSION" ] || {
  echo "error: FWP_VERSION=$VERSION does not match Cargo.toml ($MANIFEST_VERSION); bump the manifest or drop FWP_VERSION" >&2
  exit 2
}
ARM_TARGET=aarch64-apple-darwin
INTEL_TARGET=x86_64-apple-darwin

for command in cargo lipo hdiutil python3 codesign; do
  command -v "$command" >/dev/null 2>&1 || { echo "error: missing command: $command" >&2; exit 127; }
done
[ -x ./scripts/package-macos-arm64.sh ] || { echo "error: scripts/package-macos-arm64.sh is missing or not executable" >&2; exit 1; }
if command -v rustup >/dev/null 2>&1; then
  for target in "$ARM_TARGET" "$INTEL_TARGET"; do
    rustup target list --installed | grep -qx "$target" || {
      echo "error: rust target $target is not installed (rustup target add $target)" >&2
      exit 1
    }
  done
fi

# The arm64 bundle goes through cargo-bundle; the x86_64 slice is built with the
# same profile and features so the two halves cannot diverge.
./scripts/package-macos-arm64.sh
cargo build --release --target "$INTEL_TARGET" --locked

APP=target/$ARM_TARGET/release/bundle/osx/FreeWheeling.app
EXECUTABLE="$APP/Contents/MacOS/freewheeling-plus"
FRAMEWORKS="$APP/Contents/Frameworks"
# The merged binary is written outside Contents/ and cleaned up on every exit
# path, so a failed run cannot leave a stray file inside the bundle.
UNIVERSAL=$(mktemp -t freewheeling-universal)
trap 'rm -f "$UNIVERSAL"' EXIT INT TERM
lipo -create "$EXECUTABLE" target/$INTEL_TARGET/release/freewheeling-plus -output "$UNIVERSAL"
# `lipo` refuses to merge an already-fat input: rebuilding the arm64 bundle
# above makes this idempotent.
mv "$UNIVERSAL" "$EXECUTABLE"

# Each bundled dylib must carry both slices too: the bundle ships the arm64
# copies, so an x86_64-only library would fail verification (or, worse, launch
# under Rosetta against arm64-only code).
for framework in "$FRAMEWORKS"/*; do
  [ -f "$framework" ] || continue
  if ! lipo -archs "$framework" | grep -qw x86_64; then
    echo "error: bundled dependency has no x86_64 slice: $framework" >&2
    echo "       rebuild it as a universal binary before packaging" >&2
    exit 1
  fi
done

if [ -n "${CODESIGN_IDENTITY:-}" ]; then
  codesign --force --options runtime --sign "$CODESIGN_IDENTITY" "$APP"
else
  echo "warning: ad-hoc signing; the result is not distributable and Gatekeeper will warn" >&2
  codesign --force --sign - "$APP"
fi
python3 scripts/verify_macos_bundle.py "$APP" --architectures arm64 x86_64

DMG=target/$ARM_TARGET/release/bundle/osx/FreeWheeling-$VERSION-universal.dmg
rm -f "$DMG"
hdiutil create -volname FreeWheeling -srcfolder "$APP" -ov -format UDZO "$DMG"
if [ -n "${CODESIGN_IDENTITY:-}" ]; then
  # Sign the disk image too, and notarize/staple it when a notary profile is
  # configured: an unsigned DMG is blocked by Gatekeeper on the user's machine.
  codesign --force --sign "$CODESIGN_IDENTITY" "$DMG"
  if [ -n "${NOTARY_PROFILE:-}" ]; then
    xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$DMG"
  else
    echo "warning: NOTARY_PROFILE is unset; the DMG is signed but not notarized" >&2
  fi
fi
printf '%s\n' "$DMG"
