#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/../../.." && pwd)
CRATE="$ROOT/freewheeling-plus"

for script in "$CRATE"/scripts/linux/*.sh; do sh -n "$script"; done
test -x "$CRATE/scripts/linux/run-virtual-workflow.sh"
grep -q 'jackd --no-realtime -d dummy' "$CRATE/scripts/linux/run-virtual-acceptance.sh"
grep -q -- '--sort=name' "$CRATE/scripts/linux/package-release.sh"
grep -qF 'Command::new("amixer")' "$CRATE/src/amixer.rs"
echo "Linux scripts and direct-ALSA guard validated"
