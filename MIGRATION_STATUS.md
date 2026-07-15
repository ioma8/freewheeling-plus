# Rust migration manifest

Status at 2026-07-14: **implementation substantially migrated; full 1:1 port not yet accepted**.

All 30 top-level C/C++ implementation sources have Rust counterparts. The crate now includes concrete CPAL/CoreAudio, midir/CoreMIDI, SDL2, FluidLite, streaming codec, native DSP/event, macOS assembly, JACK, and direct ALSA implementations. Normal invocation constructs the production application; `--smoke-test` remains available.

Recent integration work connected the XML binding registry to the real SDL/MIDI event path, fixed conditions on chains beginning with `output1`, preserved typed `loopoutformat`/`streamoutformat`, continuously pumps video, and routes loop/synth/browser/fullscreen/stream/snapshot commands. Loop PCM import/export uses generation-checked preallocated transfer handles; export copies at most 4096 stereo frames per callback, and WAV/Vorbis/FLAC/AU encoding remains outside the callback. Native scene save/load now snapshots and serializes loops sequentially, backs up current scenes, restores loops through a bounded import queue, and restores snapshots. Automatic loop saving, MIDI sync/transport output, synth enable/disable, and OSC DAW transmission are wired.

Recent completion work also wires browser and snapshot rename, selected-loop
sets, patch-browser routing (including suppressed external program changes),
Unicode SDL text entry, non-retriggering trigger-gain changes, and live
renderer state from DSP/browser/config snapshots. The configuration snapshot
is bounded and read-only, and remains off the audio callback.

The current implementation is not honestly describable as a complete 1:1
release because the pixel, two-hour real-time, bundle-license, and hardware
workflow gates listed in `ACCEPTANCE_STATUS.md` remain open. Scene and
automatic-save workflows have deterministic component coverage but still need
the real-device end-to-end workflow acceptance listed below.

Run the full acceptance driver before making a completion claim:

```sh
../scripts/run_full_acceptance.sh
```

The latest automated run passes formatting, compilation, strict linting, 286
tests, diff checks, and C++ fixture completeness. Its remaining failures are
preserved in `target/full-acceptance.tsv`.
