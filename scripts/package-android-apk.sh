#!/bin/sh
# Assemble a runnable FreeWheeling+ APK with SDL's Java glue.
#
# cargo-apk only produces a NativeActivity package (android.app.NativeActivity)
# and has no Java support, but SDL2's Android backend is driven from Java
# (org.libsdl.app.SDLActivity). This script takes the cargo-apk-built cdylib
# and wraps it in a proper package: SDL's Java glue compiled to classes.dex, a
# manifest whose launcher activity is our SDLActivity subclass, the native
# library, and the bundled data/ tree as assets. Output is aligned for 16 KiB
# page-size devices and signed with the debug keystore.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$ROOT"

if [ -n "${ANDROID_HOME:-}" ] && [ -d "$ANDROID_HOME" ]; then
    : # caller-provided
elif [ -d "$HOME/Library/Android/sdk" ]; then
    ANDROID_HOME="$HOME/Library/Android/sdk"
elif [ -d "$HOME/Android/Sdk" ]; then
    ANDROID_HOME="$HOME/Android/Sdk"
elif [ -d /usr/local/lib/android/sdk ]; then
    ANDROID_HOME=/usr/local/lib/android/sdk
else
    echo "Android SDK not found; set ANDROID_HOME" >&2
    exit 1
fi
export ANDROID_HOME

