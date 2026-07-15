#!/bin/sh
set -eu

if [ "$(uname -s)" != Darwin ]; then
  echo "error: this manual diagnostic runner requires macOS" >&2
  exit 1
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
REPO=$(CDPATH= cd -- "$ROOT/.." && pwd)
APP=${FWEELIN_DIAGNOSTIC_APP:-"$ROOT/target/release/freewheeling-plus"}
DATA=${FWEELIN_DIAGNOSTIC_DATADIR:-"$REPO/data"}
DURATION=${FWEELIN_DIAGNOSTIC_SECONDS:-30}
OUTPUT=${FWEELIN_DIAGNOSTIC_OUTPUT:-"${TMPDIR:-/tmp}/freewheeling-diagnostic-$(date +%Y%m%d-%H%M%S)"}

case "$DURATION" in
  ''|*[!0-9]*) echo "error: FWEELIN_DIAGNOSTIC_SECONDS must be an integer" >&2; exit 1 ;;
esac
[ "$DURATION" -gt 0 ] || { echo "error: diagnostic duration must be positive" >&2; exit 1; }
[ -x "$APP" ] || { echo "error: source-tree binary is missing or not executable: $APP" >&2; exit 1; }
[ -f "$DATA/fweelin.xml" ] || { echo "error: data directory lacks fweelin.xml: $DATA" >&2; exit 1; }
case "$OUTPUT/" in
  "$ROOT/acceptance-evidence/"*) echo "error: diagnostic output must not be acceptance evidence" >&2; exit 1 ;;
esac

mkdir -p "$OUTPUT"
echo "Diagnostic output directory: $OUTPUT"
APP_LOG="$OUTPUT/application.log"
COREAUDIO_LOG="$OUTPUT/coreaudio-unified-log.txt"
SDL_LOG="$OUTPUT/sdl-input-events.txt"
CALLBACK_LOG="$OUTPUT/audio-callback-diagnostics.txt"
RUST_REJECTION_LOG="$OUTPUT/rust-stderr-rejections.txt"
CRASH_REPORTS="$OUTPUT/crash-reports"
SUMMARY="$OUTPUT/README.txt"
CRASH_MARKER="$OUTPUT/.crash-report-marker"

cleanup() {
  [ -n "${APP_PID:-}" ] && kill -TERM "$APP_PID" 2>/dev/null || true
  [ -n "${LOG_PID:-}" ] && kill -TERM "$LOG_PID" 2>/dev/null || true
}
trap cleanup EXIT HUP INT TERM

{
  echo "FreeWheeling manual macOS diagnostic"
  echo "This is an operator log, not acceptance evidence or an attestation."
  echo "started=$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  echo "host=$(scutil --get ComputerName 2>/dev/null || true)"
  echo "os=$(sw_vers -productVersion)"
  echo "arch=$(uname -m)"
  echo "binary=$APP"
  echo "data_dir=$DATA"
  echo "duration_seconds=$DURATION"
  echo
  echo "Files: application.log, coreaudio-devices.txt, coreaudio-unified-log.txt, audio-callback-diagnostics.txt, rust-stderr-rejections.txt, sdl-input-events.txt, crash-reports/"
  echo "Perform the input test during the dwell; press Ctrl-C to stop early."
} >"$SUMMARY"

{
  echo "=== CoreAudio device inventory ==="
  system_profiler SPAudioDataType -detailLevel full 2>&1 || true
  echo
  echo "=== Audio MIDI system snapshot ==="
  system_profiler SPUSBDataType 2>&1 || true
} >"$OUTPUT/coreaudio-devices.txt"

# Unified logging is best-effort: older systems or restricted terminals may
# not expose matching records. The application log remains the primary source.
if command -v log >/dev/null 2>&1; then
  log stream --style compact --level debug --timeout "$DURATION" \
    --predicate '(eventMessage CONTAINS[c] "CoreAudio" OR eventMessage CONTAINS[c] "CPAL" OR eventMessage CONTAINS[c] "callback" OR eventMessage CONTAINS[c] "audio" OR eventMessage CONTAINS[c] "SDL")' \
    >"$COREAUDIO_LOG" 2>&1 &
  LOG_PID=$!
