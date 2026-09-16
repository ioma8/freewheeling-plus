#!/bin/sh
# Android build script for freewheeling-plus
set -e

ROOT=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
# Every relative path below (Cargo.toml, fetch, cargo apk, the packaging
# script) must resolve against the project, not the caller's directory.
cd "$ROOT"

# Locate the Android SDK: honor an explicit ANDROID_HOME/ANDROID_SDK_ROOT,
# then fall back to the conventional per-platform install locations.
if [ -n "${ANDROID_HOME:-}" ] && [ -d "$ANDROID_HOME" ]; then
    :
elif [ -n "${ANDROID_SDK_ROOT:-}" ] && [ -d "$ANDROID_SDK_ROOT" ]; then
    export ANDROID_HOME="$ANDROID_SDK_ROOT"
elif [ -d "$HOME/Library/Android/sdk" ]; then
    export ANDROID_HOME="$HOME/Library/Android/sdk"
elif [ -d "$HOME/Android/Sdk" ]; then
    export ANDROID_HOME="$HOME/Android/Sdk"
elif [ -d "$ANDROID_SDK_HOME" ]; then
    export ANDROID_HOME="$ANDROID_SDK_HOME"
else
    echo "Android SDK not found; set ANDROID_HOME" >&2
    exit 1
fi

NDK_VERSION="${ANDROID_NDK_VERSION:-28.2.13676358}"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/$NDK_VERSION"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
export ANDROID_NDK_PATH="$ANDROID_NDK_HOME"

case "$(uname -s)" in
    Darwin) PREBUILT=darwin-x86_64 ;;
    Linux) PREBUILT=linux-x86_64 ;;
    *) echo "unsupported host for Android NDK builds: $(uname -s)" >&2; exit 1 ;;
esac

export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$PREBUILT/sysroot --target=aarch64-linux-android34"
export CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}"

if [ ! -d "$ANDROID_NDK_HOME" ]; then
    echo "Android NDK not found: $ANDROID_NDK_HOME" >&2
    exit 1
fi

# Fetch dependency sources before patching: on a fresh cache the sdl2-sys
# source is only extracted when cargo builds it, and the Android workarounds
# below must apply before that build runs.
cargo fetch --target aarch64-linux-android

# Apply a sed program in place portably: BSD sed wants `-i ''` while GNU sed
# rejects it, so write to a temporary file and move it over the original.
portable_sed() {
    pattern=$1
    file=$2
    if ! sed "$pattern" "$file" > "$file.tmp"; then
        rm -f "$file.tmp"
        echo "error: sed failed for pattern '$pattern' in $file" >&2
        return 1
    fi
    if cmp -s "$file.tmp" "$file"; then
        # `sed` exits 0 when nothing matched: an unapplied patch must be
        # visible instead of silently reporting success.
        rm -f "$file.tmp"
        echo "warning: pattern '$pattern' matched nothing in $file" >&2
        return 1
    fi
    cat "$file.tmp" > "$file"
    rm -f "$file.tmp"
}

# SDL 2.26.4 still calls ALooper_pollAll(), which is marked unavailable by
# the Android NDK headers. The APIs have the same signature here, and SDL's
# sensor queue is created without a callback, so pollOnce is the compatible
# replacement for this call site.
REGISTRY_SRC=$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -maxdepth 1 -type d -name 'index.crates.io-*' 2>/dev/null | head -n 1)
SDL2_SYS_DIR=$(find "$REGISTRY_SRC" -maxdepth 1 -type d -name 'sdl2-sys-*' 2>/dev/null | head -n 1)
if [ -z "$REGISTRY_SRC" ] || [ -z "$SDL2_SYS_DIR" ]; then
    echo "error: sdl2-sys source not found under ${CARGO_HOME:-$HOME/.cargo}/registry/src; run cargo fetch first" >&2
    exit 1
