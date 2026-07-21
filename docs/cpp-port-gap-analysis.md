# C++ to Rust Port: Gap Analysis

Compiled from systematic comparison of 40+ C++ source files against 65+ Rust modules.
Date: 2026-07-21
Branch: master at `1cfbdc8`

---

## Legend

| Status | Meaning |
|--------|---------|
| **OK** | Functionally equivalent in Rust |
| **PARTIAL** | Equivalent exists but with missing features or different architecture |
| **MISSING** | No equivalent at all in Rust |

---

## 1. Missing Features (MISSING)

These features exist in the C++ codebase but have no Rust equivalent at all.

### 1.1 FileStreamer — Disk Output Streaming

**C++ source:** `fweelin_core_dsp.h` / `fweelin_core_dsp.cc` — `FileStreamer` class

**What it does:** Records audio input directly to disk from the realtime audio callback. Used for DAW export, stem recording, and stream-to-disk. The C++ implementation:
- Spawns an encode thread connected via a lock-free ring buffer
- Encodes in the background thread (Vorbis, WAV, FLAC, AU) while the RT thread writes raw PCM
- Tracks time markers (cue points in the output stream)
- Supports writing timing metadata alongside audio
- Configurable output buffer size (default 100k frames)

**Rust equivalent:** None.

**How to fill it:**

```
src/file_streamer.rs   (new module)
```

Key components needed:

1. **`AudioStreamer` struct** — owns the encode thread handle, ring buffer, and state:
```rust
pub struct AudioStreamer {
    // Bounded lock-free ring buffer (crossbeam or rtrb)
    buffer: Option<Arc<Producer<PcmBlock>>>,
    // Encode thread handle
    encode_thread: Option<JoinHandle<()>>,
    // Output configuration
    format: Codec,
    sample_rate: u32,
    stereo: bool,
    status: AtomicU8,  // IDLE, WRITING, STOP_PENDING, ERROR
}
```

2. **`PcmBlock`** — unit of transfer from RT thread to encode thread:
```rust
struct PcmBlock {
    left: Box<[f32]>,
    right: Box<[f32]>,
    frame_count: u32,
    timestamp: u64,
    markers: SmallVec<[u32; 4]>,  // sample-accurate marker positions
}
```

3. **Encode thread loop** — pulls blocks from ring buffer, encodes via `IFileEncoder`:
   - Reuse `file_codecs.rs::Codec` / `IFileEncoder` / `SndFileEncoder`
   - Open output file on `StartWriting()`
   - Loop: `recv()` → `encoder.encode_block()` until STOP_PENDING
   - Finalize file on stop

4. **Integration with `RuntimeAudioProcessor`** — add `streamer: Option<AudioStreamer>` to `LoopSlot` or `RuntimeAudioProcessor`, route input monitor signal through it per-frame.

5. **Integrate with `native_runtime.rs`** — `ToggleDiskOutput` action already exists in `RuntimeEventDispatcher`. Wire it to create/start/stop `AudioStreamer`.

**Difficulty:** Medium. The crossbeam/rtrb ring buffer is already available as a dependency. The encode crate integration already exists. The main work is threading and signal routing.

---

### 1.2 PulseSync / PulseSyncCallback — User-Defined Sync Callbacks

**C++ source:** `fweelin_core_dsp.h` / `fweelin_core_dsp.cc` — `PulseSync` struct, `PulseSyncCallback` interface

**What it does:** Allows DSP processors (RecordProcessor, PlayProcessor) to register position callbacks on the Pulse timeline. When the Pulse reaches a registered position in its cycle, it fires the callback. This is the mechanism for sample-accurate loop quantization — starting/stopping recording and playback at exact pulse boundaries.

C++ mechanism:
```cpp
class PulseSyncCallback {
    virtual void PulseSync(int syncidx, nframes_t actualpos) = 0;
};
class PulseSync {
    PulseSyncCallback *cb;
    nframes_t syncpos;
};
// Pulse stores syncpos[MAX_SYNC_POS] array with up to 1000 entries.
```

**Rust equivalent:** None. The `Event::PulseSync` enum variant exists but is not wired to any callback mechanism. Pulse sync is handled implicitly through `RuntimeAudioProcessor`'s downbeat detection in its per-frame loop.

**How to fill it:**

In `src/core_dsp.rs`:

1. **`SyncPosition` struct:**
```rust
struct SyncPosition {
    position: NFrames,       // sample position within pulse length
    callback: Box<dyn FnMut(i32, NFrames) + Send>,  // closure-based callback
    index: i32,              // user-provided identifier
}
```

2. **Extend `Pulse` struct** with:
```rust
pub struct Pulse {
    // ... existing fields ...
    sync_positions: Vec<SyncPosition>,
    max_sync_positions: usize,  // default 1000
}
```

3. **`add_sync_position()` / `del_sync_position()` methods**:
```rust
pub fn add_sync(&mut self, pos: NFrames, cb: Box<dyn FnMut(i32, NFrames) + Send>) -> i32
pub fn del_sync(&mut self, index: i32) -> bool
```

4. **Fire callbacks in `process()` loop** — when `curpos` crosses a registered position, invoke the callback. This requires checking the position before and after each frame chunk.

