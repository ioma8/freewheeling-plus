#!/bin/sh
# Android build script for freewheeling-plus
set -e

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
    sed "$pattern" "$file" > "$file.tmp"
    mv "$file.tmp" "$file"
}

# SDL 2.26.4 still calls ALooper_pollAll(), which is marked unavailable by
# the Android NDK headers. The APIs have the same signature here, and SDL's
# sensor queue is created without a callback, so pollOnce is the compatible
# replacement for this call site.
SDL2_SENSOR="$HOME/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sdl2-sys-0.38.0/SDL/src/sensor/android/SDL_androidsensor.c"
if [ -f "$SDL2_SENSOR" ] && grep -q "ALooper_pollAll" "$SDL2_SENSOR" 2>/dev/null; then
    portable_sed 's/ALooper_pollAll/ALooper_pollOnce/g' "$SDL2_SENSOR"
    echo "Patched sdl2-sys Android sensor source for current NDK headers"
fi

# sdl2-sys 0.38.0 also emits -lhidapi for Android static builds, although
# bundled SDL builds the Android HID implementation into libSDL2.a.
SDL2_BUILD_RS="$HOME/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sdl2-sys-0.38.0/build.rs"
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

# cargo-apk requires a release signing key even for local builds. Use the
# Android debug key when no release key was configured explicitly; callers can
# still provide CARGO_APK_RELEASE_KEYSTORE[_PASSWORD] or manifest metadata.
if [ -z "${CARGO_APK_RELEASE_KEYSTORE+x}" ] \
    && [ -z "${CARGO_APK_RELEASE_KEYSTORE_PASSWORD+x}" ] \
    && ! grep -q '^\[package\.metadata\.android\.signing\.release\]' Cargo.toml 2>/dev/null \
    && [ -f "$HOME/.android/debug.keystore" ]; then
    export CARGO_APK_RELEASE_KEYSTORE="$HOME/.android/debug.keystore"
    export CARGO_APK_RELEASE_KEYSTORE_PASSWORD=android
    echo "Using Android debug keystore for local release APK signing"
fi

cargo apk build --release --lib "$@"
