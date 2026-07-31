# Reliable CoreAudio duplex capture

## Context

On macOS, FreeWheeling currently falls back to two independent CPAL streams
when the selected input and output devices have different CoreAudio device IDs.
The MacBook microphone/speaker route therefore has separate capture and
playback clocks connected by a bounded ring buffer. The playback callback can
trim valid captured frames or receive no frame and insert zeroes. Those frames
are recorded into loops, which causes the intermittent buzzing/chirped audio
seen in the diagnostic run.

The latency probe is not the source of this corruption. It measures the
output-to-microphone path and supplies a loop-alignment offset, but it cannot
repair dropped or fabricated capture frames. The reliable fix is to make the
input and output part of one CoreAudio clock domain before starting the DSP
callback.

## Goal

Create a private CoreAudio Aggregate Device programmatically when the selected
input and output devices differ, use that aggregate as the HAL AudioUnit's
single duplex device, and destroy only the aggregate owned by FreeWheeling on
shutdown or recovery.

The resulting path should be:

```text
input device + drift compensation
        + output device (master clock)
                    ↓
       private CoreAudio Aggregate Device
                    ↓
          one HAL duplex callback
                    ↓
             DSP and loop recording
```

The existing latency calibration remains after this change. It measures the
physical aggregate-device round trip and continues to provide the playback
alignment offset for newly recorded loops.

## Implementation steps

### 1. Define the ownership and route model

- [x] Add an explicit CoreAudio route state to `MacosAudioUnitBackend` containing
  the requested input/output IDs, the active device ID, whether the active
  device was created by FreeWheeling, and the aggregate UID.
- [x] Keep direct single-device operation when input and output already refer
  to the same device.
- [x] Select the output device as the aggregate master clock because playback
  is the clock consumed by the DSP callback and loop timeline.
- [x] Never destroy a device that was discovered rather than created by this
  process.

### 2. Add a small CoreAudio aggregate-device wrapper

- [x] Add macOS-only helpers in `src/macos_audio_unit.rs` for creating and
  destroying an aggregate device through `AudioHardwareCreateAggregateDevice`
  and `AudioHardwareDestroyAggregateDevice`.
- [x] Build the Core Foundation description with:
  - a unique, process-scoped aggregate UID;
  - a private-device flag so FreeWheeling does not alter the user's global
    audio-device list;
  - the selected output and input subdevices;
  - the selected output as `kAudioAggregateDeviceMasterSubDeviceKey`;
  - input drift compensation enabled through the subdevice drift-compensation
    property.
- [x] Keep all CF objects owned and released by one wrapper function; return a
  Rust error containing the failed CoreAudio operation and OSStatus.
- [x] Query the created device after creation and verify that it contains both
  the requested input and output subdevices before opening the AudioUnit.
- [x] Make creation idempotent within one backend instance and ensure cleanup
  runs on every failure path after creation.

### 3. Open the HAL AudioUnit against the aggregate

- [x] Change `configure` so the aggregate is created before setting
  `K_AUDIO_UNIT_PROPERTY_CURRENT_DEVICE` when device IDs differ.
- [x] Set the aggregate's negotiated sample rate and non-interleaved f32
  stereo format on both AudioUnit sides.
- [x] Configure the requested buffer size only after the aggregate is active;
  use the actual returned buffer size in `BackendInfo` and latency estimates.
- [x] Preserve the existing one-callback `AudioBufferList` path, but validate
  the number of input/output buffers and channel layout at activation.
- [x] If aggregate creation or verification fails, fail the reliable CoreAudio
  route with a clear diagnostic instead of silently using the known-corrupt
  split-stream recorder. A separately designed adaptive-resampler fallback
  can be added later, but must not be mixed into this implementation.

### 4. Make lifecycle and recovery leak-safe

- [x] Destroy the private aggregate only after the HAL AudioUnit has stopped and
  been uninitialized.
- [x] Ensure all setup failures unwind in reverse order: stop callback,
  uninitialize AudioUnit, dispose AudioUnit, restore device buffer size, then
  destroy the owned aggregate.
