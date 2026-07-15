# Historical C++ golden fixtures

These artifacts are captured by executing historical C++ source, never by copying Rust output or transcribing expected values. Run `scripts/capture_cpp_golden.sh` from any directory to reproduce all headless captures supported by the host. The script records the exact C++ Git revision, compiler/host provenance, source paths, and SHA-256 hashes.

Available captures include original DSP and renderer vectors; WAV, FLAC, and Ogg output; configuration, loop, and scene XML; SDL framebuffer screenshots; original MIDI input decoding and output encoding for every implemented channel-message family plus clock/start/stop; and startup-guard rollback. Category-specific provenance and manifests record each capture boundary and exact hashes.

The MIDI/startup capture has no physical-device dependency:

```sh
CPP_MIDI_STARTUP_OUT=/tmp/cpp-golden scripts/capture_cpp_midi_startup.sh
```

`midi/PROVENANCE` records the Git revision, compiler, fake boundary, capture-script hash, and hashes of both historical C++ sources. `midi/MANIFEST.sha256` authenticates the MIDI and rollback payloads. The generated harness is intentionally temporary and never consumes Rust output.

The full historical application capture is reproducible on macOS with `scripts/capture_cpp_full_startup.sh`. It launches the packaged original C++ binary with isolated configuration, dummy SDL video/audio and software rendering, waits for complete startup, and requests clean shutdown. `startup/PROVENANCE` records the binary and script hashes and the exact fake-device boundary.

The root `MANIFEST.sha256` exhaustively names every file in the seven fixture classes (`codec`, `dsp`, `midi`, `persistence`, `renderer`, `screenshots`, and `startup`). No Rust-produced codec or image output may be placed here.

To capture into a staging directory without altering approved fixtures:

```sh
CPP_GOLDEN_OUT=/tmp/cpp-golden scripts/capture_cpp_golden.sh
```

Review the staged provenance and hashes before promoting it. Capture timestamps are deliberately excluded because the generated headless payloads and manifest must be byte-reproducible.
