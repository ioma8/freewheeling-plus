# FreeWheeling+

A Rust port of [FreeWheeling](https://github.com/free-wheeling/freewheeling) — a live-looper and audio performance instrument.

This is an in-progress migration from the original C++ codebase. The architecture uses a generic `Core<T: CoreServices>` at the center with native adapters for audio (CPAL / CoreAudio / JACK), MIDI, video (SDL2), and DSP.

## Status

Daily-driver quality on macOS. Linux should work but gets less testing. Windows untested.

The port currently preserves the full feature set of the original: multi-track loop recording, pulse-synced overdubbing, scene management, snapshot recall, FluidSynth-backed patch browser, OSC control, and the complete FreeWheeling XML configuration system.

## Build

```sh
cargo build --release
cargo test
```

The binary is `freewheeling-plus`. Run `--smoke-test` to verify the core lifecycle without audio or video hardware:

```sh
cargo run --release -- --smoke-test
```

## License

GPL-2.0 — same as the original FreeWheeling.
