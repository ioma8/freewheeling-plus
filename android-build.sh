#!/bin/sh
# Android build script for freewheeling-plus
set -e

export ANDROID_HOME=/Users/jakubkolcar/Library/Android/sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/30.0.15729638
export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/sysroot --target=aarch64-linux-android34"

# Patch sdl2-sys build.rs to add -Wno-deprecated-declarations for NDK 30
# (ALooper_pollAll is marked unavailable in NDK 30 headers).
SDL2_SYS="$HOME/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/sdl2-sys-0.38.0/build.rs"
if [ -f "$SDL2_SYS" ]; then
    # Apply the patch only if not already applied
    if ! grep -q "deprecated-declarations" "$SDL2_SYS" 2>/dev/null; then
        # Insert cflag before cfg.build() at the end of compile_sdl2
        sed -i '' 's/cfg.build()/cfg.cflag("-Wno-deprecated-declarations");\
    cfg.build()/' "$SDL2_SYS"
        echo "Patched sdl2-sys build.rs for NDK 30 compatibility"
    fi
fi

cargo apk build --release "$@"
