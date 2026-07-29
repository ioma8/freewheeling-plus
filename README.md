# FreeWheeling+

A Rust port of [FreeWheeling](https://github.com/free-wheeling/freewheeling) — a live-looper and audio performance instrument.

This is an in-progress migration from the original C++ codebase. The architecture uses a generic `Core<T: CoreServices>` at the center with native adapters for audio (CPAL / CoreAudio / JACK), MIDI, video (SDL2), and DSP.

## Status

Daily-driver quality on macOS. Linux and Windows should work but get less testing.
Android: entry point, path handling, and audio tuning implemented; needs
`cargo-apk` + Android NDK toolchain and touch input mapping.


### Android

Requires the Android SDK, NDK, and `cargo-apk`:

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

**CPAL** requires no audio server — it uses the platform's default audio API (CoreAudio on macOS, ALSA on Linux). Transport state is synthesized from the internal pulse clock.

```sh
cargo run --release -- --smoke-test
```

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