5. **Wire `RecordProcessor` / `PlayProcessor` equivalents** (see sections 2.1, 2.2 below) to use `add_sync()` rather than the current implicit downbeat detection.

**Difficulty:** Low-Medium. The mechanism is well-defined. The main complexity is making it realtime-safe (the callbacks must not allocate).

---

## 2. Partially Ported Features (PARTIAL)

### 2.1 RecordProcessor — Fixed-Size Record, SyncUp, GetRecordedLength

**C++ source:** `fweelin_core_dsp.h` / `.cc` — `RecordProcessor` class (3 constructors, ~40 methods)

**Current Rust state:** Recording logic is integrated into `RuntimeAudioProcessor` in `native_dsp_graph.rs`. Fresh recording, overdubbing, and sync-aware End/EndNow are all present. The `Jump` fade-out/fade-in logic and `REC_TAIL_LEN` constant are ported.

**Missing pieces:**

| Feature | C++ | Rust | Impact |
|---------|-----|------|--------|
| Fixed-size record constructor (record into pre-existing AudioBlock) | `RecordProcessor(app, iset, inputvol, dest_block, stereo)` | Not present | Cannot record into a fixed-size preallocated buffer |
| `SyncUp()` | Re-syncs overdub to a newly created pulse | Not present | Overdub syncing uses different mechanism |
| `GetRecordedLength()` | Returns total recorded frames | Not present | No way to query record progress externally |
| `GetFirstRecordedBlock()` | Returns first AudioBlock of recording | Not present | Exposes recorded data for export |
| `compute_stats = 1` path | Computes peaks/sums/DC offset during fixed-size recording | Not present | Recording stats not available for fixed-size mode |

**How to fill it:**

In `src/native_dsp_graph.rs`:

1. **Record into pre-existing block**: Add a `RecordConfig` variant:
```rust
pub enum RecordConfig {
    Fresh { pulse: Option<PulseId>, grow_chain: bool },
    Overdub { loop_id: u8, ... },
    FixedSize { dest_block: LoopBlockChain },  // NEW
}
```

2. **`SyncUp()`**: After a pulse reset, re-register the pulse-sync callback for the recording slot. Add a method to `RuntimeAudioProcessor`:
```rust
fn resync_recording(&mut self, slot: u8) -> Result<(), ...>
```

3. **`GetRecordedLength()`**: Add a `recorded_frames` counter per `LoopSlot` and expose it:
```rust
pub fn recorded_frames(&self, slot: u8) -> NFrames
```

4. **Stats computation**: When recording in fixed-size mode, call the existing `record_scope_sample()` path for peak/DC-offset tracking (this is already done for overdub, just needs enabling for fixed-size).

**Difficulty:** Low. Most infrastructure exists, the gaps are wiring.

---

### 2.2 PlayProcessor — SyncUp()

**C++ source:** `fweelin_core_dsp.h` / `.cc` — `PlayProcessor` class

**Current Rust state:** Playing logic is integrated into `RuntimeAudioProcessor` per-frame loop. Volume control and pulse-sync restart with boundary fade are present.

**Missing:**

| Feature | C++ | Rust | Impact |
|---------|-----|------|--------|
| `SyncUp()` | Re-syncs a playing loop to a newly created pulse | Not present | Playback may drift after pulse changes |
| Halt/stopped as processor concept | Clear stopped flag + SS_ENDED | Implicit in RuntimeAudioProcessor frame loop | Less flexible halt handling |

**How to fill it:**

1. Add `SyncUp()` equivalent to `RuntimeAudioProcessor`:
```rust
pub fn resync_playback(&mut self, slot: u8) -> Result<(), ...>
```
This should re-align the loop's playback position (AudioBlockIterator position + phase offset) to the current pulse position, with a crossfade at the boundary (reuse existing fade logic).

**Difficulty:** Low. The fade-in/fade-out infrastructure already exists.

---

### 2.3 Pulse — AddPulseSync, Long Count, Metronome, MIDI Clock

**C++ source:** `fweelin_core_dsp.h` / `.cc` — `Pulse` class (extends Processor, ~50 fields)

**Current Rust state:** `src/core_dsp.rs::Pulse` has `len`, `curpos`, `wrapped`, `quantize_length`. The metronome, long count, and MIDI clock state have been moved to `RuntimeAudioProcessor`.

**Missing from `Pulse` struct:**

| Feature | C++ | Rust | Impact |
|---------|-----|------|--------|
| `AddPulseSync` / `DelPulseSync` | syncpos array of 1000 entries + callback invocation | Not in Pulse | Part of the PulseSync gap (section 1.2) |
| `Long count` | `lc_len`, `lc_cur`, `ExtendLongCount()`, `ResetLongCount()`, `GetLongCount_Len/Cur/CurPct` | In `RuntimeAudioProcessor` | Pulse doesn't own its beat counting |
| `Metronome` | Metro samples, hit/tone, active flag, volume | In `RuntimeAudioProcessor` | Pulse doesn't own metronome state |
| `MIDI clock` | `SetMIDIClock()`, `clockrun` state, prev_sync_bb/cnt/speed/type | In `RuntimeAudioProcessor` | Pulse doesn't own MIDI clock state |
| `SetLength` | Dynamic pulse length change | Present | ✅ OK |
| `Wrap()` | Force wrap to beginning | Present | ✅ OK |

