#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/../../.." && pwd)
CRATE="$ROOT/freewheeling-plus"
USER_ID=${UID:-$(id -u)}
RUNTIME=${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}/freewheeling-jack-$USER_ID}
RESULT=${FWP_PERFORMANCE_RESULT:-$RUNTIME/performance.json}
JACK_PID=
APP_PID=

cleanup() {
  [ -z "$APP_PID" ] || kill "$APP_PID" 2>/dev/null || true
  [ -z "$JACK_PID" ] || kill "$JACK_PID" 2>/dev/null || true
  [ -z "$APP_PID" ] || wait "$APP_PID" 2>/dev/null || true
  [ -z "$JACK_PID" ] || wait "$JACK_PID" 2>/dev/null || true
}
trap cleanup EXIT HUP INT TERM

for command in cargo jackd jack_lsp jack_transport; do
  command -v "$command" >/dev/null || { echo "error: missing command: $command" >&2; exit 127; }
done
mkdir -p "$RUNTIME"
chmod 700 "$RUNTIME"
export XDG_RUNTIME_DIR="$RUNTIME"
export JACK_NO_AUDIO_RESERVATION=1

# Dummy JACK is timing-accurate enough for protocol acceptance and needs no devices.
jackd --no-realtime -d dummy -r 48000 -p 256 >"$RUNTIME/jackd.log" 2>&1 &
JACK_PID=$!
i=0
until jack_lsp >/dev/null 2>&1; do
  i=$((i + 1)); [ "$i" -lt 100 ] || { cat "$RUNTIME/jackd.log" >&2; exit 1; }
  sleep 0.05
done

FWP_PERFORMANCE_RESULT="$RESULT" FWP_REALTIME_ACCEPTANCE_SECONDS=${FWP_REALTIME_ACCEPTANCE_SECONDS:-3} \
  cargo run --quiet --manifest-path "$CRATE/Cargo.toml" --locked --bin realtime_acceptance &
APP_PID=$!
i=0
until jack_lsp | grep -q '^freewheeling-realtime-acceptance:'; do
  kill -0 "$APP_PID" 2>/dev/null || { wait "$APP_PID"; exit 1; }
  i=$((i + 1)); [ "$i" -lt 200 ] || { echo "error: FreeWheeling JACK ports did not appear" >&2; exit 1; }
  sleep 0.05
done

PORTS=$(jack_lsp | grep '^freewheeling-realtime-acceptance:' || true)
printf '%s\n' "$PORTS" | grep -q ':audio_in_l$'
printf '%s\n' "$PORTS" | grep -q ':audio_in_r$'
printf '%s\n' "$PORTS" | grep -q ':audio_out_l$'
printf '%s\n' "$PORTS" | grep -q ':audio_out_r$'
printf '%s\n' "$PORTS" | grep -q ':midi_in_0$'
printf '%s\n' "$PORTS" | grep -q ':midi_out_0$'

# Exercise transport state and relocation while the client is processing.
printf 'locate 48000\nplay\nquit\n' | jack_transport >/dev/null
sleep 0.2
# `jack_transport` reports command failures through its exit status; its
# prompt does not print query state. Keeping the client rolling for a bounded
# interval exercises JACK's shared transport state before the explicit stop.
printf 'stop\nquit\n' | jack_transport >/dev/null

wait "$APP_PID"
APP_PID=
python3 "$CRATE/scripts/validate_performance_result.py" "$RESULT"
printf 'virtual JACK acceptance passed; no physical hardware was used\n'
