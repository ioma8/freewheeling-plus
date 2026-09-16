#!/bin/sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
out=${CPP_FULL_STARTUP_OUT:-"$repo/freewheeling-plus/fixtures/cpp-golden"}
app=${CPP_FULL_STARTUP_APP:-"$repo/MacOSX/build/Release/fweelin.app/Contents/MacOS/fweelin"}

if [ "$(uname -s)" != Darwin ]; then
  echo "full historical startup capture requires macOS" >&2
  exit 1
fi
if [ ! -x "$app" ]; then
  echo "historical application binary is missing: $app" >&2
  exit 1
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/fweelin-cpp-startup.XXXXXX")
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
raw=$tmp/full-application.raw
mkdir -p "$tmp/home" "$tmp/home/tmpdir"

HOME=$tmp/home \
TMPDIR=$tmp/home/tmpdir \
LC_ALL=C \
SDL_VIDEODRIVER=dummy \
SDL_RENDER_DRIVER=software \
SDL_AUDIODRIVER=dummy \
"$app" >"$raw" 2>&1 &
pid=$!

# The historical binary uses block-buffered stdio when redirected, so readiness
# is validated after shutdown rather than by tailing output that is not flushed.
dwell=${CPP_FULL_STARTUP_DWELL_SECONDS:-10}
elapsed=0
while [ "$elapsed" -lt "$dwell" ] && kill -0 "$pid" 2>/dev/null; do
  sleep 1
  elapsed=$((elapsed + 1))
done
if ! kill -0 "$pid" 2>/dev/null; then
  wait "$pid" 2>/dev/null || true
  echo "historical application exited before the startup dwell completed" >&2
  tail -40 "$raw" >&2
  exit 1
fi
kill -TERM "$pid"
status=0
grace=${CPP_FULL_STARTUP_SHUTDOWN_SECONDS:-10}
elapsed=0
while [ "$elapsed" -lt "$grace" ] && kill -0 "$pid" 2>/dev/null; do
  sleep 1
  elapsed=$((elapsed + 1))
done
if kill -0 "$pid" 2>/dev/null; then
  echo "historical application ignored SIGTERM; escalating to SIGKILL" >&2
  kill -KILL "$pid" 2>/dev/null || true
fi
wait "$pid" || status=$?
# 143 = SIGTERM, 130 = SIGINT: a signal-driven shutdown is the expected path.
if [ "$status" -ne 0 ] && [ "$status" -ne 143 ] && [ "$status" -ne 130 ]; then
  echo "historical application did not shut down successfully (status $status)" >&2
  tail -60 "$raw" >&2
  exit 1
fi

for marker in \
  'VIDEO: Creating temporary buffers' \
  'SDLIO: SDL Input thread start.' \
  'MIDI: begin close...' \
  'MIDI: end' \
  'AUDIO: end' \
  'EVENT: manager end.' \
  'MEM: End cleanup.' \
  'MAIN: end'
do
  grep -Fq "$marker" "$raw" || {
    echo "successful startup/shutdown marker missing: $marker" >&2
    exit 1
  }
done

# Provenance is collected and validated first: failing after the golden log was
# written would leave partial, misleading evidence in a committed fixtures tree.
if ! revision=$(git -C "$repo" rev-parse HEAD 2>/dev/null) || [ -z "$revision" ]; then
  echo "cannot determine the git revision for $repo" >&2
  exit 1
fi
compiler=$(c++ --version 2>/dev/null | sed -n '1p')
if [ -z "$compiler" ]; then
  echo "a C++ compiler is required for provenance" >&2
  exit 1
fi

mkdir -p "$out/startup"
# `$tmp` is interpolated into a sed program, so its regex metacharacters are
# escaped; the child also gets a deterministic TMPDIR (set above) so no
# user-specific path can leak into the committed log.
home_escaped=$(printf '%s' "$tmp/home" | sed 's/[][\.*^$/&|]/\\&/g')
sed \
  -e "s|$home_escaped|<HOME>|g" \
  -E -e 's/0x[[:xdigit:]]{6,}/<addr>/g' \
  "$raw" >"$out/startup/full-application.log"
binary_sha=$(shasum -a 256 "$app" | awk '{print $1}')
script_sha=$(shasum -a 256 "$0" | awk '{print $1}')
cat >"$out/startup/PROVENANCE" <<EOF
schema=freewheeling-cpp-full-startup-v1
cpp_revision=$revision
capture_script=scripts/capture_cpp_full_startup.sh
capture_script_sha256=$script_sha
application_binary=MacOSX/build/Release/fweelin.app/Contents/MacOS/fweelin
application_binary_sha256=$binary_sha
compiler=$compiler
host_os=$(sw_vers -productVersion)
host_arch=$(uname -m)
locale=C
video_driver=dummy
render_driver=software
audio_driver=dummy
midi_backend=CoreMIDI
shutdown_signal=TERM
normalization=temporary HOME and hexadecimal runtime addresses only
EOF

# Evidence directories must not carry files from an earlier run into the new
# manifest: only what this capture produced is hashed.
for directory in codec dsp midi persistence renderer screenshots startup; do
  if [ ! -d "$out/$directory" ]; then
    echo "missing evidence directory: $out/$directory" >&2
    exit 1
  fi
  find "$out/$directory" -mindepth 1 -delete
done
(cd "$out/startup" && shasum -a 256 full-application.log PROVENANCE >MANIFEST.sha256)
rm -f "$out/MANIFEST.sha256"
# `xargs` runs its command even with no input on some hosts (BSD), which would
# make `shasum` read stdin: collect the file list first and check it is empty.
evidence_files=$(cd "$out" && find codec dsp midi persistence renderer screenshots startup -type f -print0 \
  | LC_ALL=C sort -z | tr '\0' '\n' | grep -c . || true)
if [ "$evidence_files" -eq 0 ]; then
  echo "error: the capture produced no evidence files under $out" >&2
  exit 1
fi
(cd "$out" && find codec dsp midi persistence renderer screenshots startup -type f -print0 \
  | LC_ALL=C sort -z | xargs -0 shasum -a 256 >MANIFEST.sha256)
echo "captured genuine historical startup and clean shutdown: $out/startup/full-application.log"
