#!/bin/sh
# Static validation of the Linux packaging scripts.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/../../.." && pwd)
CRATE="$ROOT/freewheeling-plus"

fail() {
    echo "validate: $1" >&2
    exit 1
}

found=0
for script in "$CRATE"/scripts/linux/*.sh; do
    [ -f "$script" ] || continue
    found=$((found + 1))
    sh -n "$script" || fail "syntax error in $script"
done
[ "$found" -gt 0 ] || fail "no Linux packaging scripts found under $CRATE/scripts/linux"

for required in \
    "$CRATE/scripts/linux/run-virtual-workflow.sh" \
    "$CRATE/scripts/linux/run-virtual-acceptance.sh" \
    "$CRATE/scripts/linux/package-release.sh" \
    "$CRATE/src/amixer.rs"; do
    [ -r "$required" ] || fail "required file missing: $required"
done
test -x "$CRATE/scripts/linux/run-virtual-workflow.sh" \
    || fail "run-virtual-workflow.sh is not executable"

grep -q 'jackd --no-realtime -d dummy' "$CRATE/scripts/linux/run-virtual-acceptance.sh" \
    || fail "the virtual acceptance script no longer starts a dummy jackd"
grep -q -- '--sort=name' "$CRATE/scripts/linux/package-release.sh" \
    || fail "package-release.sh lost its reproducible tar ordering"
grep -qF 'Command::new("amixer")' "$CRATE/src/amixer.rs" \
    || fail "src/amixer.rs no longer invokes amixer directly (ALSA backend guard)"

echo "Linux scripts and direct-ALSA guard validated"
