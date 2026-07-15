#!/bin/sh
set -eu

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  echo "error: packaging requires an Apple Silicon macOS host" >&2
  exit 1
fi

cd "$(dirname "$0")/.."
CARGO_BUNDLE_VERSION=0.11.0
cargo bundle --version | grep -Fx "cargo-bundle v$CARGO_BUNDLE_VERSION" >/dev/null || {
  echo "error: install cargo-bundle $CARGO_BUNDLE_VERSION with --locked" >&2
  exit 1
}

# The SF2 INFO chunk says only "example", names SWAMI as the editor, and has
# blank copyright/comment fields. Repository history merely records its 2007
# import. A separately reviewed license is therefore mandatory.
SF2_LICENSE=${BASIC_SF2_LICENSE_FILE:-licenses/basic.sf2-LICENSE.txt}
if [ ! -s "$SF2_LICENSE" ]; then
  echo "error: basic.sf2 has no proven distribution license; provide a reviewed license via BASIC_SF2_LICENSE_FILE" >&2
  exit 1
fi

# cargo-bundle 0.11.0 does not accept Cargo's --locked flag. Build explicitly
# with the lockfile first, then let cargo-bundle reuse that release artifact.
cargo build --release --target aarch64-apple-darwin --locked
cargo bundle --release --target aarch64-apple-darwin --format osx
APP=target/aarch64-apple-darwin/release/bundle/osx/FreeWheeling.app
RESOURCES="$APP/Contents/Resources"
FRAMEWORKS="$APP/Contents/Frameworks"
mkdir -p "$RESOURCES/licenses" "$FRAMEWORKS"
rm -rf "$RESOURCES/data"
cp -R ../data "$RESOURCES/data"
cp ../COPYING "$RESOURCES/licenses/COPYING"
cp ../AUTHORS "$RESOURCES/licenses/AUTHORS"
cp "$SF2_LICENSE" "$RESOURCES/licenses/basic.sf2-LICENSE.txt"

# This verbatim notice is present in the name table of both bundled 1.10 TTFs.
python3 - ../data/Vera.ttf ../data/VeraBd.ttf "$RESOURCES/licenses/Bitstream-Vera-NOTICE.txt" <<'PY'
import pathlib, re, sys
notices = []
for name in sys.argv[1:3]:
    data = pathlib.Path(name).read_bytes().replace(b"\0", b"")
    match = re.search(
        b"Copyright \(c\) 2003 by Bitstream, Inc\.\r?\n"
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

/usr/libexec/PlistBuddy -c "Delete :NSMicrophoneUsageDescription" "$APP/Contents/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :NSMicrophoneUsageDescription string FreeWheeling uses audio input to record and loop live sound." "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Delete :CFBundleDocumentTypes" "$APP/Contents/Info.plist" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :CFBundleDocumentTypes array" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleDocumentTypes:0 dict" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleDocumentTypes:0:CFBundleTypeName string FreeWheeling Audio or Scene" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleDocumentTypes:0:CFBundleTypeRole string Editor" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleDocumentTypes:0:CFBundleTypeExtensions array" "$APP/Contents/Info.plist"
for extension in wav aiff aif au flac ogg xml; do
  /usr/libexec/PlistBuddy -c "Add :CFBundleDocumentTypes:0:CFBundleTypeExtensions: string $extension" "$APP/Contents/Info.plist"
done

bundle_dependency() {
  binary=$1
  otool -L "$binary" | tail -n +2 | awk '{print $1}' | while IFS= read -r dependency; do
    case "$dependency" in
      /System/Library/*|/usr/lib/*|@rpath/*|@loader_path/*) continue ;;
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
find "$FRAMEWORKS" -type f -name '*.dylib' -exec codesign --force --sign - {} \;
codesign --force --sign - "$APP"
python3 scripts/verify_macos_bundle.py "$APP"
