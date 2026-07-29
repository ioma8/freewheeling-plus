#!/bin/sh
# Android build script for freewheeling-plus
set -e

export ANDROID_HOME=/Users/jakubkolcar/Library/Android/sdk
NDK_VERSION="${ANDROID_NDK_VERSION:-28.2.13676358}"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/$NDK_VERSION"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
export ANDROID_NDK_PATH="$ANDROID_NDK_HOME"
export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/sysroot --target=aarch64-linux-android34"
export CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}"

if [ ! -d "$ANDROID_NDK_HOME" ]; then
    echo "Android NDK not found: $ANDROID_NDK_HOME" >&2
    exit 1
fi

# SDL 2.26.4 still calls ALooper_pollAll(), which is marked unavailable by
# the Android NDK headers. The APIs have the same signature here, and SDL's
# sensor queue is created without a callback, so pollOnce is the compatible
# replacement for this call site.
SDL2_SENSOR="$HOME/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sdl2-sys-0.38.0/SDL/src/sensor/android/SDL_androidsensor.c"
if [ -f "$SDL2_SENSOR" ] && grep -q "ALooper_pollAll" "$SDL2_SENSOR" 2>/dev/null; then
    sed -i '' 's/ALooper_pollAll/ALooper_pollOnce/g' "$SDL2_SENSOR"
    echo "Patched sdl2-sys Android sensor source for current NDK headers"
fi

# sdl2-sys 0.38.0 also emits -lhidapi for Android static builds, although
# bundled SDL builds the Android HID implementation into libSDL2.a.
SDL2_BUILD_RS="$HOME/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sdl2-sys-0.38.0/build.rs"
if [ -f "$SDL2_BUILD_RS" ] && grep -q 'cargo:rustc-link-lib=hidapi' "$SDL2_BUILD_RS" 2>/dev/null; then
    sed -i '' '/cargo:rustc-link-lib=hidapi/d' "$SDL2_BUILD_RS"
    echo "Patched sdl2-sys Android HIDAPI link directive"
fi
if [ -f "$SDL2_BUILD_RS" ] && ! grep -q 'cargo:rustc-link-lib=c++_static' "$SDL2_BUILD_RS" 2>/dev/null; then
    sed -i '' 's/println!("cargo:rustc-link-lib=OpenSLES");/println!("cargo:rustc-link-lib=OpenSLES");\
            println!("cargo:rustc-link-lib=c++_static");/' "$SDL2_BUILD_RS"
    echo "Patched sdl2-sys Android C++ runtime link directive"
fi
if [ -f "$SDL2_BUILD_RS" ] && ! grep -q 'cargo:rustc-link-lib=c++abi' "$SDL2_BUILD_RS" 2>/dev/null; then
    sed -i '' 's/println!("cargo:rustc-link-lib=c++_static");/println!("cargo:rustc-link-lib=c++_static");\
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