**How to fill it:**

This is a design choice, not necessarily a bug. The C++ architecture made Pulse a full Processor with all timing responsibilities. The Rust architecture distributes these across `RuntimeAudioProcessor`'s per-frame loop. The distribution is intentional and functionally complete.

If you want to restore `Pulse` as the single owner of all timing:

1. Move metronome fields from `RuntimeAudioProcessor` to `Pulse`
2. Move long count fields from `RuntimeAudioProcessor` to `Pulse`
3. Move MIDI clock fields from `RuntimeAudioProcessor` to `Pulse`
4. Add `add_sync()` / `del_sync()` as described in section 1.2

**Recommendation:** Leave as-is. The current architecture is cleaner.

---

### 2.4 Processor Base — Per-Processor Pre-Buffer (dopreprocess)

**C++ source:** `fweelin_core_dsp.h` — `Processor` base class, `dopreprocess()`, `fadepreandcurrent()`

**Current Rust state:** `src/core_dsp.rs::SmoothState` provides `fade()` but it's not owned by each processor. The pre-buffer must be manually managed externally. `RootProcessor::do_preprocess()` in `core_dsp_root.rs` handles the preprocess call but it's not per-processor.

**Missing:**

| Feature | C++ | Rust | Impact |
|---------|-----|------|--------|
| Per-processor pre-buffer | Each `Processor` owns `preab` (AudioBuffers) + `prelen` + `prewritten`/`prewriting` flags | `SmoothState` is standalone, not owned per-processor | Each processor must manage its own smoothing externally |

**How to fill it:**

In `src/core_dsp.rs`, make `SmoothState` embeddable:
```rust
pub struct SmoothState {
    pub pre_len: usize,
    pub prewritten: bool,
    pub prewriting: bool,
    pub pre: Vec<Vec<Sample>>,
}

impl SmoothState {
    pub fn dopreprocess(&mut self, process: &mut dyn FnMut(&mut [Vec<Sample>])) {
        // Store current output as pre-buffer, then run processor
        self.prewriting = true;
        process(&mut self.pre);
        self.prewritten = true;
        self.prewriting = false;
    }

    pub fn fade_or_process(&mut self, outputs: &mut [Vec<Sample>], 
                           process: &mut dyn FnMut(&mut [Vec<Sample>])) {
        if !self.prewritten {
            process(outputs);
        } else {
            self.fade(outputs);
        }
    }
}
```

Then have `Processor` trait implementations optionally hold a `SmoothState` field.

**Difficulty:** Low.

---

### 2.5 MIDI — Auto-Bypass, Note Tracking, Bank/Program Change Send

**C++ source:** `fweelin_midiio.h` / `.cc`

**Current Rust state:** `src/midiio.rs` has basic I/O, echo routing, clock sync, channel routing. `midiio_platform.rs` has `MidirMidiBackend` and `RegistryMidiBackend`.

**Missing:**

| Feature | C++ | Rust | Impact |
|---------|-----|------|--------|
| `BypassInfo` / `CheckBypass` / `CheckUnbypass` | Auto-bypass for notes held across patch changes | Not present | Held notes don't auto-release on patch change |
| `ReceiveNoteOnEvent` / `ReceiveNoteOffEvent` / `ReceiveAftertouchEvent` / `ReceivePolyAftertouchEvent` | Create and post MIDI events from hardware input | Not present as MIDI-IO-level methods | Event creation is done in `NativeEventBridge` instead |
| `SetMIDIForPatch` / `SendBankProgramChange` | Route patches to specific MIDI ports+channels with bank select | Not present | Patch→MIDI port mapping not implemented |
| `note_def_port` / `note_patch` | Per-note tracking arrays | Not present | Cannot determine which patch a note was triggered from |

**How to fill it:**

In `src/midiio.rs`:

1. **Auto-bypass**: Add `held_notes: Vec<(u8, u8)>` (note, channel) to `MidiIo<B>`. On patch change, iterate held notes and send NoteOff for each, then clear:
```rust
pub fn release_held_notes(&mut self) {
    for (note, channel) in self.held_notes.drain(..) {
        self.send(&MidiMessage::NoteOff { channel, note, velocity: 0 });
    }
}
```

2. **`note_def_port` / `note_patch`**: Add tracking arrays:
```rust
note_port: [Option<u8>; 128],      // per-note port
note_patch: [Option<PatchRef>; 128], // per-note patch reference
```
Update on NoteOn, clear on NoteOff.

3. **`SendBankProgramChange()`**: Add method that sends CC 32 (bank select LSB), CC 0 (bank select MSB), then Program Change:
```rust
pub fn send_bank_program(&mut self, channel: u8, bank: u8, program: u8)
```

4. **`SetMIDIForPatch()`**: Add `PatchMidiRoute` struct:
```rust
pub struct PatchMidiRoute {
    pub port: u8,
    pub channel: u8,
    pub bank: u8,
    pub program: u8,
}
```
Store in `patch_routes: HashMap<PatchId, PatchMidiRoute>`. Apply when a patch is triggered.

