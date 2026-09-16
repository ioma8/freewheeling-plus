#!/bin/sh
set -eu

if [ "$(uname -s)" != Darwin ]; then
  echo "error: packaging requires a macOS host" >&2
  exit 1
fi

cd "$(dirname "$0")/.."
CARGO_BUNDLE_VERSION=0.11.0
cargo bundle --version | grep -Fx "cargo-bundle v$CARGO_BUNDLE_VERSION" >/dev/null || {
  echo "error: install cargo-bundle $CARGO_BUNDLE_VERSION with --locked" >&2
  exit 1
}

# cargo-bundle 0.11.0 does not accept Cargo's --locked flag. Build explicitly
# with the lockfile first, then let cargo-bundle reuse that release artifact.
cargo build --release --target aarch64-apple-darwin --locked
cargo bundle --release --target aarch64-apple-darwin --format osx
APP=target/aarch64-apple-darwin/release/bundle/osx/FreeWheeling.app
RESOURCES="$APP/Contents/Resources"
FRAMEWORKS="$APP/Contents/Frameworks"
mkdir -p "$RESOURCES/licenses" "$FRAMEWORKS"
# Start from a clean bundle: leftovers from an earlier run (including
# Frameworks/*.dylib, which the copy below would keep) must not ship.
rm -rf "$FRAMEWORKS"
mkdir -p "$FRAMEWORKS"
rm -rf "$RESOURCES/data"
cp -R data "$RESOURCES/data"
cp COPYING "$RESOURCES/licenses/COPYING"
cp AUTHORS "$RESOURCES/licenses/AUTHORS"

# This verbatim notice is present in the name table of both bundled 1.10 TTFs.
python3 - data/Vera.ttf data/VeraBd.ttf "$RESOURCES/licenses/Bitstream-Vera-NOTICE.txt" <<'PY'
import pathlib, re, sys
notices = []
for name in sys.argv[1:3]:
    data = pathlib.Path(name).read_bytes().replace(b"\0", b"")
    match = re.search(
        rb"Copyright \(c\) 2003 by Bitstream, Inc\.\r?\n"
        b"All Rights Reserved\..*?fonts at gnome dot org",
        data,
        re.S,
    )
    if not match:
        raise SystemExit(f"error: embedded Bitstream Vera license not found in {name}")
    notices.append(match.group().decode("latin-1"))
if notices[0] != notices[1]:
    raise SystemExit("error: bundled Vera fonts contain different license notices")
pathlib.Path(sys.argv[3]).write_text(notices[0] + "\n", encoding="utf-8")
PY

# Every mutation reports which command failed instead of aborting with a raw
# PlistBuddy error under `set -e`.
plist_set() {
  if ! /usr/libexec/PlistBuddy -c "$1" "$APP/Contents/Info.plist"; then
    echo "error: PlistBuddy failed: $1" >&2
    exit 1
  fi
}
plist_set "Delete :NSMicrophoneUsageDescription" 2>/dev/null || true
plist_set "Add :NSMicrophoneUsageDescription string FreeWheeling uses audio input to record and loop live sound."
# cargo-bundle's generated document types are replaced, so a type it adds from
# `Cargo.toml` later would be dropped silently unless it is added here too:
# this list is the single source of truth for what the bundle advertises.
DOCUMENT_EXTENSIONS="wav aiff aif au flac ogg xml"
plist_set "Delete :CFBundleDocumentTypes" 2>/dev/null || true
plist_set "Add :CFBundleDocumentTypes array"
plist_set "Add :CFBundleDocumentTypes:0 dict"
plist_set "Add :CFBundleDocumentTypes:0:CFBundleTypeName string FreeWheeling Audio or Scene"
plist_set "Add :CFBundleDocumentTypes:0:CFBundleTypeRole string Editor"
plist_set "Add :CFBundleDocumentTypes:0:CFBundleTypeExtensions array"
for extension in $DOCUMENT_EXTENSIONS; do
  plist_set "Add :CFBundleDocumentTypes:0:CFBundleTypeExtensions: string $extension"
done
declared=$(/usr/libexec/PlistBuddy -c "Print :CFBundleDocumentTypes:0:CFBundleTypeExtensions" \
  "$APP/Contents/Info.plist" | grep -c '^    ')
[ "$declared" -eq "$(printf '%s\n' $DOCUMENT_EXTENSIONS | wc -l | tr -d ' ')" ] || {
  echo "error: the bundle declares $declared document extensions, expected the $DOCUMENT_EXTENSIONS list" >&2
  exit 1
}

bundle_dependency() {
  binary=$1
  [ -f "$binary" ] || { echo "error: cannot inspect missing binary: $binary" >&2; exit 1; }
  # `sed` strips the trailing "(compatibility ...)" text: `awk '{print $1}'`
  # would truncate any dependency path containing a space.
  otool -L "$binary" | tail -n +2 | sed -e 's/^[[:space:]]*//' -e 's/ (compatibility.*//' | while IFS= read -r dependency; do
    [ -n "$dependency" ] || continue
    case "$dependency" in
      /System/Library/*|/usr/lib/*) continue ;;
      @rpath/*|@loader_path/*|@executable_path/*)
        echo "warning: pre-linked dependency was not bundled: $dependency (in $binary)" >&2
        continue ;;
    esac
    [ -f "$dependency" ] || { echo "error: unresolved dependency: $dependency" >&2; exit 1; }
    target="$FRAMEWORKS/$(basename "$dependency")"
    if [ ! -f "$target" ]; then
      cp "$dependency" "$target"
      chmod u+w "$target"
      install_name_tool -id "@rpath/$(basename "$dependency")" "$target"
      bundle_dependency "$target"
    fi
    install_name_tool -change "$dependency" "@rpath/$(basename "$dependency")" "$binary"
  done
}

bundle_dependency "$APP/Contents/MacOS/freewheeling-plus"
# Every regular file in Frameworks is signed: a framework binary copied by
# basename (or a .so) would otherwise break the seal.
find "$FRAMEWORKS" -type f -exec codesign --force --sign - {} \;
codesign --force --sign - "$APP"
python3 scripts/verify_macos_bundle.py "$APP"
