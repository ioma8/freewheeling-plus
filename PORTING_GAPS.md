# C++ Porting Gaps and Improvement Opportunities

Audit date: 2026-07-16

Compared the Rust codebase against the C++ source at:

`/Users/jakubkolcar/projects/customs/freewheeling`

All 237 Rust library tests passed during this audit. However, the tests use the Rust-side `../data`; its `interfaces.xml` has several optional C++ interfaces disabled, so the full original configuration is not exercised.

## Highest-priority gaps

### 1. Runtime output-event dispatch is incomplete

Rust parses many event types but does not execute them at runtime. Important missing cases include:

- `GoSub`
- `ALSAMixerControlSet`
- `VideoShowDisplay`
- `VideoShowHelp`
- `VideoShowSnapshotPage`
- `VideoShowParamSetBank` and `VideoShowParamSetPage`
- `SlideInVolume`
- `SetInVolume`
- `ToggleInputRecord`
- `RenameLoop`

The event types exist in [`src/event.rs`](src/event.rs), but the runtime mapping in [`src/runtime_event_actions.rs`](src/runtime_event_actions.rs) does not cover them.

`GoSub` is especially important because it is widely used by the stock core and controller XML files. The parser creates these events, but they are not recursively dispatched, so many XML bindings silently do nothing.

### 2. External audio inputs are parsed but not wired into production audio

Rust parses settings such as `externalaudioinputs`, `audioinputmonitoring`, `streaminputs`, `streamfinalmix`, and `streamloopmix`, but production audio is hardcoded to two input channels in [`src/audioio.rs`](src/audioio.rs) and [`src/native_runtime.rs`](src/native_runtime.rs).

The production DSP only consumes `inputs[0]` and `inputs[1]` in [`src/native_dsp_graph.rs`](src/native_dsp_graph.rs). The original C++ implementation supports configurable mono/stereo inputs, per-input monitoring, volume, recording selection, and streaming.

### 3. The production audio backend is not equivalent to C++ JACK

The C++ version is JACK-based and provides JACK transport, timebase synchronization, and dynamic JACK ports.

The production Rust runtime uses CPAL/CoreAudio. There is a Linux JACK implementation in [`src/linux_native.rs`](src/linux_native.rs), including transport-related functionality, but it is not connected to `NativeRuntime`.

This is a major Linux parity gap. It may be acceptable if the Rust version intentionally targets CPAL/CoreAudio on macOS, but it should be documented as a platform deviation.

### 4. Disk recording supports only one final stereo stream

Rust currently has one stream encoder connected to the final callback output. The parsed stream-selection options are not used.

The C++ implementation can record the final mix, loop mix, and multiple individual inputs. It also uses dedicated encoder threads and timing marker files. Rust currently performs encoding from the main event loop in [`src/native_runtime.rs`](src/native_runtime.rs), which can stall event processing under disk or encoder load.

The loop library recognizes `.wav.usx` timing files, but the streaming path does not currently write them.

The OGG format should remain unchanged; this is a recording-topology and threading gap, not a request to change codecs.

## UI and XML compatibility gaps

### 5. Production XML display support is incomplete

The production XML parser in [`src/native_ui_scene.rs`](src/native_ui_scene.rs) does not support `paramset`, although the C++ `synthedit.xml` uses `type="paramset"`.

Other compatibility issues include:

- C++ `<squares>` uses `firstsquareval`, `lastsquareval`, and `squareinterval`; Rust expects different attribute names.
- Production squares are always rendered horizontally even though orientation is parsed.
- `switchvar`, `calibrate`, `color`, and `marks` are not fully implemented.
- Browser `xdelay` is ignored.
- Dynamic display, help-page, snapshot-page, and parameter-set events are not fully connected to the runtime UI.

The generic widget implementation in [`src/videoio_display_widgets.rs`](src/videoio_display_widgets.rs) is more complete than the production XML wiring.

### 6. Live system variables are only partially reflected in the UI

The C++ code publishes many live variables, including MIDI output count, transpose, pitch bend, sync state, synth state, help page, loop counts, patch-bank data, snapshot page, and detailed input state.

Rust publishes only a subset in [`src/native_runtime.rs`](src/native_runtime.rs). Some variables are declared during startup but never updated live. For example, input-record indicators are currently hardcoded instead of reflecting actual input selections.

## Smaller behavior differences

### 7. MIDI transport and echo behavior differ

Rust starts MIDI transport with Start but does not send the C++ equivalent Song Position Pointer before Start.

Incoming MIDI echo covers fewer message families than C++. Rust handles common note/controller/program messages, but not all pressure, system-common, sysex, and realtime messages that the C++ implementation passes through.

Relevant Rust code is in [`src/midiio.rs`](src/midiio.rs) and [`src/native_runtime.rs`](src/native_runtime.rs).

### 8. Some parsed settings are silently ignored

- FluidSynth interpolation is parsed but not applied to the FluidLite backend.
- OGG/Vorbis quality is parsed, but the encoder uses the crate default rather than the configured quality.
- Linux hardware mixer support exists in isolation, but is not connected to the production runtime. This is optional/platform-specific because the C++ implementation is ALSA-specific.

Relevant files are [`src/config.rs`](src/config.rs), [`src/fluidsynth.rs`](src/fluidsynth.rs), [`src/file_codecs.rs`](src/file_codecs.rs), and [`src/native_runtime.rs`](src/native_runtime.rs).

## Recommended implementation order

1. Complete runtime output dispatch, especially `GoSub`.
2. Wire dynamic UI events and missing live system variables.
3. Fix production XML compatibility, especially `paramset` and `<squares>` attributes.
4. Decide whether Linux parity requires JACK as the production backend.
5. Implement configurable external inputs and multi-stream recording.
6. Fix the MIDI, FluidSynth, Vorbis-quality, and ALSA mixer differences.

## Areas that appear substantially ported

The following areas looked reasonably complete in the inspected code:

- Core loop/DSP processing
- Overdub and pulse handling
- Basic MIDI clock behavior
- Persistence and loop-library handling
- MD5 replacement
- OGG recording itself
- OSC support
- Patch-browser core logic
- Runtime lifecycle and shutdown handling
- Recording-status display