- [x] On route/device changes, close the old AudioUnit, destroy the old owned
  aggregate, create a new aggregate for the new route, and reopen the callback.
- [x] Do not destroy or replace an aggregate created externally by the user,
  even if its subdevices happen to match.
- [x] Reset the provisional recording alignment after route recreation and
  queue a fresh latency calibration only after the new duplex callback is
  running.

### 5. Remove the corruption mechanism from the reliable path

- [x] Ensure the aggregate path has one capture/playback callback and no CPAL
  capture ring buffer.
- [x] Add runtime assertions/diagnostics that capture and playback frame counts
  match and that no input frames are dropped or fabricated.
- [x] Keep the existing diagnostic counters for the CPAL fallback, but label
  that path explicitly as split-clock fallback.
- [x] Do not use the latency offset to compensate for frame loss; it is only a
  temporal alignment value.

### 6. Preserve calibration and loop alignment semantics

- [x] Start the probe only after the aggregate callback has produced valid
  input and output frames.
- [x] Measure latency in aggregate-device frames at the negotiated sample
  rate.
- [x] Apply the measured offset only to playback/capture alignment of newly
  recorded synced loops, preserving the current unsynchronised-loop behavior.
- [x] Reject or defer recording commands while calibration is active so probe
  audio cannot enter a recording through input-history prefill.
- [x] Keep calibration failure behavior safe: retain the driver estimate,
  report the failure, and never leave probe output active.

### 7. Add tests

- [x] Unit-test the aggregate-description builder with deterministic device
  IDs, UID, master-device selection, private flag, and drift-compensation
  settings.
- [x] Unit-test cleanup ownership: created aggregate is destroyed exactly once;
  externally supplied aggregate/device is never destroyed.
- [x] Add macOS integration coverage that opens a private aggregate from two
  available devices, runs the duplex callback, then closes it and verifies the
  device disappears.
- [x] Add a callback regression test that feeds a known microphone waveform and
  asserts every frame reaches recording in order with no zero insertion,
  trimming, or frame-count mismatch.
- [x] Retain the existing latency-calibration tests and add a route-reopen test
  proving the offset is recalibrated after device recreation.
- [x] Add an opt-in hardware acceptance test for the default MacBook route;
  keep it out of ordinary CI unless the required devices are present.

### 8. Update diagnostics and documentation

- [x] Log the aggregate UID, master device, subdevices, sample rate, buffer
  size, and whether drift compensation is enabled when diagnostics are active.
- [x] Log aggregate creation/destruction and every recovery reason with a
  stable `[AUDIO-DIAG]` prefix.
- [x] Update the README to explain that non-JACK macOS duplex audio uses a
  private aggregate device and that startup calibration measures the resulting
  physical round trip.
- [x] Document the explicit failure behavior when CoreAudio cannot create or
  activate the aggregate device.

## Acceptance criteria

- [x] On a MacBook route with different input/output device IDs, FreeWheeling
  opens one private aggregate device and reports one duplex callback path.
- [x] During a 60-second recording run, capture/playback frame counts remain
  matched and dropped, trimmed, missing, and fabricated input-frame counters
  remain zero.
- [x] Newly recorded loops contain no periodic buzzing or chirped gaps caused by
  callback starvation.
- [x] Startup latency calibration still reports a physical round-trip value and
  synced loops remain aligned using that value.
- [x] Closing FreeWheeling removes only the private aggregate it created.
- [x] Device changes and setup failures do not leave an aggregate device,
  AudioUnit, or modified device buffer size behind.
- [x] JACK behavior and non-macOS backends remain unchanged.

## Design decision

Do not solve this by increasing the existing ring-buffer size or replacing
missing samples with the last sample. Those approaches hide timing failures,
change latency unpredictably, and still corrupt loop boundaries. Do not use the
latency calibration offset as a correction for dropped frames. Establishing a
single clock domain first is the prerequisite for reliable recording; latency
calibration then handles only the remaining fixed physical delay.