**Difficulty:** Medium. Mostly mechanical additions, no new dependencies.

---

### 2.6 AudioIO — Transport State for Non-JACK Backends

**C++ source:** `fweelin_audioio.h` / `.cc`

**Current Rust state:** `AudioBackend` trait in `audioio.rs`. JACK backend (`linux_native.rs`) implements `Timebase` with `IsSync()`, `IsTimebaseMaster()`, `IsTransportRolling()`, `GetTransportBar()`, `GetTransportBeat()`, `GetTransportBPM()`, `GetTransportBPB()`. CPAL and macOS AudioUnit backends always return `transport_rolling = false`.

**Missing:**

| Feature | C++ | Rust | Impact |
|---------|-----|------|--------|
| Transport roll detection for non-JACK backends | JACK timebase callback | CPAL/AudioUnit: always false | Pulse sync and quantization won't work with CPAL/AudioUnit backends |
| GetTransport_Bar/Beat/BPM/BPB for non-JACK | JACK transport info | CPAL/AudioUnit: no transport info available | Cannot sync to external transport |

**How to fill it:**

This is a non-trivial architectural gap. JACK provides transport as a first-class feature. CPAL and CoreAudio AudioUnits don't.

Options:
1. **Internal transport clock**: Treat the `Pulse` as the transport master when using CPAL/AudioUnit. Always report `is_timebase_master = true`, derive bar/beat/bpm from `Pulse` state.
2. **MIDI clock slave**: Add a `MidiClockSlave` mode that derives transport from incoming MIDI clock.
3. **Accept the limitation**: Document that transport sync requires JACK. Lowers portability but is honest.

**Recommendation:** Option 1 (internal transport from Pulse) — it's ~20 lines of wiring and provides the correct UX.

---

### 2.7 SDLIO — Legacy handle_key Handler

**C++ source:** `fweelin_sdlio.h` / `.cc`

**Current Rust state:** Synchronous polling via `poll_event()`. `KeySettings`, key held tracking present. `sdlkey_compat.rs` provides SDL1→SDL2 key translation.

**Missing:**

| Feature | C++ | Rust | Impact |
|---------|-----|------|--------|
| `handle_key()` | Legacy hardcoded handler for F1-F10 subdivide, greyENTER status report, modifier key tracking | Not ported | F1-F10 quick key functions and greyENTER status not available |
| Separate SDL thread | `pthread_create` + `run_sdl_thread` + wakeup pipe | Synchronous `poll_event()` in main loop | Different threading model, functionally equivalent for UI |
| Unicode/KeyRepeat enable | Inline C++ methods | `unicode_input_enabled`/`key_repeat_enabled` fields | ✅ Present in Rust |

**How to fill it:**

The separate SDL thread is intentional — Rust uses the main thread for SDL polling, which is simpler and avoids inter-thread synchronization for input. Not a gap.

The `handle_key()` handler can be ported to `native_runtime.rs` or `sdlio.rs`:
```rust
// In native_runtime.rs or a new key_handler module
fn handle_legacy_key(keysym: i32) -> Option<Event> {
    match keysym {
        SDLK_F1..=SDLK_F10 => Some(Event::GoSub { sub: keysym - SDLK_F1 + 1, ... }),
        SDLK_greyENTER => { /* status report */ None }
        _ => None,
    }
}
```

**Difficulty:** Low.

---

### 2.8 amixer — Shell-Based ALSA Control

**C++ source:** `fweelin_amixer.h` / `.cc`

**Current Rust state:** `src/amixer.rs` wraps `amixer` shell command.

**Missing:** Rich control introspection: `control_type`, `control_access`, `get_integer`, `parse_control_id`, `show_control_id`, `decode_tlv`, `cset` with value ranges. The C++ implementation (~700 lines of direct ALSA asoundlib C API) is replaced by ~100 lines of shell command wrapping.

**How to fill it:**

Either:
1. Create a `alsa-sys` or `alsa` crate-based implementation that calls `snd_ctl_elem_id`, `snd_hctl`, TLV decoding directly.
2. Accept the shell-based approach as sufficient for `AlsaMixerControlSet`'s use case (setting mixer values by name).

**Recommendation:** The shell approach works for the one use case (`AlsaMixerControlSet` event). The C++ ALSA code handles control enumeration and introspection that isn't needed in the Rust port. Keep as-is unless full ALSA mixer control is required.

---

### 2.9 Browser — Callback Traits Not Ported

**C++ source:** `fweelin_browser.h` / `.cc`

**Missing:**

| Feature | C++ | Rust | Impact |
|---------|-----|------|--------|
| `BrowserCallback` | `ItemBrowsed()`, `ItemSelected()`, `ItemRenamed()` callbacks | Not present as traits | Browser→app communication uses different mechanism (scene layer) |
| `RenameCallback` | `ItemRenamed(char *nw)` | Not present as trait | Rename result communicated differently |
| `MouseButton`/`MouseMotion` on browser items | Browser handles mouse directly | Not ported | Mouse interaction deferred to scene layer |

**How to fill it:**

The Rust architecture handles these through the scene/native_runtime layer rather than callbacks. The functional gap is zero — the communication still happens, just through different channels. No action needed.

