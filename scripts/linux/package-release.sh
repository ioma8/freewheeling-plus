#!/bin/sh
set -eu
umask 022

ROOT=$(CDPATH= cd -- "$(dirname "$0")/../../.." && pwd)
CRATE="$ROOT/freewheeling-plus"
VERSION=${FWP_VERSION:-1.1.0}
TARGET=${FWP_TARGET:-x86_64-unknown-linux-gnu}
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || printf 0)}
OUT=${FWP_DIST_DIR:-$CRATE/dist}
STAGE="$OUT/freewheeling-plus-$VERSION-$TARGET"
ARCHIVE="$STAGE.tar.gz"

case "$VERSION" in *[!0-9A-Za-z._+-]*|'') echo "error: invalid FWP_VERSION" >&2; exit 2;; esac
case "$TARGET" in *[!0-9A-Za-z._-]*|'') echo "error: invalid FWP_TARGET" >&2; exit 2;; esac
case "$SOURCE_DATE_EPOCH" in *[!0-9]*|'') echo "error: SOURCE_DATE_EPOCH must be an integer" >&2; exit 2;; esac

SF2_LICENSE=${BASIC_SF2_LICENSE_FILE:-$ROOT/licenses/basic.sf2-LICENSE.txt}
if [ ! -s "$SF2_LICENSE" ]; then
  echo "error: basic.sf2 distribution requires a reviewed license in BASIC_SF2_LICENSE_FILE" >&2
  exit 1
fi

if [ "${FWP_SKIP_BUILD:-0}" != 1 ]; then
  cargo build --manifest-path "$CRATE/Cargo.toml" --release --locked --target "$TARGET"
fi
BINARY="$CRATE/target/$TARGET/release/freewheeling-plus"
[ -x "$BINARY" ] || { echo "error: release binary not found: $BINARY" >&2; exit 1; }

rm -rf "$STAGE" "$ARCHIVE"
mkdir -p "$STAGE/bin" "$STAGE/share/freewheeling/data" "$STAGE/share/doc/freewheeling/licenses"
install -m 0755 "$BINARY" "$STAGE/bin/freewheeling-plus"
cp -R "$ROOT/data/." "$STAGE/share/freewheeling/data/"
install -m 0644 "$ROOT/COPYING" "$ROOT/AUTHORS" "$ROOT/LINUX_PACKAGING.md" "$STAGE/share/doc/freewheeling/"
install -m 0644 "$SF2_LICENSE" "$STAGE/share/doc/freewheeling/licenses/basic.sf2-LICENSE.txt"

# Normalize metadata and ordering so identical inputs produce identical bytes.
find "$STAGE" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
LC_ALL=C tar --sort=name --format=ustar --owner=0 --group=0 --numeric-owner \
  --mtime="@$SOURCE_DATE_EPOCH" -C "$OUT" -cf - "$(basename "$STAGE")" | gzip -n -9 >"$ARCHIVE"
sha256sum "$ARCHIVE" >"$ARCHIVE.sha256"
printf '%s\n' "$ARCHIVE"
