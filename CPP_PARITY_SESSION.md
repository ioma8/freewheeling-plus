# C++ parity audit session — status handoff

Session date: 2026-07-16. Comparison target: `/Users/jakubkolcar/projects/customs/freewheeling` (C++ source of truth).

## Committed (done, tested)

1. **`6995643` Match C++ Pulse sync edge cases exactly** — removed `sync_speed.max(1)` clamps, the `bpm > 0.` gate, and the `len == 0` guard in `quantize_length` in `core_dsp_pulse.rs`. NOTE: this module was later found to be dead code (see below); these fixes were re-applied where they matter when porting the logic into the live engine.
2. **`88f1795` Remove unused core_dsp_processors scaffold** — deleted `src/core_dsp_processors.rs` (never referenced; real DSP engine is `native_dsp_graph.rs`).
3. **`d7be620` Ramp overdub feedback per sample** — added `feedback_last` per loop slot + per-sample linear ramp in `native_dsp_graph.rs`, matching C++ `fb_delta`/`od_feedback_lastval`.
4. **`caac73d` Surface CPAL audio timing gaps** — `activate()` now waits for first playback callback (not just capture); frame-count mismatch vs negotiated buffer size counts as xrun; ring-buffer trim (implicit resampler across split capture/playback clocks) counts as capture_overruns.
5. **`1cbda38` Fix SDL key name table** — replaced ad-hoc `key_name`/`key_from_name` in `sdlio.rs` with verbatim 323-entry `SDL_names[]` transcription. Fixed: `"a"` resolved to 65 instead of 97 (broke ALL letter-key XML bindings: b/t/u/s/x/z/d), and ~30 missing names (slash, backslash, KPplus, KPminus, tilde, minus, equal, …) that resolved to 0.
6. **`ba092d1` Fix pulse-synced recording truncation and unwired delete-pulse** — (a) `recording_started_late` flag was never cleared, routing every later stop through an elapsed-time heuristic that silently cropped captured audio (5200→5024 frames traced); now cleared once a full beat completes. (b) `delete-pulse` (space+F1) was aliased to deselect; added real `RuntimeCommand::DeletePulse` that erases all pulse-synced loops like C++ `LoopManager::DeletePulse`.

## Completed in this working tree

All in the working tree, part of one coherent change-set ("clean solution" for three audit findings):

1. **Resync gating fix (DONE, tested)** — `native_dsp_graph.rs` downbeat block: Playing/Overdubbing loop resync now gated on `pulse_long_count % pulse_beats == 0` (the loop's wrap beat), matching C++ `PulseSync`'s `curbeat >= nbeats`. C++ does NOTHING on intermediate downbeats. Rewrote test `synced_playback_skips_the_record_tail_at_each_pulse_boundary` → `synced_playback_jumps_only_at_the_loop_wrap_beat_like_pulse_sync` (old test pinned wrong, Rust-invented behavior).
2. **Tap-pulse implementation (DONE, tested)** — `RuntimeCommand::TapPulse { new_len }` in engine: first tap arms zero-length stopped pulse (`pulse_tap_armed`, `pulse_prev_tap`), second tap defines length, later taps retune within C++ constants (TIMEOUT_RATIO 5.0, GRADUATION 0.0, REJECT_TOLERANCE 1.0). `Wrap()` vs `SetPos(0)` distinction via new `pulse_downbeat_suppressed` flag consumed at top of frame loop. Wired: `EventType::TapPulse` → `ApplicationAction::TapPulse` → command; `r.pulse_selected = true` on new_len tap. Tests: `tap_pulse_arms_then_defines_length_and_reanchors_the_downbeat`, `tap_pulse_rejects_a_new_length_beyond_the_cpp_timeout` (both pass).
3. **MIDI clock transmit + transport-slave sync**:
   - `AudioCallback` gained `transport_rolling: bool`; all 12 construction sites updated (JACK backend passes real `rolling`, CPAL/CoreAudio/tests pass `false`). Test helper `run_with_transport` added.
   - Engine (`native_dsp_graph.rs`): new fields `midi_sync_transmit`, `clock_run: ClockRun {None,Start,Beat}` (= C++ SS_NONE/SS_START/SS_BEAT), `midi_clock_count`, `midi_beat_count`, `sync_speed: i32` (raw/unclamped like C++), `sync_type`, `prev_bpm`, `prev_sync_bb`, `prev_sync_speed: -1`, `prev_sync_type`, `sync_cnt`, `sample_rate`.
   - New commands: `SetMidiSyncTransmit(bool)`, `SetSyncSpeed(i32)`, `SetSyncType(bool)` (handlers done).
   - New statuses: `RuntimeStatus::MidiClockTick`, `RuntimeStatus::MidiTransportOutput { running }`.
   - Transport-slave block ported to top of `process()` (BPM→pulse_frames recompute, bar/beat wrap counting) — verbatim C++ `Pulse::process` 479-520.
   - Clock generation ported into frame loop at pulse-advance point (24 PPQN, `clocks_per_pulse = 24 * sync_speed * (sync_type ? 1 : 4)`, float frames_per_clock, crossing detection, wrap fires pending START) — drives the previously-dead `metro_hi_offset`/`metro_lo_offset` tone resets.
   - `clock_run` transitions added: `SetPulseFromLoop` → Start (C++ `CreatePulse`→`SetMIDIClock(1)`); TapPulse armed/existing branches → emit stop + Start (C++ "refresh sync" = `SelectPulse(-1);SelectPulse(idx)`); `ClearPulse` → emit stop gated on transmit (C++ `SetMIDIClock(0)`); `DeletePulse` → clock dies silently (C++ sends no stop).
   - `native_runtime.rs`: `SetMidiSync`/`SetSyncType`/`SetSyncSpeed` actions now also forward commands to engine.
   - `native_runtime.rs`: the status-drain loop now forwards engine clock/start/stop statuses to the configured MIDI sync outputs in FIFO order.
   - Engine tests cover first-wrap START, 96 clocks per 960-frame pulse, metronome beat/bar re-arming, and JACK transport-slave BPM/bar changes.

4. **Delete dead DSP scaffolds** — removed `src/core_dsp_pulse.rs` and its `lib.rs` entry, and removed the stale integration test for the previously deleted `core_dsp_processors` scaffold; the live engine owns the ported logic.

## Verification completed

- `cargo fmt`
- `cargo test --lib native_dsp_graph::tests:: -- --test-threads=1` — 40 passed
- `cargo clippy --lib` — passed with the existing 14 baseline warnings and no new warnings.
- `cargo test --quiet` — all targets passed: 236 library tests plus every binary/integration target.

## Flagged, deliberately NOT changed

- `capture_alignment_frames` (CPAL input-latency compensation applied at trigger AND every wrap): Rust-only mechanism, self-consistent and deliberate; kept. It is a documented 1:1 deviation compensating the split-stream backend.
- C++ `RecordProcessor::FadeOut_Input` fades over fixed 64-frame `prelen`; Rust `overdub_fade_out` uses `total.max(frames)` (whole callback). Pre-existing, unverified, worth its own pass.
- Rust emits MIDI clock per-frame-accurate; C++ emits at most one clock per fragment (can under-emit at large buffers). Rust is a strict refinement, noted in code comment.
- Multi-pulse (`pulses[MAX_PULSES]`) not representable — port models a single global pulse (pre-existing architectural simplification).
