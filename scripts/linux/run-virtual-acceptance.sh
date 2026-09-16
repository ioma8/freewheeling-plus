#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/../../.." && pwd)
CRATE="$ROOT/freewheeling-plus"
USER_ID=${UID:-$(id -u)}
# A private, unpredictable runtime directory: jackd is pointed at it through
# XDG_RUNTIME_DIR, and a guessable name under /tmp could otherwise be
# pre-created as a symlink (mkdir -p + chmod would follow it). `cleanup`
# removes it again.
RUNTIME=$(mktemp -d "${TMPDIR:-/tmp}/freewheeling-jack-XXXXXXXX")
# The result lives outside the private directory so `cleanup` can remove the
# runtime tree without deleting the artifact it just validated.
RESULT=${FWP_PERFORMANCE_RESULT:-${TMPDIR:-/tmp}/freewheeling-performance-$USER_ID.json}
JACK_PID=
APP_PID=

cleanup() {
  [ -z "$APP_PID" ] || kill "$APP_PID" 2>/dev/null || true
  [ -z "$JACK_PID" ] || kill "$JACK_PID" 2>/dev/null || true
  [ -z "$APP_PID" ] || wait "$APP_PID" 2>/dev/null || true
  [ -z "$JACK_PID" ] || wait "$JACK_PID" 2>/dev/null || true
  rm -rf "$RUNTIME"
}
# `cleanup` is the EXIT trap only. A signal handler that merely cleaned up would
# resume the script after the trap returned, so the remaining steps would run
# against a torn-down runtime; each one exits explicitly instead.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

for command in cargo jackd jack_lsp jack_transport python3; do
  command -v "$command" >/dev/null || { echo "error: missing command: $command" >&2; exit 127; }
done
VALIDATOR="$CRATE/scripts/validate_performance_result.py"
[ -f "$VALIDATOR" ] || { echo "error: missing validator: $VALIDATOR" >&2; exit 1; }
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

# Build first: the port-appearance budget below must cover start-up only, or a
# cold compile would time out with the misleading "ports did not appear"
# message, and a build failure must not look like a runtime failure.
cargo build --manifest-path "$CRATE/Cargo.toml" --locked --features jack --bin realtime_acceptance
ACCEPTANCE="$CRATE/target/debug/realtime_acceptance"
[ -x "$ACCEPTANCE" ] || { echo "error: missing acceptance binary: $ACCEPTANCE" >&2; exit 1; }
# Run the binary directly rather than through `cargo run`: the wrapper's pid is
# not the acceptance client's, so killing it would leave the client (and its
# JACK ports) alive and `wait` could block on a stuck child.
FWP_PERFORMANCE_RESULT="$RESULT" FWP_REALTIME_ACCEPTANCE_SECONDS=${FWP_REALTIME_ACCEPTANCE_SECONDS:-3} \
  "$ACCEPTANCE" &
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
# Wait for the rolling state instead of sleeping a fixed amount: `jack_showtime`
# prints the transport state (`jack-example-tools`), and where it is not
# installed the window is bounded by the client's own liveness, so a client that
# died during the transport change fails here instead of silently passing.
if command -v jack_showtime >/dev/null 2>&1; then
  i=0
  until jack_showtime 2>/dev/null | grep -q 'Rolling'; do
    i=$((i + 1)); [ "$i" -lt 100 ] || { echo "error: JACK transport did not start rolling" >&2; exit 1; }
    sleep 0.05
  done
else
  i=0
  until [ "$i" -ge 20 ]; do
    kill -0 "$APP_PID" 2>/dev/null || {
      echo "error: acceptance client exited during the transport change" >&2
      exit 1
    }
    i=$((i + 1)); sleep 0.05
  done
fi
printf 'stop\nquit\n' | jack_transport >/dev/null

# `set -e` would abort before the logs are surfaced, so the status is captured
# explicitly: a crash must say what happened instead of failing silently.
if ! wait "$APP_PID"; then
  echo "error: realtime_acceptance exited abnormally" >&2
  tail -n 100 "$RUNTIME/jackd.log" >&2 || true
  # `cleanup` removes the runtime tree, so keep the server log next to the
  # result instead of losing the only diagnostics with it.
  cp "$RUNTIME/jackd.log" "$RESULT.jackd.log" 2>/dev/null || true
  exit 1
fi
APP_PID=
[ -f "$RESULT" ] || { echo "error: missing performance result: $RESULT" >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then
  hash_file() { sha256sum "$1" | awk '{print $1}'; }
else
  hash_file() { shasum -a 256 "$1" | awk '{print $1}'; }
fi
python3 "$VALIDATOR" "$RESULT"
# Record a positive validation signal next to the result so a downstream
# attestation can prove the validator ran (and on which artifact) instead of
# assuming it.
VALIDATION_STAMP="$RESULT.validation"
{
  printf 'validator=%s\n' "$VALIDATOR"
  printf 'validator_sha256=%s\n' "$(hash_file "$VALIDATOR")"
  printf 'validated_result_sha256=%s\n' "$(hash_file "$RESULT")"
  printf 'status=passed\n'
} >"$VALIDATION_STAMP"
printf 'virtual JACK acceptance passed; no physical hardware was used\n'