else
  echo "unavailable: macOS log command not found" >"$COREAUDIO_LOG"
fi

# Mark the start of the input test. Crash reports are collected only after
# the process exits, and only reports created or modified during this run are
# copied. This is evidence for investigation, never a success/failure signal.
touch "$CRASH_MARKER"

echo "Launching $APP for ${DURATION}s; interact with its window and audio devices now."
FWEELIN_DATADIR="$DATA" FWEELIN_DIAGNOSTICS=1 \
  "$APP" >"$APP_LOG" 2>&1 &
APP_PID=$!
sleep "$DURATION"
kill -TERM "$APP_PID" 2>/dev/null || true
wait "$APP_PID" 2>/dev/null || true
APP_PID=
wait "${LOG_PID:-}" 2>/dev/null || true
LOG_PID=

if grep -Eiq 'SDL|input|keyboard|mouse|joystick|event' "$APP_LOG"; then
  grep -Ei 'SDL|input|keyboard|mouse|joystick|event' "$APP_LOG" >"$SDL_LOG" || true
else
  echo "unavailable: no SDL/input/event diagnostics were printed by the source-tree binary" >"$SDL_LOG"
fi
if grep -Eiq 'CoreAudio|CPAL|audio|callback|xrun|underrun|overrun' "$APP_LOG"; then
  grep -Ei 'CoreAudio|CPAL|audio|callback|xrun|underrun|overrun' "$APP_LOG" >"$CALLBACK_LOG" || true
else
  echo "unavailable: no audio/callback diagnostics were printed by the source-tree binary" >"$CALLBACK_LOG"
fi
if grep -Eiq 'reject|rejected|denied|invalid|unsupported|failed' "$APP_LOG"; then
  grep -Ei 'reject|rejected|denied|invalid|unsupported|failed' "$APP_LOG" >"$RUST_REJECTION_LOG" || true
else
  echo "unavailable: no Rust stderr rejection lines were printed during the input test" >"$RUST_REJECTION_LOG"
fi

mkdir -p "$CRASH_REPORTS"
crash_count=0
for crash_dir in "$HOME/Library/Logs/DiagnosticReports" /Library/Logs/DiagnosticReports; do
  if [ -d "$crash_dir" ]; then
    while IFS= read -r report; do
      cp "$report" "$CRASH_REPORTS/" 2>/dev/null || true
      crash_count=$((crash_count + 1))
    done <<EOF
$(find "$crash_dir" -type f -newer "$CRASH_MARKER" -name '*.crash' -print 2>/dev/null || true)
EOF
  fi
done
if [ "$crash_count" -eq 0 ]; then
  echo "unavailable: no macOS crash report was created or modified during the input test" >"$CRASH_REPORTS/README.txt"
fi
rm -f "$CRASH_MARKER"

cat >>"$SUMMARY" <<EOF
finished=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
application_exit_observed=yes
coreaudio_unified_log=$(if [ -s "$COREAUDIO_LOG" ]; then echo captured-or-error-text; else echo unavailable; fi)
sdl_input_diagnostics=$(if grep -q '^unavailable:' "$SDL_LOG"; then echo unavailable; else echo lines-extracted-from-application-log; fi)
audio_callback_diagnostics=$(if grep -q '^unavailable:' "$CALLBACK_LOG"; then echo unavailable; else echo lines-extracted-from-application-log; fi)
rust_stderr_rejections=$(if grep -q '^unavailable:' "$RUST_REJECTION_LOG"; then echo unavailable; else echo lines-extracted-from-application-log; fi)
crash_reports=$(if [ "$crash_count" -gt 0 ]; then echo captured; else echo unavailable; fi)

Review the raw logs; this runner makes no pass/fail or acceptance claim.
EOF

echo "Diagnostic log written to $OUTPUT"
