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
LIB="$ROOT/target/aarch64-linux-android/release/libfreewheeling_plus.so"
SDL_JAVA="$HOME/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sdl2-sys-0.38.0/SDL/android-project/app/src/main/java"

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
d8 --release --lib "$PLATFORM/android.jar" --output "$STAGE/dex" \
    $(find "$STAGE/classes" -name '*.class')

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
    sed 's/show="1"/show="0"/g' "$ASSET_ROOT/data/browsers.xml" > "$ASSET_ROOT/data/browsers.xml.tmp"
    mv "$ASSET_ROOT/data/browsers.xml.tmp" "$ASSET_ROOT/data/browsers.xml"
    echo "Hidden desktop-only browsers for Android"
fi
# The footswitch interface is a fixed (non-switchable) overlay that renders
# on every interface; there is no MIDI footswitch on a phone, so hide it.
if [ -f "$ASSET_ROOT/data/midifootswitch.xml" ]; then
    sed 's/name="Footswitch" scale="1\.0,1\.3" pos="0\.90,0\.85"/name="Footswitch" scale="1.0,1.3" pos="0.90,0.85" show="0"/' "$ASSET_ROOT/data/midifootswitch.xml" > "$ASSET_ROOT/data/midifootswitch.xml.tmp"
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
    keytool -genkeypair -keystore "$KEYSTORE" \
        -storepass "${CARGO_APK_RELEASE_KEYSTORE_PASSWORD:-android}" \
        -alias androiddebugkey -keypass "${CARGO_APK_RELEASE_KEYSTORE_PASSWORD:-android}" \
        -dname "CN=Android Debug,O=Android,C=US" -keyalg RSA -validity 3650 >/dev/null 2>&1 || true
fi
if [ ! -f "$KEYSTORE" ]; then
    echo "no signing keystore available; set CARGO_APK_RELEASE_KEYSTORE" >&2
    exit 1
fi
apksigner sign --ks "$KEYSTORE" \
    --ks-pass "pass:${CARGO_APK_RELEASE_KEYSTORE_PASSWORD:-android}" \
    --out "$OUT/freewheeling-plus.apk" "$STAGE/aligned.apk"

apksigner verify --verbose "$OUT/freewheeling-plus.apk" | head -3
echo "runnable APK: $OUT/freewheeling-plus.apk"
