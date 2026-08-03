# FreeWheeling+

A Rust port of [FreeWheeling](https://github.com/free-wheeling/freewheeling) — a live-looper and audio performance instrument.

This is an in-progress migration from the original C++ codebase. The architecture uses a generic `Core<T: CoreServices>` at the center with native adapters for audio (CPAL / CoreAudio / JACK), MIDI, video (SDL2), and DSP.

## Status

Daily-driver quality on macOS. Linux, Windows, and Android are built and
verified in CI on every push and pull request:

- **macOS** — `cargo test` and `clippy -D warnings`; release packaging builds
a universal (arm64 + x86_64) app bundle and DMG with relocatable dependencies.
- **Linux** — `cargo check`/`test`/`clippy -D warnings` (with and without the
JACK feature), reproducible binary-plus-data archive packaging, and a virtual
JACK acceptance run (dummy driver: ports, transport, allocation/xrun/RSS
attestation).
- **Windows** — `cargo check`/`test`/`clippy -D warnings` and a zipped release
archive; a mingw cross-build script is included for local verification.
- **Android** — a signed release APK (`cargo apk`) is built and verified in CI
and attached to GitHub releases. Touch input mapping for the mouse-driven UI
is still in progress and needs on-device testing.

### Android

Requires the Android SDK, NDK, and `cargo-apk`. The script locates the SDK
via `ANDROID_HOME`/`ANDROID_SDK_ROOT` or the conventional per-platform install
paths, and picks the NDK by `ANDROID_NDK_VERSION` (default 28.2.13676358):

```sh
# 1. Install Android SDK + NDK (via Android Studio)
#    SDK: Preferences → Appearance & Behavior → System Settings → Android SDK
#    NDK: SDK Tools tab → check "NDK (Side by side)" → Apply

export ANDROID_HOME=/Users/jakubkolcar/Library/Android/sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/28.2.13676358
# 3. Install cargo subcommands and Rust target
cargo install cargo-apk
rustup target add aarch64-linux-android

# 4. Build the ARM64 APK
./android-build.sh

# Select another installed NDK if needed:
ANDROID_NDK_VERSION=30.0.15729638 ./android-build.sh
```

On first build, `cargo-apk` downloads remaining SDK components automatically.
The `sdl2` crate's `bundled` feature compiles SDL from source for Android.
For local builds, the script signs release APKs with `$HOME/.android/debug.keystore`
unless a release keystore is configured explicitly.

## Build

```sh
cargo build --release
cargo test
```

JACK support is optional. The default build uses the platform-native backend
and does not require or link against JACK. Enable it at build time with:

```sh
cargo build --release --features jack
```

To build and run with JACK:

```sh
# macOS only
brew install jack

FWEELIN_AUDIO_BACKEND=jack cargo run --release --features jack
```

On Linux, install your distribution's JACK development package before building.
Selecting `FWEELIN_AUDIO_BACKEND=jack` in a build without the feature exits with
an error explaining that the application must be rebuilt with `--features jack`.

## Audio Backend

The audio backend is selected at startup via the `FWEELIN_AUDIO_BACKEND` environment variable:

| Value | macOS | Linux | Windows | Android |
|-------|-------|-------|---------|---------|
| *unset* | CoreAudio AudioUnit | CPAL (ALSA) | CPAL (WASAPI) | CPAL (OpenSL ES) |
| `jack` | JACK (`brew install jack`) | JACK | JACK ([jackaudio.org](https://jackaudio.org)) | — |
| `cpal` | CPAL (explicit override) | CPAL | CPAL | CPAL |

**JACK** provides external transport sync (bar/beat/bpm from a DAW) and integrated MIDI ports.

**CPAL** requires no audio server — it uses the platform's default audio API
(CoreAudio on macOS, ALSA on Linux). Transport state is synthesized from the
internal pulse clock.

On macOS, the default CoreAudio backend needs a single duplex device for its
one-callback capture/playback path. When the system input and output devices
have different CoreAudio device IDs (the ordinary MacBook mic/speaker route),
FreeWheeling programmatically creates a **private Aggregate Device** whose
master clock is the output device and whose input subdevice runs with drift
compensation enabled. The HAL AudioUnit is opened against that aggregate, so
capture and playback share one clock domain and every captured frame reaches
the DSP exactly once. The aggregate is private to the process and is removed
when FreeWheeling exits or recovers; it never appears in the user's audio-device
list. If CoreAudio cannot create, verify, or activate the aggregate, startup
fails with a clear diagnostic instead of falling back to the known-corrupt
split-clock path. `FWEELIN_AUDIO_BACKEND=cpal` remains available as an explicit
override; that path uses separate capture and playback streams on independent
clocks and is labeled *split-clock fallback* in diagnostics.

```sh
cargo run --release -- --smoke-test
```

## Startup audio latency calibration

After audio starts (and only once the duplex callback is producing real input
and output frames), FreeWheeling plays five short swept chirps through the
speakers and listens for their return through the microphone. It measures the
round-trip delay of the aggregate-device route in audio frames at the
negotiated sample rate, rejects missed or inconsistent detections, and uses
the resulting consensus offset to align synchronized recordings and overdubs.
Keep the speakers audible and the microphone unobstructed during startup; if
the return signal is too quiet, the app keeps its provisional driver-based
alignment instead. After a route or device change the old alignment is
replaced and the round trip is measured again for the new route.

## Essential live-session controls

These are the default keyboard and mouse controls for a basic session.

| Control | Action |
|---|---|
| Click a loop, or press its `a`–`z` PC-keyboard key | On an empty slot: record; on a recording loop: stop and play; on a playing loop: toggle it off/on. |
| `1`–`4` | Enable or disable the corresponding audio input for recording. |
| `Shift` + `` ` `` then click a loop | Toggle overdub mode, then overdub the clicked loop. Press `Shift` + `` ` `` again to leave overdub mode. |
| `t` | Trigger or stop all selected loops. |
| `Shift` + click, or right-click a loop | Select or deselect that loop. |
| Hold `Space` + click a loop | Erase that loop. |
| `u` | Erase the last recorded loop (undo). |
| `F8` / `F9` | Save the last recorded loop / toggle automatic loop saving. |
| `F7` / `Shift` + `F7` | Save the current session / always save as a new session. |
| `b` | Cycle the patch, loop, and session browsers. |
| `-` / `=` | Previous / next browser item; the browser overlay closes after five seconds of inactivity. |
| `Enter` | Load the selected session (or import the selected loop); also closes the browser overlay. |
| `F4` or `Print Screen` | Start or stop recording the complete output stream to disk. |

`F4` is the default laptop-keyboard shortcut; `Print Screen` is its full-keyboard equivalent.

## Stream recordings

Disk-stream recordings are saved as `stream-<number>.<format>` in
`Documents/freewheeling-recordings`. The directory is created on startup.

## License

GPL-2.0 — same as the original FreeWheeling.
