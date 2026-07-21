# FreeWheeling+

A Rust port of [FreeWheeling](https://github.com/free-wheeling/freewheeling) — a live-looper and audio performance instrument.

This is an in-progress migration from the original C++ codebase. The architecture uses a generic `Core<T: CoreServices>` at the center with native adapters for audio (CPAL / CoreAudio / JACK), MIDI, video (SDL2), and DSP.

## Status

Daily-driver quality on macOS. Linux and Windows should work but get less testing.
Android: entry point, path handling, and audio tuning implemented; needs build
toolchain (`cargo-ndk` + `cargo-apk`) and touch input mapping.

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