---

### 2.10 Datatypes — Missing Data Structures

**C++ source:** `fweelin_datatypes.h` / `.cc`

**Missing (intentionally not ported):**

| Feature | C++ | Rust Replacement | Justification |
|---------|-----|------------------|---------------|
| `SRMWRingBuffer<T>` | Lock-free single-reader multi-writer ring buffer | `crossbeam::ArrayQueue` + `rtrb::RingBuffer` | External crates provide these |
| `SLinkList` | Intrusive singly-linked list | `Vec<T>` / `Vec<Weak<T>>` | Rust's standard containers |
| `DLinkList` | Intrusive doubly-linked list | Not needed | Never used in Rust |
| `RT_RWThreads` | Thread registration with `pthread_key_t` | `RcuRegistry::register_current()` | Simplified to counter-based |

**How to fill it:** No action needed — these were replaced by standard Rust containers and patterns.

---

## 3. Event System — Specific Field-Level Gaps

| Gap | C++ Event | Field | Rust Event | Impact |
|-----|-----------|-------|------------|--------|
| `presslen` | KeyInputEvent | `int presslen` | Not present | Press-and-hold duration tracking not available |
| `presslen` | LoopClickedEvent | `int presslen` | Not present | Same |
| `presslen` | JoystickButtonInputEvent | `int presslen` | Not present | Same |
| `presslen` | MouseButtonInputEvent | `int presslen` | Not present | Same |
| `od_fb` | Not in any Event — this is an overdub feedback reference | Variable reference for overdub feedback | TriggerLoop doesn't carry overdub feedback variable | The `TriggerLoop` event only has `overdub: bool`, not a feedback level variable |

**How to fill `presslen`:**

Add `presslen: u32` to the relevant Event variants. This requires:
1. Adding the field to the Event enum variant
2. Updating `get_type()` (no change needed, it ignores fields via `..`)
3. Updating all call sites that construct these events (mainly `native_event_bridge.rs`)
4. Populating the field from SDL event timing

**Difficulty:** Low.

**How to fill `od_fb`:**

If the overdub feedback variable reference is needed, add a field to `Event::TriggerLoop`:
```rust
Event::TriggerLoop {
    index: i32,
    vol: f32,
    engage: i32,
    shot: bool,
    overdub: bool,
    overdub_feedback_var: Option<String>,  // NEW: variable name for feedback level
}
```

This would require updates to `config.rs::instantiate_bound_event()` and `runtime_event_actions.rs::map_binding()`.
**Difficulty:** Low.

---

## 4. Style / Cleanup Items

| Issue | Location | Suggestion |
|-------|----------|------------|
| `block.rs:get_fragment()` chain traversal gap | `src/block.rs:225-228` | `get_fragment()` doesn't traverse `next` chain — add chain traversal or comment why not |
| `LoopTrayItem` duplicate | `src/core.rs` + `src/looplibrary.rs` | Two copies of `LoopTrayItem` — consolidate into one |
| `FloLayoutBox` duplicate | `src/videoio_display_widgets.rs` | Duplicate of `video_layout::FloLayoutBox`, only used in test — either remove or use canonical version |
| Missing SAFETY comments | `dsp_profile.rs:33`, `processor_queue.rs:52`, `signal.rs` (multiple), `videoio_platform.rs` (multiple) | Add `// SAFETY:` comments to all unsafe blocks |
| `core_event()` dead code | `src/application_services.rs:36` | Function is actually called at line 167 — not dead, ignore this finding |
| No `UnlistenEvent` | `src/event.rs` | Rust `EventManager::listen()` has no inverse — add if listener removal is needed |

---

## 6. JACK Audio/MIDI Wiring Plan

### Current State

| Aspect | C++ | Rust | Status |
|--------|-----|------|--------|
| JACK audio | `jack_client_open()` via `fweelin_audioio.cc` | `JackAudioMidiBackend` in `linux_native.rs` | ✅ Backend implemented |
| JACK MIDI | `snd_seq_*` ALSA (not JACK MIDI on Linux) | `MidirMidiBackend` (cross-platform) | ❌ JACK MIDI ports exist but unused |
| JACK transport | `jack_transport_query()`, `jack_set_timebase_callback()` | `TransportCommand` + `Timebase` in `JackProcess` | ✅ Implemented |
| Wired into production | Yes — default backend | **No** — `native_runtime.rs` uses CPAL/AudioUnit | ❌ Not wired |
| macOS JACK | C++ used JACK on all platforms | JACK only built for Linux | ❌ Not built on macOS |
| MIDI backend | ALSA seq on Linux, CoreMIDI on macOS | `MidirMidiBackend` uses `midir` crate | ✅ OK as fallback |

### Problem

1. `JackAudioMidiBackend` lives inside `linux_native.rs` which is `#[cfg(target_os = "linux")]` gated at the re-export line (542). On macOS the module compiles but the backend is not publicly exported.
2. `native_runtime.rs` hardcodes `CpalAudioBackend` on Linux and `MacosAudioUnitBackend` on macOS at compile time — no runtime selection.
3. The `jack` crate is only a dependency for Linux (`[target.'cfg(target_os = "linux")'.dependencies]`).
4. MIDI from `JackAudioMidiBackend` (JACK MIDI ports → `InlineMidi` ring buffer) is never connected — the production runtime uses `MidirMidiBackend` unconditionally.
5. On macOS, JACK works via the `jack` crate (macOS has JACK.framework or jackd via Homebrew), but the crate isn't even compiled for macOS.