fi
SDL2_SENSOR="$SDL2_SYS_DIR/SDL/src/sensor/android/SDL_androidsensor.c"
if [ -f "$SDL2_SENSOR" ] && grep -q "ALooper_pollAll" "$SDL2_SENSOR" 2>/dev/null; then
    portable_sed 's/ALooper_pollAll/ALooper_pollOnce/g' "$SDL2_SENSOR"
    echo "Patched sdl2-sys Android sensor source for current NDK headers"
fi

# sdl2-sys 0.38.0 also emits -lhidapi for Android static builds, although
# bundled SDL builds the Android HID implementation into libSDL2.a.
SDL2_BUILD_RS="$SDL2_SYS_DIR/build.rs"
if [ -f "$SDL2_BUILD_RS" ] && grep -q 'cargo:rustc-link-lib=hidapi' "$SDL2_BUILD_RS" 2>/dev/null; then
    portable_sed '/cargo:rustc-link-lib=hidapi/d' "$SDL2_BUILD_RS"
    echo "Patched sdl2-sys Android HIDAPI link directive"
fi
if [ -f "$SDL2_BUILD_RS" ] && ! grep -q 'cargo:rustc-link-lib=c++_static' "$SDL2_BUILD_RS" 2>/dev/null; then
    portable_sed 's/println!("cargo:rustc-link-lib=OpenSLES");/println!("cargo:rustc-link-lib=OpenSLES");\
            println!("cargo:rustc-link-lib=c++_static");/' "$SDL2_BUILD_RS"
    echo "Patched sdl2-sys Android C++ runtime link directive"
fi
if [ -f "$SDL2_BUILD_RS" ] && ! grep -q 'cargo:rustc-link-lib=c++abi' "$SDL2_BUILD_RS" 2>/dev/null; then
    portable_sed 's/println!("cargo:rustc-link-lib=c++_static");/println!("cargo:rustc-link-lib=c++_static");\
            println!("cargo:rustc-link-lib=c++abi");/' "$SDL2_BUILD_RS"
    echo "Patched sdl2-sys Android C++ ABI link directive"
fi

# cpal 0.18.1 opens Android capture streams without setting an AAudio input
# preset, so AAudio applies its default VOICE_RECOGNITION preset. On real
# devices that routes the microphone through the voice-processing pipeline
# (AGC, noise suppression, band-limiting), which makes recorded loops sound
# like a telephone call. Open the capture stream as UNPROCESSED so the looper
# records the raw microphone.
CPAL_DIR=$(find "$REGISTRY_SRC" -maxdepth 1 -type d -name 'cpal-*' 2>/dev/null | head -n 1)
if [ -z "$CPAL_DIR" ]; then
    echo "error: cpal source not found under ${CARGO_HOME:-$HOME/.cargo}/registry/src; run cargo fetch first" >&2
    exit 1
fi
CPAL_AAUDIO="$CPAL_DIR/src/host/aaudio/mod.rs"
if [ -f "$CPAL_AAUDIO" ] && ! grep -q 'input_preset(ndk::audio::AudioInputPreset::Unprocessed)' "$CPAL_AAUDIO" 2>/dev/null; then
    portable_sed '/fn build_input_stream<D, E>(/,/let builder = configure_for_device(builder, device, config);/ {
        s/let builder = configure_for_device(builder, device, config);/let builder = configure_for_device(builder, device, config);\
    let builder = builder.input_preset(ndk::audio::AudioInputPreset::Unprocessed);/
    }' "$CPAL_AAUDIO"
    echo "Patched cpal AAudio input preset to UNPROCESSED"
fi

