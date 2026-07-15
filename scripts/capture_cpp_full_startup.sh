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
mkdir "$tmp/home"

HOME=$tmp/home \
LC_ALL=C \
SDL_VIDEODRIVER=dummy \
SDL_RENDER_DRIVER=software \
SDL_AUDIODRIVER=dummy \
"$app" >"$raw" 2>&1 &
pid=$!

# The historical binary uses block-buffered stdio when redirected, so readiness
# is validated after shutdown rather than by tailing output that is not flushed.
sleep 10
if ! kill -0 "$pid" 2>/dev/null; then
  wait "$pid" 2>/dev/null || true
  echo "historical application exited before the startup dwell completed" >&2
  tail -40 "$raw" >&2
  exit 1
fi
kill -TERM "$pid"
if ! wait "$pid"; then
  echo "historical application did not shut down successfully" >&2
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

mkdir -p "$out/startup"
sed \
  -e "s|$tmp/home|<HOME>|g" \
  -E -e 's/0x[[:xdigit:]]+/<addr>/g' \
  "$raw" >"$out/startup/full-application.log"

revision=$(git -C "$repo" rev-parse HEAD)
compiler=$(c++ --version | sed -n '1p')
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

(cd "$out/startup" && shasum -a 256 full-application.log PROVENANCE >MANIFEST.sha256)
(cd "$out" && find codec dsp midi persistence renderer screenshots startup -type f -print | LC_ALL=C sort | xargs shasum -a 256 >MANIFEST.sha256)
echo "captured genuine historical startup and clean shutdown: $out/startup/full-application.log"
