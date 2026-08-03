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
aapt2 link -o "$STAGE/base.apk" \
    --manifest "$ROOT/android/AndroidManifest.xml" \
    -I "$PLATFORM/android.jar" \
    -A "$ROOT/data"

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
