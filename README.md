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

# 2. Set environment variables:
export ANDROID_HOME=$HOME/Library/Android/sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/27.0.12077973

# 3. Install cargo subcommands and Rust target
cargo install cargo-apk
rustup target add aarch64-linux-android

# 4. Build APK (install + run on connected device/emulator)
cargo apk build --release
# or
cargo apk run --release
```

On first build, `cargo-apk` downloads remaining SDK components automatically.
The `sdl2` crate's `bundled` feature compiles SDL from source for Android.

## Build

```sh
cargo build --release
cargo test
```


## Audio Backend

The audio backend is selected at startup via the `FWEELIN_AUDIO_BACKEND` environment variable:

| Value | macOS | Linux | Windows | Android |
|-------|-------|-------|---------|---------|
| *unset* | CoreAudio AudioUnit | CPAL (ALSA) | CPAL (WASAPI) | CPAL (OpenSL ES) |
| `jack` | JACK (`brew install jack`) | JACK | JACK ([jackaudio.org](https://jackaudio.org)) | — |
| `cpal` | CPAL (explicit override) | CPAL | CPAL | CPAL |

**JACK** provides external transport sync (bar/beat/bpm from a DAW) and integrated MIDI ports. On macOS, install JACK via Homebrew: `brew install jack`.

**CPAL** requires no audio server — it uses the platform's default audio API (CoreAudio on macOS, ALSA on Linux). Transport state is synthesized from the internal pulse clock.

```sh
cargo run --release -- --smoke-test
```

## License

GPL-2.0 — same as the original FreeWheeling.
