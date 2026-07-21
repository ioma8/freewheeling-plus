#!/bin/sh
# Windows cross-compile build script for freewheeling-plus
set -e

SYSROOT="/opt/homebrew/Cellar/mingw-w64/14.0.0_1/toolchain-x86_64"

# bindgen (used by fluidlite-sys) needs to know the mingw target and include paths
export BINDGEN_EXTRA_CLANG_ARGS="\
--target=x86_64-w64-mingw32 \
-I${SYSROOT}/x86_64-w64-mingw32/include \
-I${SYSROOT}/lib/gcc/x86_64-w64-mingw32/16.1.0/include \
-I${SYSROOT}/lib/gcc/x86_64-w64-mingw32/16.1.0/include-fixed"

cargo build --release --target x86_64-pc-windows-gnu "$@"
