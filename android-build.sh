#!/bin/sh
# Android build script for freewheeling-plus
# Requires ANDROID_HOME and ANDROID_NDK_HOME to be set.

export ANDROID_HOME=/Users/jakubkolcar/Library/Android/sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/30.0.15729638
export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/sysroot --target=aarch64-linux-android34"
export "CFLAGS_aarch64-linux-android=--target=aarch64-linux-android34 -Wno-deprecated-declarations"

cargo apk build --release "$@"
