#!/bin/sh
set -eu
umask 022

ROOT=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
CRATE="$ROOT"
# The archive name must describe the binary inside it, so the version comes
# from the crate manifest: `FWP_VERSION` may override it only with the same
# value, which is what catches a release tagged for a version the checkout
# never declared (the binary would self-report the manifest version).
MANIFEST_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$CRATE/Cargo.toml" | head -1)
[ -n "$MANIFEST_VERSION" ] || {
  echo "error: cannot read the version from $CRATE/Cargo.toml" >&2
  exit 2
}
VERSION=${FWP_VERSION:-$MANIFEST_VERSION}
[ "$VERSION" = "$MANIFEST_VERSION" ] || {
  echo "error: FWP_VERSION=$VERSION does not match Cargo.toml ($MANIFEST_VERSION); bump the manifest or drop FWP_VERSION" >&2
  exit 2
}
TARGET=${FWP_TARGET:-x86_64-unknown-linux-gnu}
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null)} || {
  echo "error: cannot determine SOURCE_DATE_EPOCH from the checkout; set it explicitly" >&2
  exit 2
}
OUT=${FWP_DIST_DIR:-$CRATE/dist}
STAGE="$OUT/freewheeling-plus-$VERSION-$TARGET"
ARCHIVE="$STAGE.tar.gz"

case "$VERSION" in *[!0-9A-Za-z._+-]*|'') echo "error: invalid FWP_VERSION" >&2; exit 2;; esac
case "$TARGET" in *[!0-9A-Za-z._-]*|'') echo "error: invalid FWP_TARGET" >&2; exit 2;; esac
case "$SOURCE_DATE_EPOCH" in *[!0-9]*|'') echo "error: SOURCE_DATE_EPOCH must be an integer" >&2; exit 2;; esac
# Probe the value before anything is packaged: `touch`/`tar` are the only
# consumers, and them rejecting the timestamp mid-packaging would leave a
# partially written archive behind.
probe=$(mktemp)
if ! touch -d "@$SOURCE_DATE_EPOCH" "$probe" 2>/dev/null; then
  rm -f "$probe"
  echo "error: SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH is out of range for touch/tar" >&2
  exit 2
fi
rm -f "$probe"

if [ "${FWP_SKIP_BUILD:-0}" != 1 ]; then
  cargo build --manifest-path "$CRATE/Cargo.toml" --release --locked --target "$TARGET"
fi
BINARY="$CRATE/target/$TARGET/release/freewheeling-plus"
[ -x "$BINARY" ] || { echo "error: release binary not found: $BINARY" >&2; exit 1; }

rm -rf "$STAGE" "$ARCHIVE" "$ARCHIVE.sha256"
mkdir -p "$STAGE/bin" "$STAGE/share/freewheeling/data" "$STAGE/share/doc/freewheeling/licenses"
install -m 0755 "$BINARY" "$STAGE/bin/freewheeling-plus"
cp -R "$ROOT/data/." "$STAGE/share/freewheeling/data/"
install -m 0644 "$ROOT/COPYING" "$ROOT/AUTHORS" "$ROOT/LINUX_PACKAGING.md" "$STAGE/share/doc/freewheeling/"
# The licenses directory is shipped, so it must not be empty: the GPL text and
# the bundled Bitstream Vera font license are the two terms a redistributor has
# to pass on (the macOS bundle ships the same pair under Resources/licenses).
install -m 0644 "$ROOT/COPYING" "$STAGE/share/doc/freewheeling/licenses/COPYING"
install -m 0644 "$ROOT/data/fonts/truetype/ttf-bitstream-vera/COPYING" \
  "$STAGE/share/doc/freewheeling/licenses/Bitstream-Vera-COPYING"

# Normalize metadata and ordering so identical inputs produce identical bytes.
# GNU-only flags are required for that (BSD tar has neither --sort nor
# --mtime); the check below reports the missing tool instead of producing a
# non-reproducible archive.
if ! tar --version 2>/dev/null | grep -q 'GNU tar'; then
  echo "error: GNU tar is required for a reproducible archive" >&2
  exit 1
fi
find "$STAGE" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} + 2>/dev/null || \
  find "$STAGE" -exec touch -d "@$SOURCE_DATE_EPOCH" {} +
# The archive is written to a temporary file first: in a pipeline only the last
# command's status is reported, so a failing `tar` would look like success and
# publish a truncated archive.
TMP_TAR="$OUT/.$(basename "$STAGE").tar"
rm -f "$TMP_TAR"
if ! LC_ALL=C tar --sort=name --format=ustar --owner=0 --group=0 --numeric-owner \
     --mtime="@$SOURCE_DATE_EPOCH" -C "$OUT" -cf "$TMP_TAR" "$(basename "$STAGE")"; then
  rm -f "$TMP_TAR"
  echo "error: tar failed" >&2
  exit 1
fi
gzip -c -n -9 "$TMP_TAR" >"$ARCHIVE"
rm -f "$TMP_TAR"
if ! command -v sha256sum >/dev/null 2>&1; then
  echo "error: sha256sum (GNU coreutils) is required" >&2
  exit 1
fi
(cd "$OUT" && sha256sum "$(basename "$ARCHIVE")") >"$ARCHIVE.sha256"
printf '%s\n' "$ARCHIVE"
