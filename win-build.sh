#!/bin/sh
# Windows cross-compile build script for freewheeling-plus (mingw-w64 on macOS)
set -e

# Discover the mingw-w64 toolchain under Homebrew instead of pinning a version;
# `brew upgrade mingw-w64` bumps the versioned Cellar directory.
SYSROOT="${MINGW_SYSROOT:-}"
if [ -z "$SYSROOT" ]; then
    for dir in /opt/homebrew/Cellar/mingw-w64/*/toolchain-x86_64 /usr/local/Cellar/mingw-w64/*/toolchain-x86_64; do
        [ -d "$dir" ] && SYSROOT="$dir" && break
    done
fi
if [ -z "$SYSROOT" ]; then
    echo "mingw-w64 toolchain not found; install with: brew install mingw-w64" >&2
    exit 1
fi

GCC_LIBDIR=$(find "$SYSROOT/lib/gcc/x86_64-w64-mingw32" -maxdepth 1 -type d -name '[0-9]*' | sort -V | tail -1)
if [ -z "$GCC_LIBDIR" ]; then
    echo "mingw-w64 GCC runtime not found under $SYSROOT" >&2
    exit 1
fi

# bindgen (used by fluidlite-sys) needs to know the mingw target and include paths
export BINDGEN_EXTRA_CLANG_ARGS="\
--target=x86_64-w64-mingw32 \
-I${SYSROOT}/x86_64-w64-mingw32/include \
-I${GCC_LIBDIR}/include \
-I${GCC_LIBDIR}/include-fixed"

cargo build --release --target x86_64-pc-windows-gnu "$@"
