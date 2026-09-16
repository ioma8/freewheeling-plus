#!/bin/sh
# Windows cross-compile build script for freewheeling-plus (mingw-w64 on macOS)
set -eu

# Discover the mingw-w64 toolchain under Homebrew instead of pinning a version;
# `brew upgrade mingw-w64` bumps the versioned Cellar directory.
# Whether `$1` names a higher dotted version than `$2`, comparing each numeric
# field (so 14.2.0 outranks 9.5.0).
highest_version() {
    [ "$1" = "$2" ] && return 1
    first=$(printf '%s\n' "$1" | awk -F/ '{print $(NF-1)}' | grep -oE '[0-9]+(\.[0-9]+)*' | head -n1)
    second=$(printf '%s\n' "$2" | awk -F/ '{print $(NF-1)}' | grep -oE '[0-9]+(\.[0-9]+)*' | head -n1)
    [ -n "$first" ] && [ -n "$second" ] || return 1
    [ "$(printf '%s\n%s\n' "$first" "$second" | sort -t. -k1,1n -k2,2n -k3,3n | tail -n1)" = "$first" ]
}

SYSROOT="${MINGW_SYSROOT:-}"
if [ -z "$SYSROOT" ]; then
    for dir in /opt/homebrew/Cellar/mingw-w64/*/toolchain-x86_64 /usr/local/Cellar/mingw-w64/*/toolchain-x86_64; do
        [ -d "$dir" ] || continue
        # Keep the highest version. A shell `[ "$a" \> "$b" ]` compares
        # lexicographically, where 9.5.0 beats 14.2.0; compare the numeric
        # fields instead.
        if [ -z "$SYSROOT" ] || highest_version "$dir" "$SYSROOT"; then
            SYSROOT="$dir"
        fi
    done
fi
if [ -z "$SYSROOT" ]; then
    echo "mingw-w64 toolchain not found; install with: brew install mingw-w64" >&2
    exit 1
fi

# `find -maxdepth`/`sort -V` are GNU-only; plain globbing keeps this working on
# both macOS and Linux hosts.
GCC_LIBDIR=""
for dir in "$SYSROOT"/lib/gcc/x86_64-w64-mingw32/*/; do
    [ -d "$dir" ] || continue
    case "$(basename "$dir")" in
        *[!0-9.]*) continue ;;
    esac
    if [ -z "$GCC_LIBDIR" ] || highest_version "$dir" "$GCC_LIBDIR"; then
        GCC_LIBDIR="$dir"
    fi
done
GCC_LIBDIR=${GCC_LIBDIR%/}
if [ -z "$GCC_LIBDIR" ]; then
    echo "mingw-w64 GCC runtime not found under $SYSROOT" >&2
    exit 1
fi

# bindgen (used by fluidlite-sys) needs to know the mingw target and include
# paths. Each directory is validated here so a layout change fails clearly
# instead of surfacing as an obscure bindgen/clang error, and any inherited
# value is preserved.
for include in \
    "$SYSROOT/x86_64-w64-mingw32/include" \
    "$GCC_LIBDIR/include" \
    "$GCC_LIBDIR/include-fixed"; do
    [ -d "$include" ] || { echo "missing clang include dir: $include" >&2; exit 1; }
done
export BINDGEN_EXTRA_CLANG_ARGS="${BINDGEN_EXTRA_CLANG_ARGS:+$BINDGEN_EXTRA_CLANG_ARGS }\
--target=x86_64-w64-mingw32 \
-I${SYSROOT}/x86_64-w64-mingw32/include \
-I${GCC_LIBDIR}/include \
-I${GCC_LIBDIR}/include-fixed"

# Caller flags come first so the pinned --target cannot be overridden: cargo
# accepts repeated --target values and would build host artifacts instead.
cargo build "$@" --release --target x86_64-pc-windows-gnu