### Implementation Plan

#### Step 1: Extract JACK Backend Into Its Own Module

**Move** `JackAudioMidiBackend`, `JackProcess`, `Notifications`, `Shared`, `InlineMidi`, `PendingQueues`, `TransportCommand`, and `Timebase` from `src/linux_native.rs` into a new `src/jack.rs`.

**Rationale:** `linux_native.rs` currently bundles JACK audio+MIDI+transport + ALSA mixer. JACK works on both platforms; ALSA is Linux-only. Splitting keeps the gating correct.

New files:
```
src/
  jack.rs           # JACK audio+MIDI+transport backend (cfg: linux, macos)
  linux_native.rs   # retains only DirectAlsaMixerBackend (cfg: linux only)
```

`src/jack.rs` structure:
```rust
//! JACK Audio Connection Kit backend — audio, MIDI, and transport.
//!
//! Works on Linux and macOS. Requires a running JACK server (jackd).
//! Falls back to CPAL/AudioUnit when JACK is unavailable.

// Platform-independent types (always compiled, no JACK dependency)
pub struct JackOptions {
    pub midi_inputs: usize,
    pub midi_outputs: usize,
    pub client_name: String,
    pub connect_audio: bool,       // auto-connect to physical ports
    pub connect_midi: bool,
    pub realtime: bool,            // enable RT scheduling
}

impl Default for JackOptions {
    fn default() -> Self {
        Self {
            midi_inputs: 1,
            midi_outputs: 1,
            client_name: "FreeWheeling".into(),
            connect_audio: true,
            connect_midi: true,
            realtime: true,
        }
    }
}

// Platform-specific impl (requires JACK server)
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod native { ... }  // current JackAudioMidiBackend moved here

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use native::JackAudioMidiBackend;
```

`JackOptions` is always compiled (platform-independent struct) so `native_runtime.rs` can reference it without cfg gates. The `JackAudioMidiBackend` itself is gated.

#### Step 2: Make `jack` Crate a Cross-Platform Dependency

In `Cargo.toml`:

```toml
# Before:
[target.'cfg(target_os = "linux")'.dependencies]
jack = "0.13.5"

# After — both platforms:
[target.'cfg(any(target_os = "linux", target_os = "macos"))'.dependencies]
jack = "0.13.5"
```

The `jack` crate 0.13.5 builds on macOS when JACK is installed (via `pkg-config`). It links against `libjack.dylib` (provided by Homebrew: `brew install jack` or the JACK Audio Toolkit).

**Build-time detection:** Add a `links = "jack"` build script or document that `libjack` must be installed at build time. The `jack` crate's build.rs already calls `pkg-config` to find the library.

#### Step 3: Extend `AudioBackend` Trait for JACK-Specific Operations

The current `AudioBackend` trait in `audioio.rs` is sufficient for audio but misses two JACK-specific capabilities:

```rust
pub trait AudioBackend: Send {
    // Existing methods...
    fn open(&mut self, client_name: &str) -> Result<BackendInfo, String>;
    fn activate(&mut self, callback: AudioCallbackFn) -> Result<(), String>;
    fn close(&mut self);
    fn relocate(&mut self, frame: NFrames);
    fn metrics(&self) -> AudioMetrics;
    fn cpu_load(&self) -> Option<f32>;
    fn input_latency_frames(&self) -> NFrames;
    fn recovery_requested(&self) -> bool;
    fn recover(&mut self) -> Result<BackendInfo, String>;

    // NEW: JACK-specific transport query
    fn transport_state(&self) -> TransportState {
        TransportState::default()  // default = not rolling
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TransportState {
    pub rolling: bool,
    pub frame: u32,
    pub bar: i32,
    pub beat: i32,
    pub bpm: f64,
    pub beats_per_bar: f32,
    pub beat_type: i32,
}
```

The existing `JackAudioMidiBackend` already tracks `shared.rolling` and `shared.frame`. The `JackProcess` already queries `client.transport().query()`. The method just needs exposing.

#### Step 4: Add `send_midi` / `receive_midi` to `AudioBackend` (Optional)

JACK provides audio + MIDI in a single callback. Currently MIDI goes through a separate `MidirMidiBackend`. For JACK, MIDI must come from the same process callback. Options:

**Option A (cleaner):** Make MIDI a separate abstracted path. `native_runtime.rs` polls both `AudioBackend::receive_midi()` (for JACK) and `MidirMidiBackend` (for standalone MIDI). JACK backend returns MIDI events from its ring buffer; CPAL/AudioUnit return `None`.

```rust
pub trait AudioBackend: Send {
    // ... existing ...
    fn receive_midi(&mut self) -> Option<MidiPortMessage> { None }
    fn send_midi(&mut self, _msg: MidiPortMessage, _offset: NFrames) -> Result<(), String> {
        Err("MIDI not supported by this backend".into())
    }
}
```