BUILD_TOOLS=$(ls -d "$ANDROID_HOME"/build-tools/* 2>/dev/null | sort -V | tail -1)
PLATFORM=$(ls -d "$ANDROID_HOME"/platforms/android-* 2>/dev/null | sort -V | tail -1)
if [ -z "$BUILD_TOOLS" ] || [ -z "$PLATFORM" ]; then
    echo "Android build-tools or platforms missing under $ANDROID_HOME" >&2
    exit 1
fi

STAGE=${FWP_ANDROID_STAGE:-$ROOT/target/android-stage}
OUT=$ROOT/target/release/apk
mkdir -p "$OUT"
LIB="$ROOT/target/aarch64-linux-android/release/libfreewheeling_plus.so"
# Resolved from the cargo registry rather than a hardcoded index hash and
# version, so a registry bump or a vendored CARGO_HOME cannot silently skip the
# Java-source staging below.
REGISTRY_SRC=$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -maxdepth 1 -type d -name 'index.crates.io-*' 2>/dev/null | head -n 1)
SDL2_SYS_DIR=$(find "$REGISTRY_SRC" -maxdepth 1 -type d -name 'sdl2-sys-*' 2>/dev/null | head -n 1)
if [ -z "$REGISTRY_SRC" ] || [ -z "$SDL2_SYS_DIR" ]; then
    echo "error: sdl2-sys source not found under ${CARGO_HOME:-$HOME/.cargo}/registry/src; run cargo fetch first" >&2
    exit 1
fi
SDL_JAVA="$SDL2_SYS_DIR/SDL/android-project/app/src/main/java"

for command in javac java aapt2 d8 zipalign apksigner; do
    command -v "$command" >/dev/null 2>&1 || PATH="$BUILD_TOOLS:$PATH"
done
command -v java >/dev/null 2>&1 || { echo "JDK (javac/java) is required to assemble the Android APK" >&2; exit 1; }

if [ ! -f "$LIB" ]; then
    echo "Android cdylib not found: $LIB (run android-build.sh first)" >&2
    exit 1
fi

rm -rf "$STAGE"
mkdir -p "$STAGE/classes"

# 1. Compile SDL's Java glue plus the FreeWheelingActivity subclass.
find "$SDL_JAVA" "$ROOT/android/java" -name '*.java' > "$STAGE/sources.txt"
javac --release 8 -nowarn -classpath "$PLATFORM/android.jar" \
    -d "$STAGE/classes" @"$STAGE/sources.txt"

# 2. Dex the compiled classes.
mkdir -p "$STAGE/dex"
# The class paths are passed as an argument vector (not through shell word
# splitting), so a checkout path containing whitespace cannot split them.
python3 - "$PLATFORM/android.jar" "$STAGE/classes" "$STAGE/dex" <<'CLASSES'
import pathlib, subprocess, sys

android_jar, classes, output = sys.argv[1:4]
inputs = sorted(str(path) for path in pathlib.Path(classes).rglob("*.class"))
if not inputs:
    raise SystemExit(f"error: no compiled classes under {classes}")
subprocess.run(
    ["d8", "--release", "--lib", android_jar, "--output", output, *inputs],
    check=True,
)
CLASSES

# 3. Link the base APK: manifest, resources, and assets (bundled data/).
# aapt2 -A adds a directory's *contents* at the asset root, so passing
# data/ directly would flatten it (assets/fweelin.xml instead of
# assets/data/fweelin.xml). Stage the tree under a data/ prefix so the
# activity can extract assets/data/ -> files/data at first launch.
ASSET_ROOT="$STAGE/assets-root"
mkdir -p "$ASSET_ROOT/data"
cp -R "$ROOT/data/." "$ASSET_ROOT/data/"
# On Android the mobile layout's action buttons occupy the band just above
# the meters, so compact the coreinterface display cluster into the bottom
# ~20% of the screen: smaller meters, and the right-edge status switches
# moved down below the buttons. Desktop-only displays (midi transpose text,
# CPU/overdub bars, inputs 3-4, and the keyboard-mode switches) are hidden;
# the phone keeps the IN/OUT/LMT meters plus the stereo input levels.
if [ -f "$ASSET_ROOT/data/coreinterface.xml" ]; then
    # Hide pass runs first, on the original coordinates.
    sed 's/title="Xp " pos="0\.0,0\.9"/title="Xp " pos="0.0,0.9" show="0"/; s/title="CPU" pos="0\.05,0\.8"/title="CPU" pos="0.05,0.8" show="0"/; s/pos="0\.26,0\.8"/pos="0.26,0.8" show="0"/g; s/pos="0\.29,0\.8"/pos="0.29,0.8" show="0"/g; s/title="FBK" pos="0\.75,0\.8"/title="FBK" pos="0.75,0.8" show="0"/; s/pos="0\.895,0\.64" title="SYNTH"/pos="0.895,0.64" title="SYNTH" show="0"/; s/pos="0\.914,0\.68" *$/pos="0.914,0.68" show="0"/; s/pos="0\.925,0\.6" *$/pos="0.925,0.6" show="0"/; s/pos="0\.925,0\.72" *$/pos="0.925,0.72" show="0"/; s/pos="0\.925,0\.76" *$/pos="0.925,0.76" show="0"/' "$ASSET_ROOT/data/coreinterface.xml" > "$ASSET_ROOT/data/coreinterface.xml.tmp"
    # Compaction pass: smaller meters, switches below the buttons.
    sed 's/barscale="0\.3"/barscale="0.14"/g; s/pos="\([0-9.]*\),0\.8"/pos="\1,0.94"/g; s/0\.925,0\.6"/0.925,0.80"/g; s/0\.895,0\.64"/0.895,0.82"/g; s/0\.914,0\.68"/0.914,0.84"/g; s/0\.925,0\.72"/0.925,0.86"/g; s/0\.925,0\.76"/0.925,0.88"/g' "$ASSET_ROOT/data/coreinterface.xml.tmp" > "$ASSET_ROOT/data/coreinterface.xml"
    rm "$ASSET_ROOT/data/coreinterface.xml.tmp"
    echo "Compacted coreinterface display cluster for Android"
fi
# The patch browser and loop tray render at the very bottom of every
# interface; on the phone the grid replaces them (and there is no keyboard
# to switch browsers with). Hide them in the Android assets.
if [ -f "$ASSET_ROOT/data/browsers.xml" ]; then
    # Hide exactly the two desktop browsers by id: a blanket
    # `show="1" -> 0` rewrite would also hide a display added later, and a
    # count-based check cannot tell the two apart.
    python3 - "$ASSET_ROOT/data/browsers.xml" <<'HIDE'
import pathlib, re, sys

path = pathlib.Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
hidden = ("DISPLAY_browser_patch", "DISPLAY_loop_tray")
for display_id in hidden:
    pattern = re.compile(r'<display\b[^>]*?id="' + re.escape(display_id) + r'"[^>]*?>', re.S)
    tag = pattern.search(text)
    if tag is None:
        raise SystemExit(f"error: {display_id} not found in {path}")
    rewritten = tag.group(0).replace('show="1"', 'show="0"')
    if rewritten == tag.group(0):
        raise SystemExit(f"error: {display_id} has no visible show=\"1\" in {path}")
    text = text[: tag.start()] + rewritten + text[tag.end() :]
path.write_text(text, encoding="utf-8")
print("hidden desktop-only displays: " + ", ".join(hidden))
HIDE
    for display_id in DISPLAY_browser_patch DISPLAY_loop_tray; do
        grep -qE "<display[^>]*id=\"$display_id\"[^>]*show=\"0\"" "$ASSET_ROOT/data/browsers.xml" || {
            echo "error: $display_id is still visible in the Android assets" >&2
            exit 1
        }
    done
fi
# The scene browser is the mobile saved-sessions list: give it a large box
# above the action buttons instead of the desktop bottom strip.
if [ -f "$ASSET_ROOT/data/browsers.xml" ]; then
    python3 - "$ASSET_ROOT/data/browsers.xml" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path).read()
# Match the whole DISPLAY_browser_scene display block and rewrite its
# position/size; the other browsers keep their desktop geometry.
text, n = re.subn(
    r'(id="DISPLAY_browser_scene"[^>]*pos=")[^"]*("[\s\S]*?xbox=")[^"]*(")',
    r'\g<1>0.0,0.0\g<2>0.05,0.08, 0.95,0.50\g<3>',
    text,
)
assert n == 1, f"DISPLAY_browser_scene block not matched ({n})"
open(path, "w").write(text)
PY
    echo "Resized the scene browser for the mobile sessions list"
fi
# The footswitch interface is a fixed (non-switchable) overlay that renders
# on every interface; there is no MIDI footswitch on a phone, so hide it.
if [ -f "$ASSET_ROOT/data/midifootswitch.xml" ]; then
    sed 's/namepos="0\.02,0\.02" show="1"/namepos="0.02,0.02" show="0"/' "$ASSET_ROOT/data/midifootswitch.xml" > "$ASSET_ROOT/data/midifootswitch.xml.tmp"
    mv "$ASSET_ROOT/data/midifootswitch.xml.tmp" "$ASSET_ROOT/data/midifootswitch.xml"
    echo "Hidden footswitch overlay for Android"
fi
aapt2 link -o "$STAGE/base.apk" \
    --manifest "$ROOT/android/AndroidManifest.xml" \
    -I "$PLATFORM/android.jar" \
    -A "$ASSET_ROOT"

# 4. Add the dex and the native library (16 KiB-aligned on repack).
mkdir -p "$STAGE/payload/lib/arm64-v8a"
cp "$STAGE/dex/classes.dex" "$STAGE/payload/classes.dex"
cp "$LIB" "$STAGE/payload/lib/arm64-v8a/"
(cd "$STAGE/payload" && zip -q -r "$STAGE/base.apk" classes.dex lib/)
# Native libraries must be stored uncompressed so they can be mapped
# directly from the APK on modern (16 KiB page-size) devices.
(cd "$STAGE/payload" && zip -q -0 "$STAGE/base.apk" lib/arm64-v8a/libfreewheeling_plus.so)

# 5. Align (16 KiB page-size compatible) and sign with the debug keystore.
zipalign -f -P 16 4 "$STAGE/base.apk" "$STAGE/aligned.apk"
KEYSTORE="${CARGO_APK_RELEASE_KEYSTORE:-$HOME/.android/debug.keystore}"
if [ ! -f "$KEYSTORE" ] && command -v keytool >/dev/null 2>&1; then
    mkdir -p "$HOME/.android"
    # Let keytool's diagnostic surface: `|| true` used to hide a read-only
    # $HOME/.android or a weak crypto policy behind a misleading message.
    export CARGO_APK_RELEASE_KEYSTORE_PASSWORD="${CARGO_APK_RELEASE_KEYSTORE_PASSWORD:-android}"
    # `:env` (JDK 9+) reads the password from the environment: passing it as
    # `-storepass <value>`/`-keypass <value>` would expose it through ps//proc.
    keytool -genkeypair -keystore "$KEYSTORE" \
        -storepass:env CARGO_APK_RELEASE_KEYSTORE_PASSWORD \
        -alias androiddebugkey -keypass:env CARGO_APK_RELEASE_KEYSTORE_PASSWORD \
        -dname "CN=Android Debug,O=Android,C=US" -keyalg RSA -validity 3650
fi
if [ ! -f "$KEYSTORE" ]; then
    echo "no signing keystore available; set CARGO_APK_RELEASE_KEYSTORE (or look at the keytool error above)" >&2
    exit 1
fi
# The password is passed through the environment: a command-line `--ks-pass`
# is world-readable through `ps`/`/proc`.
export CARGO_APK_RELEASE_KEYSTORE_PASSWORD="${CARGO_APK_RELEASE_KEYSTORE_PASSWORD:-android}"
apksigner sign --ks "$KEYSTORE" \
    --ks-pass "env:CARGO_APK_RELEASE_KEYSTORE_PASSWORD" \
    --out "$OUT/freewheeling-plus.apk" "$STAGE/aligned.apk"

apksigner verify --verbose "$OUT/freewheeling-plus.apk" | head -3
echo "runnable APK: $OUT/freewheeling-plus.apk"