# cpal requests AAUDIO_PERFORMANCE_MODE_NONE without its "realtime" feature,
# which on Android falls back to the Legacy (AudioFlinger-mixed) path. The
# Legacy input thread delivers capture in large bursts, which starves the
# app's steady 128-frame playback. Request LowLatency (MMAP) on Android so
# the streams get regular low-latency delivery where the device supports it.
# Only the configure_for_device block is changed: the data-callback realtime
# guards (stream.performance_mode() != LowLatency) would otherwise emit
# RealtimeDenied errors on Android, so they must keep their original cfg.
if [ -f "$CPAL_AAUDIO" ] && ! grep -q 'any(feature = "realtime", target_os = "android")' "$CPAL_AAUDIO" 2>/dev/null; then
    CPAL_TMP=$(mktemp)
    awk '
        {
            if ($0 ~ /builder = builder.performance_mode\(ndk::audio::AudioPerformanceMode::LowLatency\);/) {
                if (NR >= 3 && lines[NR-1] ~ /^    \{$/ && lines[NR-2] ~ /#\[cfg\(feature = "realtime"\)\]/) {
                    gsub(/cfg\(feature = "realtime"\)/, "cfg(any(feature = \"realtime\", target_os = \"android\"))", lines[NR-2]);
                }
            }
            lines[NR] = $0;
        }
        END { for (i = 1; i <= NR; i++) print lines[i]; }
    ' "$CPAL_AAUDIO" > "$CPAL_TMP"
    # Copy the rewrite back through the original inode (a `mv` would reset its
    # mode) and only report success when the new cfg is actually present.
    if grep -q 'any(feature = "realtime", target_os = "android")' "$CPAL_TMP"; then
        cat "$CPAL_TMP" > "$CPAL_AAUDIO"
        rm -f "$CPAL_TMP"
        echo "Patched cpal AAudio low-latency (MMAP) performance mode on Android"
    else
        rm -f "$CPAL_TMP"
        echo "error: cpal AAudio performance-mode patch did not apply" >&2
        exit 1
    fi
fi

# cargo-apk requires a release signing key even for local builds. Use the
# Android debug key when no release key was configured explicitly; callers can
# still provide CARGO_APK_RELEASE_KEYSTORE[_PASSWORD] or manifest metadata.
# CI runners have no Android Studio, so synthesize the standard debug
# keystore (same alias/passwords Android Studio generates) when absent.
if [ ! -f "$HOME/.android/debug.keystore" ] && command -v keytool >/dev/null 2>&1; then
    mkdir -p "$HOME/.android"
    keytool -genkeypair -keystore "$HOME/.android/debug.keystore" \
        -storepass android -alias androiddebugkey -keypass android \
        -dname "CN=Android Debug,O=Android,C=US" -keyalg RSA -validity 3650 \
        >/dev/null 2>&1 || rm -f "$HOME/.android/debug.keystore"
fi
if [ -z "${CARGO_APK_RELEASE_KEYSTORE+x}" ] \
    && [ -z "${CARGO_APK_RELEASE_KEYSTORE_PASSWORD+x}" ] \
    && ! grep -q '^\[package\.metadata\.android\.signing\.release\]' Cargo.toml \
    && [ -f "$HOME/.android/debug.keystore" ]; then
    # The debug keystore's credentials are public: an APK signed with it can
    # never be replaced by a real release and must not be distributed. Fall
    # back to it only for local testing, and say so loudly.
    echo "WARNING: signing with the Android debug keystore (public credentials)." >&2
    echo "WARNING: this APK is for local testing only and must not be distributed." >&2
    if [ "${ALLOW_DEBUG_SIGNING:-0}" != "1" ]; then
        echo "Set ALLOW_DEBUG_SIGNING=1 to accept debug signing, or provide CARGO_APK_RELEASE_KEYSTORE." >&2
        exit 1
    fi
    export CARGO_APK_RELEASE_KEYSTORE="$HOME/.android/debug.keystore"
    export CARGO_APK_RELEASE_KEYSTORE_PASSWORD=android
fi

cargo apk build --release --lib "$@"

# cargo-apk alone cannot package SDL2's Java glue; wrap the cdylib in a
# runnable APK (SDLActivity + classes.dex + data assets).
"$ROOT/scripts/package-android-apk.sh"