**Option B (simpler):** Keep `MidirMidiBackend` always active, even with JACK audio. JACK MIDI ports are not used — MIDI goes through ALSA/CoreMIDI via midir. Simpler but loses JACK's sample-accurate MIDI timing and port naming.

**Recommendation:** Option A, with fallback to Midir when JACK is not the audio backend. This preserves sample-accurate MIDI timing.

#### Step 5: Runtime Backend Selection

Add an enum and env-var/config-based selection in `native_runtime.rs`:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioBackendKind {
    #[default]
    Auto,        // JACK if available, else CPAL/AudioUnit
    Jack,
    Cpal,
    AudioUnit,   // macOS only
}
```

Selection logic (in `NativeRuntime::setup_audio`):

```rust
fn select_backend(kind: AudioBackendKind) -> Result<Box<dyn AudioBackend>, String> {
    match kind {
        AudioBackendKind::Jack => {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            {
                let mut backend = jack::JackAudioMidiBackend::new(1, 1);
                backend.open("FreeWheeling")?;
                return Ok(Box::new(backend));
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            { Err("JACK backend not available on this platform".into()) }
        }
        AudioBackendKind::Cpal => {
            Ok(Box::new(CpalAudioBackend::new(DeviceSelection::default(), CpalAudioOptions::default())))
        }
        AudioBackendKind::AudioUnit => {
            #[cfg(target_os = "macos")] {
                Ok(Box::new(MacosAudioUnitBackend::new()))
            }
            #[cfg(not(target_os = "macos"))] {
                Err("AudioUnit is macOS-only".into())
            }
        }
        AudioBackendKind::Auto => {
            // Try JACK first, fall back to platform default
            #[cfg(any(target_os = "linux", target_os = "macos"))] {
                if let Ok(backend) = Self::select_backend(AudioBackendKind::Jack) {
                    return Ok(backend);
                }
            }
            #[cfg(target_os = "macos")]
            { Self::select_backend(AudioBackendKind::AudioUnit) }
            #[cfg(not(target_os = "macos"))]
            { Self::select_backend(AudioBackendKind::Cpal) }
        }
    }
}
```

**This requires `AudioBackend` to be object-safe.** Currently all methods take `&mut self` or `&self` — no `Sized` constraints, no generic parameters on methods. The only issue: `activate` takes `AudioCallbackFn` which is `Box<dyn FnMut(...)>` — this is object-safe. The `AudioIO<B: AudioBackend>` struct is generic, so it would need a type-erased sibling:

```rust
pub struct DynAudioIO {
    backend: Box<dyn AudioBackend>,
    // ... same fields as AudioIO ...
}
```

Or, to minimize refactoring, use an enum:

```rust
pub enum AnyAudioBackend {
    Cpal(CpalAudioBackend),
    Jack(JackAudioMidiBackend),
    AudioUnit(MacosAudioUnitBackend),
}

impl AudioBackend for AnyAudioBackend {
    fn open(&mut self, name: &str) -> Result<BackendInfo, String> {
        match self {
            Self::Cpal(b) => b.open(name),
            Self::Jack(b) => b.open(name),
            Self::AudioUnit(b) => b.open(name),
        }
    }
    // ... delegates all methods ...
}
```

**Recommendation:** Use the enum approach. It avoids `Box<dyn ...>` overhead, keeps all backends available at compile time, and makes `AudioIO<AnyAudioBackend>` work without changes.

#### Step 6: Wire JACK MIDI Into the Event Pipeline

Currently `native_runtime.rs` polls MIDI via:
```rust
fn poll_midi(&mut self) {
    while let Some(event) = self.resources.borrow_mut().midi.as_mut().and_then(|m| m.receive()) {
        // ... forward to event bridge ...
    }
}
```

When using the JACK backend, MIDI comes from `audio.backend().receive_midi()` instead:

```rust
fn poll_midi(&mut self) {
    // Poll JACK MIDI (if backend provides it)
    if let Some(audio) = self.resources.borrow().audio.as_ref() {
        while let Some(msg) = audio.backend().receive_midi() {
            Self::handle_midi_message(&mut self.resources.borrow_mut(), msg);
        }
    }
    // Poll standalone MIDI backend (Midir for CPAL/AudioUnit fallback)
    if let Some(midi) = self.resources.borrow_mut().midi.as_mut() {
        while let Some(event) = midi.receive() {
            // ... forward to event bridge ...
        }
    }
}
```

To avoid double-receiving, disable the `MidirMidiBackend` when using JACK:
- JACK backend: `self.midi = None` (MIDI via `audio.backend().receive_midi()`)
- CPAL/AudioUnit backend: `self.midi = Some(MidiIo::new(MidirMidiBackend::new(...)))` (MIDI via `midir`)

#### Step 7: macOS-Specific Considerations

| Concern | Detail |
|---------|--------|
| **JACK on macOS** | Requires `jackd` or `JackRouter` installed. Available via Homebrew (`brew install jack`). The `jack` crate links against `libjack.0.dylib`. |
| **AudioUnit vs JACK** | macOS AudioUnit backend provides lower latency via HAL AudioUnit. JACK adds transport sync. Default should remain AudioUnit; JACK selectable via config/env. |
| **App bundle** | JACK's dylib must be in the library path. Document as a build prerequisite or add a brew-install check in the build script. |
| **macOS startup guard** | `StartupGuard` rollback must handle JACK client deactivation on setup failure. |

#### Step 8: Cargo.toml Dependency Changes

```toml
# Current:
[target.'cfg(target_os = "linux")'.dependencies]
jack = "0.13.5"
alsa = "0.11.0"

[target.'cfg(target_os = "macos")'.dependencies]
coreaudio-sys = { version = "0.2", features = ["audio_unit"] }

# Proposed:
[target.'cfg(target_os = "linux")'.dependencies]
jack = "0.13.5"
alsa = "0.11.0"

[target.'cfg(target_os = "macos")'.dependencies]
jack = "0.13.5"
coreaudio-sys = { version = "0.2", features = ["audio_unit"] }
```

#### Step 9: Library Module Declaration

In `src/lib.rs`:

```rust
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod jack;           // NEW

pub mod linux_native;   // unchanged, DirectAlsaMixerBackend stays
```

#### Step 10: Testing the JACK Backend

| Test | What to verify |
|------|----------------|
| `cargo test --features jack` | Unit tests for `JackOptions`, `TransportState`, `Timebase` |
| Acceptance test with JACK | `cargo run -- --smoke-test` with `FWEELIN_AUDIO_BACKEND=jack` |
| Audio callback rate | Correct `JackPosition.frame` and `nframes` delivered to DSP |
| MIDI I/O | Events written to `midi_in` ports appear in `receive_midi()` output |
| Transport commands | `transport(TransportCommand::Start)` triggers JACK transport |
| Port auto-connect | Audio/MIDI ports connect to physical ports when `connect_audio`/`connect_midi` is true |
| macOS JACK | Same tests on macOS with `libjack` installed |

### Summary of Changes

| File | Change |
|------|--------|
| `Cargo.toml` | Add `jack` to `[target.'cfg(target_os = "macos")'.dependencies]` |
| `src/jack.rs` | **New file.** Extract `JackAudioMidiBackend` + `JackProcess` + `InlineMidi` + `TransportCommand` + `Timebase` from `linux_native.rs` |
| `src/audioio.rs` | Add `TransportState` struct, optional `transport_state()` + `receive_midi()` + `send_midi()` to `AudioBackend` trait |
| `src/audioio.rs` | Add `AnyAudioBackend` enum delegating all `AudioBackend` methods |
| `src/linux_native.rs` | Remove JACK code, keep `DirectAlsaMixerBackend` |
| `src/lib.rs` | Add `pub mod jack` with cfg gate |
| `src/native_runtime.rs` | Add `AudioBackendKind` enum, runtime backend selection, JACK MIDI polling path |
| `src/native_runtime.rs` | Plumb `JackOptions` from config/env: `FWEELIN_JACK_MIDI_INPUTS`, `FWEELIN_JACK_MIDI_OUTPUTS`, `FWEELIN_AUDIO_BACKEND` |

### Migration Path (No-Regret)

Steps that can be done independently and each improves the codebase:

1. ✅ Extract JACK into its own module (cleaner architecture regardless)
2. ✅ Add `TransportState` to `AudioBackend` trait (CPAL/AudioUnit return defaults, no functional change)
3. ✅ Add `receive_midi()` / `send_midi()` to `AudioBackend` trait (CPAL returns None)
4. ❌ Runtime backend selection (requires all above + testing)
5. ❌ macOS JACK support (requires `jack` crate on macOS + testing)

## 5. Summary

| Category | Count | Details |
|----------|-------|---------|
| **MISSING** | 2 | FileStreamer, PulseSync callback mechanism |
| **PARTIAL (notable)** | 10 | RecordProcessor, PlayProcessor, Pulse, Processor base, MIDI auto-bypass, AudioIO transport, ALSA mixer, Browser callbacks, SDL handle_key, Datatypes ring-buffer/lists |
| **Field gaps** | 5 | `presslen` (4 events), `od_fb` (TriggerLoop) |
| **Style/cleanup** | 6 | Block chain, LoopTrayItem duplicate, FloLayoutBox duplicate, SAFETY comments (4 files), UnlistenEvent |
| **Fully ported** | 30+ | All Event types, EventManager, AudioLevel, AudioBuffers, InputSettings, RootProcessor, AutoLimitProcessor, AudioBlock, block managers, codecs, Loop/Snapshot types, config, Saveable, Browser items, video types, SDL core, OSC, FluidSynth, MemManager, stacktrace |

### Priority Order for Filling Gaps

1. **FileStreamer** — critical for DAW export / disk streaming functionality. Affects production recording use cases.
2. **Transport state for CPAL/AudioUnit** — makes quantization work without JACK. Affects core loop recording UX.
3. **MIDI auto-bypass + note tracking** — affects patch-change handling and held-note correctness.
4. **PulseSync callback mechanism** — affects sample-accurate loop quantization for third-party processors.
5. **RecordProcessor `SyncUp()` + `GetRecordedLength()`** — polish for recording UX.
6. **`presslen` fields** — legacy event compatibility.
7. **Cleanup items** — SAFETY comments, duplicates, `block.rs` chain traversal.
