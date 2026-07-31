//! Low-latency macOS duplex AudioUnit backend.
//!
//! Unlike CPAL's portable backend, this owns one HAL Output AudioUnit.  Its
//! render callback calls `AudioUnitRender` to obtain capture samples and then
//! immediately invokes the DSP with the device's output buffers.  There is no
//! capture/playback queue and consequently no extra callback of monitor or
//! recording latency.
//!
//! When the selected input and output devices have different CoreAudio device
//! IDs (the ordinary MacBook mic/speaker route), the backend creates a private
//! CoreAudio Aggregate Device whose master clock is the output device and
//! whose input subdevice has drift compensation enabled. The HAL AudioUnit is
//! opened against that single aggregate, so capture and playback share one
//! clock domain and one duplex callback; no captured frame can be trimmed or
//! replaced by the backend. The aggregate is private to this process and is
//! destroyed on close, recovery, or drop.

use crate::audio_native_cpal::{
    AudioDeviceInfo, AudioLatency, CpalAudioOptions, CpalAudioStatus, CpalStreamDiagnostics,
    DeviceSelection,
};
use crate::audioio::{
    AudioBackend, AudioCallback, AudioCallbackFn, AudioMetrics, AudioRecoveryMetrics, BackendInfo,
    JackPosition, NFrames, NUM_CHANNELS,
};
use crate::realtime_guard::RealtimeMetrics;
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, TCFType};
use core_foundation::string::CFStringRef;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use coreaudio_sys::*;

// Keep AudioBufferList2 for non-interleaved stereo capture (coreaudio_sys
// AudioBufferList has exactly 1 trailing buffer).
#[allow(non_snake_case)]
#[repr(C)]
struct AudioBufferList2 {
    mNumberBuffers: u32,
    mBuffers: [AudioBuffer; NUM_CHANNELS],
}

// AudioToolbox/CoreAudio property and format constants. These FourCharCode
// values are stable ABI from macOS 10.0 onward; pulling them out of
// coreaudio_sys just avoids churn when the SDK bindings are regenerated.
const NO_ERR: i32 = 0;
const K_AUDIO_FORMAT_LINEAR_PCM: u32 = 0x6c70_636d; // 'lpcm'
const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1;
const K_AUDIO_FORMAT_FLAG_IS_PACKED: u32 = 1 << 3;
const K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED: u32 = 1 << 5;
const K_AUDIO_UNIT_TYPE_OUTPUT: u32 = 0x6175_6f75;
const K_AUDIO_UNIT_SUBTYPE_HAL_OUTPUT: u32 = 0x6168_616c;
const K_AUDIO_UNIT_MANUFACTURER_APPLE: u32 = 0x6170_706c;
const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE: u32 = 0x6449_6e20;
const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE: u32 = 0x644f_7574;
const K_AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE: u32 = 2000;
const K_AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO: u32 = 2003;
const K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT: u32 = 8;
const K_AUDIO_UNIT_PROPERTY_MAXIMUM_FRAMES_PER_SLICE: u32 = 14;
const K_AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK: u32 = 23;
const K_AUDIO_DEVICE_PROPERTY_NOMINAL_SAMPLE_RATE: u32 = 0x6e73_7274;
const K_AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE: u32 = 0x6673_697a;
const K_AUDIO_DEVICE_PROPERTY_LATENCY: u32 = 0x6c74_6e63;
const K_AUDIO_DEVICE_PROPERTY_SAFETY_OFFSET: u32 = 0x7361_6674;
const K_AUDIO_DEVICE_PROPERTY_DEVICE_UID: u32 = 0x7569_6420; // 'uid '
const K_AUDIO_AGGREGATE_PROPERTY_FULL_SUBDEVICE_LIST: u32 = 0x6772_7570; // 'grup'
// CoreFoundation keys of the aggregate-device composition dictionary. The
// values are the same strings CoreAudio defines in AudioHardware.h; the
// bindgen-generated constants are byte placeholders, not usable CFStringRefs.
const AGGREGATE_KEY_UID: &str = "uid";
const AGGREGATE_KEY_NAME: &str = "name";
const AGGREGATE_KEY_SUBDEVICES: &str = "subdevices";
const AGGREGATE_KEY_MASTER: &str = "master";
const AGGREGATE_KEY_PRIVATE: &str = "private";
const AGGREGATE_KEY_DRIFT: &str = "drift";
const AGGREGATE_NAME: &str = "FreeWheeling Aggregate";
const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
const K_AUDIO_OBJECT_SCOPE_GLOBAL: u32 = 0;
const K_AUDIO_OBJECT_ELEMENT_MAIN: u32 = 0;
const K_AUDIO_OBJECT_SCOPE_INPUT: u32 = 0x696e_7074;
const K_AUDIO_OBJECT_SCOPE_OUTPUT: u32 = 0x6f75_7470;
const K_AUDIO_UNIT_SCOPE_GLOBAL: u32 = 0;
const K_AUDIO_UNIT_SCOPE_INPUT: u32 = 1;
const K_AUDIO_UNIT_SCOPE_OUTPUT: u32 = 2;
const ROUTE_POLL_INTERVAL_MS: u64 = 250;
const DEFAULT_RATE: u32 = 48_000;
const MAX_CALLBACK_FRAMES: usize = 16_384;
#[derive(Default)]
struct SharedMetrics {
    xruns: AtomicU64,
    stream_errors: AtomicU64,
    callbacks: AtomicU64,
    callback_frames: AtomicU64,
    callback_peak_nanos: AtomicU64,
    callback_total_nanos: AtomicU64,
    recovery_requests: AtomicU64,
    #[allow(dead_code)]
    callback_panics: AtomicU64,
    active: AtomicBool,
    cpu_load_bits: AtomicU32,
    // Reliable-path frame diagnostics. The duplex callback captures and plays
    // the same frame count per callback, so capture_frames and playback_frames
    // must stay equal and the corruption counters must stay zero.
    capture_frames: AtomicU64,
    playback_frames: AtomicU64,
    trimmed_frames: AtomicU64,
    missing_frames: AtomicU64,
    frame_size_mismatches: AtomicU64,
}

impl SharedMetrics {
    fn cpu_load(&self) -> f32 {
        f32::from_bits(self.cpu_load_bits.load(Ordering::Acquire))
    }
    fn snapshot(&self) -> AudioMetrics {
        AudioMetrics {
            xruns: self.xruns.load(Ordering::Relaxed),
            stream_errors: self.stream_errors.load(Ordering::Relaxed),
            callbacks: self.callbacks.load(Ordering::Relaxed),
            callback_frames: self.callback_frames.load(Ordering::Relaxed),
            callback_peak_nanos: self.callback_peak_nanos.load(Ordering::Relaxed),
            callback_total_nanos: self.callback_total_nanos.load(Ordering::Relaxed),
            recovery_requests: self.recovery_requests.load(Ordering::Relaxed),
            ..AudioMetrics::default()
        }
    }
    fn stream_diagnostics(&self) -> CpalStreamDiagnostics {
        CpalStreamDiagnostics {
            capture_frames: self.capture_frames.load(Ordering::Relaxed),
            playback_frames: self.playback_frames.load(Ordering::Relaxed),
            max_queue_frames: 0,
            trimmed_frames: self.trimmed_frames.load(Ordering::Relaxed),
            missing_frames: self.missing_frames.load(Ordering::Relaxed),
            frame_size_mismatches: self.frame_size_mismatches.load(Ordering::Relaxed),
        }
    }
}

struct CallbackState {
    unit: AudioUnit,
    processor: Option<AudioCallbackFn>,
    input_left: Vec<f32>,
    input_right: Vec<f32>,
    scratch_right: Vec<f32>,
    capture: AudioBufferList2,
    sample_rate: u32,
    frame_position: u64,
    metrics: Arc<SharedMetrics>,
    realtime_metrics: Option<Arc<RealtimeMetrics>>,
    cpu_sample_count: u32,
    cpu_sample_frames: u64,
    cpu_sample_start: Option<Instant>,
}

#[derive(Clone, Copy)]
struct BufferRestore {
    device: u32,
    previous_frames: u32,
    applied_frames: u32,
}

/// Explicit CoreAudio route state owned by the backend.
///
/// `active_device` is the device the HAL AudioUnit is bound to: either a
/// single physical device (when input and output already share one device ID)
/// or the private aggregate created by this process. An aggregate is never
/// adopted from the system; `owned_aggregate_uid` is `Some` exactly when this
/// process created the active device and therefore must destroy it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CoreAudioRoute {
    requested_input: u32,
    requested_output: u32,
    active_device: u32,
    owned_aggregate_uid: Option<String>,
}

/// A private CoreAudio Aggregate Device created and owned by this process.
struct OwnedAggregate {
    device_id: u32,
    uid: String,
    input_id: u32,
    output_id: u32,
    drift_compensation: bool,
}

/// Monotonic per-process counter so every aggregate UID is unique even when
/// routes are recreated in the same process lifetime.
static AGGREGATE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

unsafe impl Send for CallbackState {}

impl CallbackState {
    fn new(
        sample_rate: u32,
        metrics: Arc<SharedMetrics>,
        realtime_metrics: Option<Arc<RealtimeMetrics>>,
    ) -> Self {
        let mut input_left = vec![0.0; MAX_CALLBACK_FRAMES];
        let mut input_right = vec![0.0; MAX_CALLBACK_FRAMES];
        let capture = AudioBufferList2 {
            mNumberBuffers: 2,
            mBuffers: [
                AudioBuffer {
                    mNumberChannels: 1,
                    mDataByteSize: 0,
                    mData: input_left.as_mut_ptr().cast(),
                },
                AudioBuffer {
                    mNumberChannels: 1,
                    mDataByteSize: 0,
                    mData: input_right.as_mut_ptr().cast(),
                },
            ],
        };
        Self {
            unit: ptr::null_mut(),
            processor: None,
            input_left,
            input_right,
            scratch_right: vec![0.0; MAX_CALLBACK_FRAMES],
            capture,
            sample_rate,
            frame_position: 0,
            metrics,
            realtime_metrics,
            cpu_sample_count: 0,
            cpu_sample_frames: 0,
            cpu_sample_start: None,
        }
    }
}

/// A macOS-only backend that mirrors the original C++ HAL AudioUnit path.
pub struct MacosAudioUnitBackend {
    options: CpalAudioOptions,
    unit: AudioUnit,
    state: Option<Box<CallbackState>>,
    info: Option<BackendInfo>,
    latency: Option<AudioLatency>,
    buffer_restores: Vec<BufferRestore>,
    route: Option<CoreAudioRoute>,
    metrics: Arc<SharedMetrics>,
    realtime_metrics: Option<Arc<RealtimeMetrics>>,
    route_poll_origin: Instant,
    route_poll_after_ms: AtomicU64,
}

// Safety: MacosAudioUnitBackend is created, used, and dropped on the audio
// thread only. It is never accessed concurrently. The Send impl allows it to
// be moved into the audio callback closure.
unsafe impl Send for MacosAudioUnitBackend {}

impl MacosAudioUnitBackend {
    pub fn new(_selection: DeviceSelection, options: CpalAudioOptions) -> Self {
        Self {
            options,
            unit: ptr::null_mut(),
            state: None,
            info: None,
            latency: None,
            buffer_restores: Vec::new(),
            route: None,
            metrics: Arc::new(SharedMetrics::default()),
            realtime_metrics: None,
            route_poll_origin: Instant::now(),
            route_poll_after_ms: AtomicU64::new(0),
        }
    }

    pub fn set_realtime_metrics(&mut self, metrics: Arc<RealtimeMetrics>) {
        // The callback state is built during open(), before the acceptance
        // runner attaches its instrumentation; update the live state as well
        // so instrumentation always sees the realtime callback.
        if let Some(state) = &mut self.state {
            state.realtime_metrics = Some(Arc::clone(&metrics));
        }
        self.realtime_metrics = Some(metrics);
    }

    pub fn status(&self) -> CpalAudioStatus {
        let route = self.route.as_ref();
        let input = route.map(|route| device_info(route.requested_input));
        let output = route.map(|route| device_info(route.requested_output));
        CpalAudioStatus {
            active: self.metrics.active.load(Ordering::Acquire),
            input,
            output,
            format: self.info,
            // One duplex callback does both jobs. Reporting it in both fields
            // keeps the existing diagnostic contract meaningful.
            capture_callbacks: self.metrics.callbacks.load(Ordering::Relaxed),
            playback_callbacks: self.metrics.callbacks.load(Ordering::Relaxed),
            latency: self.latency,
            metrics: self.metrics.snapshot(),
            stream: self.metrics.stream_diagnostics(),
        }
    }

    /// Verify both AudioUnit sides negotiated the expected non-interleaved
    /// stereo float format and buffer count before starting IO. A mismatch
    /// here means the device rejected the requested layout, so starting the
    /// callback would deliver corrupt or truncated audio.
    /// Verify both AudioUnit sides negotiated the expected non-interleaved
    /// stereo float format before starting IO. The HAL output unit exposes the
    /// playback side on the input scope (element 0) and the capture side on
    /// the output scope (element 1); those are the two (scope, element) pairs
    /// `configure` sets, so a mismatch here means the device rejected the
    /// requested layout and starting the callback would deliver corrupt or
    /// truncated audio.
    fn validate_duplex_format(&self) -> Result<(), String> {
        for (scope, element, side) in [
            (K_AUDIO_UNIT_SCOPE_INPUT, 0, "playback"),
            (K_AUDIO_UNIT_SCOPE_OUTPUT, 1, "capture"),
        ] {
            let format = audio_unit_format(self.unit, scope, element)?;
            let non_interleaved = format.mFormatFlags & K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED != 0;
            let float = format.mFormatFlags & K_AUDIO_FORMAT_FLAG_IS_FLOAT != 0;
            if format.mChannelsPerFrame < NUM_CHANNELS as u32 || !non_interleaved || !float {
                return Err(format!(
                    "CoreAudio {side} format is not non-interleaved stereo float \
                     (channels={}, flags=0x{:x}); refusing to start the duplex callback",
                    format.mChannelsPerFrame, format.mFormatFlags
                ));
            }
        }
        Ok(())
    }

    fn requested_frames(&self) -> u32 {
        std::env::var("FWEELIN_AUDIO_BUFFER_FRAMES")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|frames| *frames > 0)
            .unwrap_or(self.options.preferred_buffer_frames.max(1))
    }

    fn take_callback(&mut self) -> Option<AudioCallbackFn> {
        self.state.as_mut().and_then(|state| state.processor.take())
    }

    /// Open the HAL AudioUnit against the route's active device (the private
    /// aggregate when one was created, otherwise the single physical device).
    ///
    /// The aggregate is created by `open()` before this runs, so every failure
    /// path here unwinds through `dispose_unit` + `destroy_owned_aggregate` and
    /// never leaves an owned aggregate or a running AudioUnit behind.
    fn configure(&mut self) -> Result<BackendInfo, String> {
        let (active, aggregate_path, requested_input, requested_output) = self
            .route
            .as_ref()
            .map(|route| {
                (
                    route.active_device,
                    route.owned_aggregate_uid.is_some(),
                    route.requested_input,
                    route.requested_output,
                )
            })
            .ok_or("audio route must be resolved before configuring the AudioUnit")?;
        let desc = AudioComponentDescription {
            componentType: K_AUDIO_UNIT_TYPE_OUTPUT,
            componentSubType: K_AUDIO_UNIT_SUBTYPE_HAL_OUTPUT,
            componentManufacturer: K_AUDIO_UNIT_MANUFACTURER_APPLE,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        // SAFETY: CoreAudio takes the description only for this call and writes
        // a fresh AudioUnit instance into `unit`.
        let component = unsafe { AudioComponentFindNext(ptr::null_mut(), &desc) };
        if component.is_null() {
            return Err("cannot find macOS HAL audio unit".into());
        }
        let mut unit = ptr::null_mut();
        check(
            unsafe { AudioComponentInstanceNew(component, &mut unit) },
            "create HAL audio unit",
        )?;
        self.unit = unit;

        let setup = (|| {
            set_property(
                unit,
                K_AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
                K_AUDIO_UNIT_SCOPE_INPUT,
                1,
                &1u32,
                "enable input",
            )?;
            set_property(
                unit,
                K_AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO,
                K_AUDIO_UNIT_SCOPE_OUTPUT,
                0,
                &1u32,
                "enable output",
            )?;
            set_property(
                unit,
                K_AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE,
                K_AUDIO_UNIT_SCOPE_GLOBAL,
                0,
                &active,
                "select duplex audio device",
            )?;
            let rate = nominal_rate(active)
                .unwrap_or(DEFAULT_RATE as f64)
                .round()
                .max(1.0) as u32;
            let format = pcm_format(rate);
            set_property(
                unit,
                K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
                K_AUDIO_UNIT_SCOPE_INPUT,
                0,
                &format,
                "set output stream format",
            )?;
            set_property(
                unit,
                K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
                K_AUDIO_UNIT_SCOPE_OUTPUT,
                1,
                &format,
                "set input stream format",
            )?;
            let mut state = Box::new(CallbackState::new(
                rate,
                Arc::clone(&self.metrics),
                self.realtime_metrics.clone(),
            ));
            state.unit = unit;
            let callback = AURenderCallbackStruct {
                inputProc: Some(render_callback),
                inputProcRefCon: (&mut *state as *mut CallbackState).cast(),
            };
            set_property(
                unit,
                K_AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK,
                K_AUDIO_UNIT_SCOPE_INPUT,
                0,
                &callback,
                "set duplex render callback",
            )?;
            let frames = if aggregate_path {
                // An aggregate device only honours its buffer-frame-size
                // property once its IO proc is attached, which happens when
                // the AudioUnit is initialized. Negotiate after that point and
                // always report the size the device actually returned.
                check(
                    unsafe { AudioUnitInitialize(unit) },
                    "initialize HAL audio unit",
                )?;
                let (_, actual) = set_low_latency_buffer(active, self.requested_frames())?;
                actual
            } else {
                let (previous_frames, actual) =
                    set_low_latency_buffer(active, self.requested_frames())?;
                if previous_frames != actual {
                    self.buffer_restores.push(BufferRestore {
                        device: active,
                        previous_frames,
                        applied_frames: actual,
                    });
                }
                check(
                    unsafe { AudioUnitInitialize(unit) },
                    "initialize HAL audio unit",
                )?;
                actual
            };
            let max_frames = (MAX_CALLBACK_FRAMES as u32).max(frames);
            set_property(
                unit,
                K_AUDIO_UNIT_PROPERTY_MAXIMUM_FRAMES_PER_SLICE,
                K_AUDIO_UNIT_SCOPE_GLOBAL,
                0,
                &max_frames,
                "set maximum frames per slice",
            )?;
            self.latency = latency_estimate(requested_input, requested_output, frames, 0);
            self.state = Some(state);
            Ok(BackendInfo {
                sample_rate: rate,
                buffer_size: frames,
            })
        })();
        if setup.is_err() {
            self.dispose_unit();
            self.restore_device_buffers();
            self.destroy_owned_aggregate();
        }
        setup
    }

    /// Stop, uninitialize, and dispose the HAL AudioUnit. The owned aggregate
    /// (if any) is intentionally left alive so the caller controls the exact
    /// teardown order; `destroy_owned_aggregate` runs after this.
    fn dispose_unit(&mut self) {
        if !self.unit.is_null() {
            // SAFETY: only called by the owner after a successful stop (or
            // during failed setup before the unit can run).
            unsafe {
                let _ = AudioOutputUnitStop(self.unit);
                let _ = AudioUnitUninitialize(self.unit);
                let _ = AudioComponentInstanceDispose(self.unit);
            }
            self.unit = ptr::null_mut();
        }
        self.metrics.active.store(false, Ordering::Release);
        self.info = None;
        self.latency = None;
    }

    /// Device ID of the private aggregate currently owned by this backend, if
    /// any. Exposed for acceptance instrumentation that verifies the device is
    /// gone after close.
    pub fn owned_aggregate_device(&self) -> Option<u32> {
        self.route
            .as_ref()
            .filter(|route| route.owned_aggregate_uid.is_some())
            .map(|route| route.active_device)
    }

    /// Claim the device that must be destroyed for the current route. The
    /// `take` makes destruction exactly-once: a second call returns `None`.
    /// Routes whose active device was discovered (never created by this
    /// process) return `None` and are never destroyed.
    fn take_owned_aggregate_device(&mut self) -> Option<u32> {
        let route = self.route.as_mut()?;
        route
            .owned_aggregate_uid
            .take()
            .map(|_| route.active_device)
    }

    /// Destroy the private aggregate owned by this process, if any, and clear
    /// the route. Runs on close, recovery, drop, and every setup failure after
    /// aggregate creation. Never touches a device FreeWheeling discovered.
    fn destroy_owned_aggregate(&mut self) {
        if let Some(device) = self.take_owned_aggregate_device() {
            match destroy_aggregate_device(device) {
                Ok(()) => {
                    eprintln!("[AUDIO-DIAG] audio: destroyed private aggregate device {device}")
                }
                Err(error) => eprintln!("[AUDIO-DIAG] audio: {error}"),
            }
        }
        self.route = None;
    }

    fn restore_device_buffers(&mut self) {
        for restore in self.buffer_restores.drain(..) {
            // Do not overwrite a change made by the user or another app after
            // FreeWheeling opened its streams.
            if device_u32(
                restore.device,
                K_AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE,
                K_AUDIO_OBJECT_SCOPE_GLOBAL,
            )
            .is_ok_and(|current| current == restore.applied_frames)
            {
                let _ = set_device_u32(
                    restore.device,
                    K_AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE,
                    K_AUDIO_OBJECT_SCOPE_GLOBAL,
                    restore.previous_frames,
                );
            }
        }
    }

    fn direct_route_changed(&self) -> bool {
        let (Some(route), Some(info)) = (self.route.as_ref(), self.info) else {
            return false;
        };
        let elapsed_ms = self.route_poll_origin.elapsed().as_millis() as u64;
        let due = self.route_poll_after_ms.load(Ordering::Acquire);
        if elapsed_ms < due
            || self
                .route_poll_after_ms
                .compare_exchange(
                    due,
                    elapsed_ms.saturating_add(ROUTE_POLL_INTERVAL_MS),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
        {
            return false;
        }
        let defaults_changed = !matches!(
            default_device(K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE),
            Ok(current) if current == route.requested_input
        ) || !matches!(
            default_device(K_AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE),
            Ok(current) if current == route.requested_output
        );
        let device_format_changed = nominal_rate(route.active_device)
            .map(|rate| rate.round() as u32 != info.sample_rate)
            .unwrap_or(true)
            || device_u32(
                route.active_device,
                K_AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE,
                K_AUDIO_OBJECT_SCOPE_GLOBAL,
            )
            .map(|frames| frames != info.buffer_size)
            .unwrap_or(true);
        defaults_changed || device_format_changed
    }
}

impl AudioBackend for MacosAudioUnitBackend {
    fn open(&mut self, _client_name: &str) -> Result<BackendInfo, String> {
        self.close();
        let input = default_device(K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE)?;
        let output = default_device(K_AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE)?;
        // When input and output are different devices (the ordinary MacBook
        // mic/speaker route), a private aggregate is required: a HAL Output
        // unit cannot bind one physical device for output and another for
        // capture, and the split-stream alternative corrupts recorded frames.
        let aggregate = if input != output {
            eprintln!(
                "[AUDIO-DIAG] audio: input device {input} and output device {output} differ; \
                 creating a private CoreAudio Aggregate Device"
            );
            let aggregate = OwnedAggregate::create(input, output)?;
            eprintln!(
                "[AUDIO-DIAG] audio: aggregate uid={} device={} input={} output={} \
                 master=output drift_compensation={}",
                aggregate.uid,
                aggregate.device_id,
                aggregate.input_id,
                aggregate.output_id,
                aggregate.drift_compensation,
            );
            Some(aggregate)
        } else {
            None
        };
        self.route = Some(CoreAudioRoute {
            requested_input: input,
            requested_output: output,
            active_device: aggregate
                .as_ref()
                .map_or(output, |aggregate| aggregate.device_id),
            owned_aggregate_uid: aggregate.map(|aggregate| aggregate.uid),
        });
        // configure unwinds in reverse order on failure (dispose AudioUnit,
        // restore device buffer sizes, destroy the owned aggregate); nothing
        // is left behind on a failed open.
        let info = self.configure()?;
        self.info = Some(info);
        Ok(info)
    }

    fn activate(&mut self, callback: AudioCallbackFn) -> Result<(), String> {
        self.validate_duplex_format()?;
        let state = self
            .state
            .as_mut()
            .ok_or("audio backend must be opened before activation")?;
        if state.processor.is_some() {
            return Err("audio backend is already active".into());
        }
        state.processor = Some(callback);
        if let Err(error) = check(
            unsafe { AudioOutputUnitStart(self.unit) },
            "start HAL audio unit",
        ) {
            state.processor = None;
            return Err(error);
        }
        self.metrics.active.store(true, Ordering::Release);
        Ok(())
    }

    fn close(&mut self) {
        self.dispose_unit();
        // SAFETY: AudioOutputUnitStop inside dispose_unit is synchronous
        // (Apple docs: "does not return until the unit has stopped"), so
        // no render callback accesses state after this point.
        self.state = None;
        self.restore_device_buffers();
        self.destroy_owned_aggregate();
    }

    fn relocate(&mut self, _frame: NFrames) {}

    fn metrics(&self) -> AudioMetrics {
        self.metrics.snapshot()
    }

    fn input_latency_frames(&self) -> NFrames {
        self.latency
            .map(|latency| {
                latency
                    .input_device_frames
                    .saturating_add(latency.software_queue_frames)
            })
            .unwrap_or(0)
    }

    fn cpu_load(&self) -> Option<f32> {
        Some(self.metrics.cpu_load())
    }

    fn recovery_requested(&self) -> bool {
        if self.direct_route_changed() {
            self.metrics
                .recovery_requests
                .fetch_add(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn recover(&mut self) -> Result<BackendInfo, String> {
        // Recovery closes the old AudioUnit, destroys the old owned aggregate,
        // creates a fresh aggregate for the current route, and reopens the
        // duplex callback with the retained processor.
        eprintln!("[AUDIO-DIAG] audio: route changed; recreating audio route");
        let callback = self
            .take_callback()
            .ok_or("audio callback is unavailable for recovery")?;
        self.close();
        let info = self.open("FreeWheeling")?;
        self.activate(callback).map(|()| info)
    }

    fn recovery_metrics(&self) -> AudioRecoveryMetrics {
        AudioRecoveryMetrics::default()
    }
}

impl Drop for MacosAudioUnitBackend {
    fn drop(&mut self) {
        self.close();
    }
}

unsafe extern "C" fn render_callback(
    ref_con: *mut c_void,
    flags: *mut u32,
    timestamp: *const AudioTimeStamp,
    _bus: u32,
    frames: u32,
    io_data: *mut AudioBufferList,
) -> i32 {
    if ref_con.is_null()
        || timestamp.is_null()
        || io_data.is_null()
        || frames as usize > MAX_CALLBACK_FRAMES
    {
        zero_output(io_data);
        return -1;
    }
    // SAFETY: `ref_con` is a Box<CallbackState> held by the backend until the
    // AudioUnit has stopped; this callback is its sole mutable realtime user.
    let state = unsafe { &mut *ref_con.cast::<CallbackState>() };
    let _guard = state
        .realtime_metrics
        .as_ref()
        .map(|metrics| metrics.enter_callback());
    let started = Instant::now();
    if state.cpu_sample_count == 0 {
        state.cpu_sample_start = Some(started);
    }
    let count = frames as usize;
    state.capture.mBuffers[0].mDataByteSize = frames * std::mem::size_of::<f32>() as u32;
    state.capture.mBuffers[1].mDataByteSize = frames * std::mem::size_of::<f32>() as u32;
    // SAFETY: all pointers are valid for the duration of the callback and the
    // capture list points at preallocated buffers with MAX_CALLBACK_FRAMES.
    if unsafe {
        AudioUnitRender(
            state.unit,
            flags,
            timestamp,
            1,
            frames,
            (&mut state.capture as *mut AudioBufferList2).cast(),
        )
    } != NO_ERR
    {
        // A failed render leaves no capture frames for this callback. The
        // zero fill below would fabricate input, so count it as missing and
        // let diagnostics expose the failure instead of hiding it.
        state.input_left[..count].fill(0.0);
        state.input_right[..count].fill(0.0);
        state.metrics.stream_errors.fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .missing_frames
            .fetch_add(frames as u64, Ordering::Relaxed);
    }
    state
        .metrics
        .capture_frames
        .fetch_add(frames as u64, Ordering::Relaxed);
    state
        .metrics
        .playback_frames
        .fetch_add(frames as u64, Ordering::Relaxed);
    // Reconstruct `state` inside the closure from the raw pointer ref_con
    // rather than capturing &mut CallbackState, so catch_unwind compiles
    // without AssertUnwindSafe.  Panics in an audio callback corrupt the
    // processor state and cannot be recovered safely.
    let result = std::panic::catch_unwind(|| {
        // SAFETY: ref_con is a Box<CallbackState> owned by the backend; the
        // AudioUnit guarantees this callback is the sole caller.
        let state = unsafe { &mut *ref_con.cast::<CallbackState>() };
        // SAFETY: HAL is configured for non-interleaved f32 stereo. The
        // callback's `frames` cannot exceed either supplied buffer size.
        let output = unsafe { &mut *io_data };
        if output.mNumberBuffers < NUM_CHANNELS as u32 {
            state
                .metrics
                .frame_size_mismatches
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        let left_buffer = unsafe { buffer_at(io_data, 0) };
        let left = left_buffer.mData.cast::<f32>();
        if left.is_null() || left_buffer.mDataByteSize < frames * 4 {
            state
                .metrics
                .frame_size_mismatches
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        // SAFETY: left was validated above (non-null, sufficient size).
        let left = unsafe { std::slice::from_raw_parts_mut(left, count) };
        let right = {
            let right_buffer = unsafe { buffer_at(io_data, 1) };
            let pointer = right_buffer.mData.cast::<f32>();
            if pointer.is_null() || right_buffer.mDataByteSize < frames * 4 {
                state
                    .metrics
                    .frame_size_mismatches
                    .fetch_add(1, Ordering::Relaxed);
                None
            } else {
                // SAFETY: pointer was validated above.
                Some(unsafe { std::slice::from_raw_parts_mut(pointer, count) })
            }
        };
        state.scratch_right[..count].fill(0.0);
        left.fill(0.0);
        let mut audio = AudioCallback {
            inputs: [&state.input_left[..count], &state.input_right[..count]],
            outputs: [left, &mut state.scratch_right[..count]],
            nframes: frames,
            position: JackPosition {
                frame: state.frame_position.min(u32::MAX as u64) as u32,
                frame_rate: state.sample_rate,
                ..JackPosition::default()
            },
            transport_rolling: false,
        };
        if let Some(processor) = state.processor.as_mut() {
            processor(&mut audio);
        }
        if let Some(right) = right {
            right.copy_from_slice(&state.scratch_right[..count]);
        }
    });
    if let Err(_panic) = result {
        zero_output(io_data);
        // State may be corrupted after a panic; abort rather than continue
        // with undefined behaviour on the next callback invocation.
        std::process::abort();
    }
    state.frame_position = state.frame_position.wrapping_add(frames as u64);
    let elapsed = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    state.metrics.callbacks.fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .callback_frames
        .fetch_add(frames as u64, Ordering::Relaxed);
    state
        .metrics
        .callback_total_nanos
        .fetch_add(elapsed, Ordering::Relaxed);
    state
        .metrics
        .callback_peak_nanos
        .fetch_max(elapsed, Ordering::Relaxed);
    state.cpu_sample_count += 1;
    state.cpu_sample_frames += frames as u64;
    if state.cpu_sample_count >= 16 {
        let elapsed = state
            .cpu_sample_start
            .take()
            .expect("CPU window starts with its first callback")
            .elapsed()
            .as_nanos() as f64;
        let period =
            state.cpu_sample_frames as f64 / state.sample_rate.max(1) as f64 * 1_000_000_000.0;
        if period > 0.0 {
            state.metrics.cpu_load_bits.store(
                ((elapsed / period).clamp(0.0, f32::MAX as f64) as f32).to_bits(),
                Ordering::Release,
            );
        }
        state.cpu_sample_count = 0;
        state.cpu_sample_frames = 0;
    }
    NO_ERR
}

fn zero_output(io_data: *mut AudioBufferList) {
    if io_data.is_null() {
        return;
    }
    // SAFETY: CoreAudio owns `io_data`; only clear the buffers it reports.
    unsafe {
        let output = &mut *io_data;
        for index in 0..output.mNumberBuffers.min(2) as usize {
            let buffer = buffer_at(io_data, index);
            if !buffer.mData.is_null() {
                ptr::write_bytes(buffer.mData, 0, buffer.mDataByteSize as usize);
            }
        }
    }
}

/// # Safety
/// `list` must point to a CoreAudio AudioBufferList containing `index + 1`
/// buffers. The caller checks `number_buffers` before requesting an index.
unsafe fn buffer_at<'a>(list: *mut AudioBufferList, index: usize) -> &'a mut AudioBuffer {
    // SAFETY: caller guarantees `index < list.mNumberBuffers`.
    // `mBuffers` is declared as `[AudioBuffer; 1]` but CoreAudio allocates
    // the list with `mNumberBuffers` AudioBuffer entries.  Using array
    // indexing (`(*list).mBuffers[index]`) for index > 0 would read past
    // the declared bound — that is UB even when the allocation is larger.
    // Use pointer arithmetic from the base of `mBuffers` instead.
    unsafe {
        let base = std::ptr::addr_of_mut!((*list).mBuffers) as *mut AudioBuffer;
        &mut *base.add(index)
    }
}

fn device_info(id: u32) -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: id.to_string(),
        name: format!("CoreAudio device {id}"),
        is_default: true,
    }
}

fn check(status: i32, operation: &str) -> Result<(), String> {
    (status == NO_ERR)
        .then_some(())
        .ok_or_else(|| format!("CoreAudio: cannot {operation} (OSStatus {status})"))
}

fn set_property<T>(
    unit: AudioUnit,
    id: u32,
    scope: u32,
    element: u32,
    value: &T,
    operation: &str,
) -> Result<(), String> {
    // SAFETY: CoreAudio reads exactly `size_of::<T>()` bytes synchronously.
    check(
        unsafe {
            AudioUnitSetProperty(
                unit,
                id,
                scope,
                element,
                (value as *const T).cast(),
                std::mem::size_of::<T>() as u32,
            )
        },
        operation,
    )
}

fn audio_unit_format(
    unit: AudioUnit,
    scope: u32,
    element: u32,
) -> Result<AudioStreamBasicDescription, String> {
    let mut format = AudioStreamBasicDescription::default();
    let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
    // SAFETY: output pointer points to initialized local storage.
    check(
        unsafe {
            AudioUnitGetProperty(
                unit,
                K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT,
                scope,
                element,
                (&mut format as *mut AudioStreamBasicDescription).cast(),
                &mut size,
            )
        },
        "query audio unit stream format",
    )?;
    Ok(format)
}

fn default_device(selector: u32) -> Result<u32, String> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: K_AUDIO_OBJECT_SCOPE_GLOBAL,
        mElement: K_AUDIO_OBJECT_ELEMENT_MAIN,
    };
    let mut device = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: output pointers point to initialized local storage.
    check(
        unsafe {
            AudioObjectGetPropertyData(
                K_AUDIO_OBJECT_SYSTEM_OBJECT,
                &address,
                0,
                ptr::null(),
                &mut size,
                (&mut device as *mut u32).cast(),
            )
        },
        "query default audio device",
    )?;
    (device != 0)
        .then_some(device)
        .ok_or("CoreAudio returned no default audio device".into())
}

fn nominal_rate(device: u32) -> Result<f64, String> {
    let address = AudioObjectPropertyAddress {
        mSelector: K_AUDIO_DEVICE_PROPERTY_NOMINAL_SAMPLE_RATE,
        mScope: K_AUDIO_OBJECT_SCOPE_GLOBAL,
        mElement: K_AUDIO_OBJECT_ELEMENT_MAIN,
    };
    let mut rate = DEFAULT_RATE as f64;
    let mut size = std::mem::size_of::<f64>() as u32;
    // SAFETY: output pointers point to initialized local storage.
    check(
        unsafe {
            AudioObjectGetPropertyData(
                device,
                &address,
                0,
                ptr::null(),
                &mut size,
                (&mut rate as *mut f64).cast(),
            )
        },
        "query device sample rate",
    )?;
    (rate > 0.0)
        .then_some(rate)
        .ok_or("CoreAudio returned an invalid sample rate".into())
}

fn set_low_latency_buffer(device: u32, requested: u32) -> Result<(u32, u32), String> {
    let previous = device_u32(
        device,
        K_AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE,
        K_AUDIO_OBJECT_SCOPE_GLOBAL,
    )?;
    for candidate in [requested, 64, 128, 256, 512] {
        if candidate == 0 {
            continue;
        }
        if set_device_u32(
            device,
            K_AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE,
            K_AUDIO_OBJECT_SCOPE_GLOBAL,
            candidate,
        )
        .is_ok()
        {
            break;
        }
    }
    let actual = device_u32(
        device,
        K_AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE,
        K_AUDIO_OBJECT_SCOPE_GLOBAL,
    )?;
    (actual > 0)
        .then_some((previous, actual))
        .ok_or("CoreAudio returned an invalid device buffer size".into())
}

fn device_u32(device: u32, selector: u32, scope: u32) -> Result<u32, String> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: K_AUDIO_OBJECT_ELEMENT_MAIN,
    };
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: output pointers point to initialized local storage.
    check(
        unsafe {
            AudioObjectGetPropertyData(
                device,
                &address,
                0,
                ptr::null(),
                &mut size,
                (&mut value as *mut u32).cast(),
            )
        },
        "query audio device property",
    )?;
    Ok(value)
}

fn set_device_u32(device: u32, selector: u32, scope: u32, value: u32) -> Result<(), String> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: K_AUDIO_OBJECT_ELEMENT_MAIN,
    };
    // SAFETY: CoreAudio consumes the scalar synchronously.
    check(
        unsafe {
            AudioObjectSetPropertyData(
                device,
                &address,
                0,
                ptr::null(),
                std::mem::size_of::<u32>() as u32,
                (&value as *const u32).cast(),
            )
        },
        "set audio device property",
    )
}

/// Read the device's `kAudioDevicePropertyDeviceUID` as a Rust string.
fn device_uid(device: u32) -> Result<String, String> {
    let address = AudioObjectPropertyAddress {
        mSelector: K_AUDIO_DEVICE_PROPERTY_DEVICE_UID,
        mScope: K_AUDIO_OBJECT_SCOPE_GLOBAL,
        mElement: K_AUDIO_OBJECT_ELEMENT_MAIN,
    };
    let mut size = 0u32;
    // SAFETY: first call with a null data pointer only requests the size.
    check(
        unsafe { AudioObjectGetPropertyDataSize(device, &address, 0, ptr::null(), &mut size) },
        "query device UID size",
    )?;
    let mut bytes = vec![0u8; size as usize];
    // SAFETY: the buffer matches the size CoreAudio reported.
    check(
        unsafe {
            AudioObjectGetPropertyData(
                device,
                &address,
                0,
                ptr::null(),
                &mut size,
                bytes.as_mut_ptr().cast(),
            )
        },
        "query device UID",
    )?;
    // The buffer holds the CFStringRef *value*; dereference it rather than
    // passing the buffer address. The returned object is owned by this call
    // (Create Rule); wrapping without retaining matches that ownership.
    let string = unsafe {
        let reference: CFStringRef = ptr::read_unaligned(bytes.as_ptr().cast());
        CFString::wrap_under_create_rule(reference)
    };
    let uid = string.to_string();
    if uid.is_empty() {
        return Err(format!("CoreAudio device {device} has no UID"));
    }
    Ok(uid)
}

/// Build the Core Foundation composition dictionary for a private aggregate
/// device. Pure builder, kept testable without touching the HAL.
///
/// - `uid` is a unique, process-scoped aggregate UID;
/// - `private = 1` keeps the device out of the user's global device list;
/// - the output subdevice is the master clock;
/// - the input subdevice has drift compensation enabled.
fn aggregate_description(uid: &str, input_uid: &str, output_uid: &str) -> CFDictionary {
    let input_subdevice = CFDictionary::<CFString, CFType>::from_CFType_pairs(&[
        (
            CFString::new(AGGREGATE_KEY_UID),
            CFString::new(input_uid).into_CFType(),
        ),
        (
            CFString::new(AGGREGATE_KEY_DRIFT),
            CFNumber::from(1i32).into_CFType(),
        ),
    ]);
    let output_subdevice = CFDictionary::<CFString, CFType>::from_CFType_pairs(&[(
        CFString::new(AGGREGATE_KEY_UID),
        CFString::new(output_uid).into_CFType(),
    )]);
    let subdevices = CFArray::<CFType>::from_CFTypes(&[
        input_subdevice.into_CFType(),
        output_subdevice.into_CFType(),
    ]);
    CFDictionary::<CFString, CFType>::from_CFType_pairs(&[
        (
            CFString::new(AGGREGATE_KEY_UID),
            CFString::new(uid).into_CFType(),
        ),
        (
            CFString::new(AGGREGATE_KEY_NAME),
            CFString::new(AGGREGATE_NAME).into_CFType(),
        ),
        (
            CFString::new(AGGREGATE_KEY_SUBDEVICES),
            subdevices.into_CFType(),
        ),
        (
            CFString::new(AGGREGATE_KEY_MASTER),
            CFString::new(output_uid).into_CFType(),
        ),
        (
            CFString::new(AGGREGATE_KEY_PRIVATE),
            CFNumber::from(1i32).into_CFType(),
        ),
    ])
    .into_untyped()
}

/// A unique, process-scoped aggregate UID. Includes the process ID and a
/// monotonic per-process counter so routes recreated in one process lifetime
/// never collide with each other or with devices another process created.
fn aggregate_uid(input: u32, output: u32) -> String {
    let sequence = AGGREGATE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "org.freewheeling.aggregate.{}.{sequence:x}.{input:x}.{output:x}",
        std::process::id()
    )
}

impl OwnedAggregate {
    /// Create a private aggregate device over `input` and `output`, with the
    /// output device as master clock and drift compensation on the input.
    fn create(input: u32, output: u32) -> Result<Self, String> {
        let input_uid = device_uid(input)?;
        let output_uid = device_uid(output)?;
        let uid = aggregate_uid(input, output);
        let description = aggregate_description(&uid, &input_uid, &output_uid);
        let mut device_id = 0u32;
        check(
            unsafe {
                AudioHardwareCreateAggregateDevice(
                    description.as_CFTypeRef().cast(),
                    &mut device_id,
                )
            },
            &format!("create private aggregate device {uid}"),
        )?;
        let aggregate = Self {
            device_id,
            uid,
            input_id: input,
            output_id: output,
            drift_compensation: true,
        };
        if let Err(error) = aggregate.verify(&input_uid, &output_uid) {
            // Never hand a half-verified aggregate to the backend: destroy it
            // immediately so a verification failure cannot leak a private
            // device into the session.
            let _ = destroy_aggregate_device(device_id);
            return Err(error);
        }
        Ok(aggregate)
    }

    /// Confirm the created aggregate actually contains both requested
    /// subdevices before any AudioUnit opens it.
    fn verify(&self, input_uid: &str, output_uid: &str) -> Result<(), String> {
        let address = AudioObjectPropertyAddress {
            mSelector: K_AUDIO_AGGREGATE_PROPERTY_FULL_SUBDEVICE_LIST,
            mScope: K_AUDIO_OBJECT_SCOPE_GLOBAL,
            mElement: K_AUDIO_OBJECT_ELEMENT_MAIN,
        };
        let mut size = 0u32;
        // SAFETY: first call with a null data pointer only requests the size.
        check(
            unsafe {
                AudioObjectGetPropertyDataSize(self.device_id, &address, 0, ptr::null(), &mut size)
            },
            "query aggregate subdevice list size",
        )?;
        let mut bytes = vec![0u8; size as usize];
        // SAFETY: the buffer matches the size CoreAudio reported.
        check(
            unsafe {
                AudioObjectGetPropertyData(
                    self.device_id,
                    &address,
                    0,
                    ptr::null(),
                    &mut size,
                    bytes.as_mut_ptr().cast(),
                )
            },
            "query aggregate subdevice list",
        )?;
        // The buffer holds the CFArrayRef *value*; dereference it rather than
        // passing the buffer address. The returned CFArray of CFStrings is
        // owned by this call (Create Rule); wrapping without retaining
        // transfers that ownership to the wrapper, which releases it on drop.
        let subdevices = unsafe {
            let reference: CFArrayRef = ptr::read_unaligned(bytes.as_ptr().cast());
            CFArray::<CFType>::wrap_under_create_rule(reference)
        };
        let mut found_input = false;
        let mut found_output = false;
        for index in 0..subdevices.len() {
            let Some(item) = subdevices.get(index) else {
                continue;
            };
            // SAFETY: the full sub-device list is documented as CFStrings.
            // Get rule: the element is owned by the array, so retain-on-take
            // with release-on-drop balances to a no-op instead of releasing
            // the array's own reference.
            let subdevice = unsafe { CFString::wrap_under_get_rule(item.as_CFTypeRef().cast()) };
            let uid = subdevice.to_string();
            found_input |= uid == input_uid;
            found_output |= uid == output_uid;
        }
        if !found_input || !found_output {
            return Err(format!(
                "CoreAudio aggregate {0} ({1}) is missing requested subdevices \
                 input={2} ({input_uid}) output={3} ({output_uid})",
                self.device_id, self.uid, self.input_id, self.output_id
            ));
        }
        Ok(())
    }
}

/// Destroy an aggregate device previously created by this process. The
/// destruction is asynchronous inside the HAL; a failed destroy is reported
/// loudly because the private device would otherwise leak into the session.
fn destroy_aggregate_device(device: u32) -> Result<(), String> {
    check(
        unsafe { AudioHardwareDestroyAggregateDevice(device) },
        &format!("destroy private aggregate device {device}"),
    )
}

fn latency_estimate(
    input: u32,
    output: u32,
    buffer_frames: u32,
    software_queue_frames: u32,
) -> Option<AudioLatency> {
    let input_device_frames = buffer_frames
        .checked_add(
            device_u32(
                input,
                K_AUDIO_DEVICE_PROPERTY_LATENCY,
                K_AUDIO_OBJECT_SCOPE_INPUT,
            )
            .ok()?,
        )?
        .checked_add(
            device_u32(
                input,
                K_AUDIO_DEVICE_PROPERTY_SAFETY_OFFSET,
                K_AUDIO_OBJECT_SCOPE_INPUT,
            )
            .ok()?,
        )?;
    let output_device_frames = buffer_frames
        .checked_add(
            device_u32(
                output,
                K_AUDIO_DEVICE_PROPERTY_LATENCY,
                K_AUDIO_OBJECT_SCOPE_OUTPUT,
            )
            .ok()?,
        )?
        .checked_add(
            device_u32(
                output,
                K_AUDIO_DEVICE_PROPERTY_SAFETY_OFFSET,
                K_AUDIO_OBJECT_SCOPE_OUTPUT,
            )
            .ok()?,
        )?;
    Some(AudioLatency {
        input_device_frames,
        output_device_frames,
        software_queue_frames,
        estimated_round_trip_frames: input_device_frames
            .saturating_add(output_device_frames)
            .saturating_add(software_queue_frames),
    })
}

fn pcm_format(rate: u32) -> AudioStreamBasicDescription {
    AudioStreamBasicDescription {
        mSampleRate: rate as f64,
        mFormatID: K_AUDIO_FORMAT_LINEAR_PCM,
        mFormatFlags: K_AUDIO_FORMAT_FLAG_IS_FLOAT
            | K_AUDIO_FORMAT_FLAG_IS_PACKED
            | K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED,
        mBytesPerPacket: 4,
        mFramesPerPacket: 1,
        mBytesPerFrame: 4,
        mChannelsPerFrame: 2,
        mBitsPerChannel: 32,
        mReserved: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::base::{CFIndex, CFTypeRef};

    fn dict_value(dict: &CFDictionary, key: &str) -> Option<CFTypeRef> {
        let key = CFString::new(key);
        dict.find(key.as_CFTypeRef()).map(|value| *value)
    }

    fn dict_string(dict: &CFDictionary, key: &str) -> Option<String> {
        let raw = dict_value(dict, key)?;
        // SAFETY: the dictionary value is a CFString for the tested keys.
        let string = unsafe { CFString::wrap_under_get_rule(raw.cast()) };
        Some(string.to_string())
    }

    fn dict_number(dict: &CFDictionary, key: &str) -> Option<i32> {
        let raw = dict_value(dict, key)?;
        // SAFETY: the dictionary value is a CFNumber for the tested keys.
        let number = unsafe { CFNumber::wrap_under_get_rule(raw.cast()) };
        number.to_i32()
    }

    fn dict_array(dict: &CFDictionary, key: &str) -> Option<CFArray<CFType>> {
        let raw = dict_value(dict, key)?;
        // SAFETY: the dictionary value is a CFArray for the tested keys.
        Some(unsafe { CFArray::wrap_under_get_rule(raw.cast()) })
    }

    fn array_subdict(array: &CFArray<CFType>, index: usize) -> CFDictionary {
        let item = array.get(index as CFIndex).expect("index in range");
        // SAFETY: the array holds CFDictionary values for the tested index.
        unsafe { CFDictionary::wrap_under_get_rule(item.as_CFTypeRef().cast()) }
    }

    fn test_backend() -> MacosAudioUnitBackend {
        MacosAudioUnitBackend::new(DeviceSelection::default(), CpalAudioOptions::default())
    }

    #[test]
    fn aggregate_description_sets_master_private_and_uids() {
        let dict = aggregate_description("org.freewheeling.test.1", "input-uid", "output-uid");
        assert_eq!(
            dict_string(&dict, "uid"),
            Some("org.freewheeling.test.1".into())
        );
        assert_eq!(dict_string(&dict, "name"), Some(AGGREGATE_NAME.to_string()));
        // The output subdevice is the master clock.
        assert_eq!(dict_string(&dict, "master"), Some("output-uid".into()));
        // The device is private to this process and never published globally.
        assert_eq!(dict_number(&dict, "private"), Some(1));
    }

    #[test]
    fn aggregate_description_subdevices_carry_drift_on_input_only() {
        let dict = aggregate_description("org.freewheeling.test.2", "input-uid", "output-uid");
        let subdevices = dict_array(&dict, "subdevices").expect("subdevices key");
        assert_eq!(subdevices.len(), 2);
        let input = array_subdict(&subdevices, 0);
        assert_eq!(dict_string(&input, "uid"), Some("input-uid".into()));
        assert_eq!(dict_number(&input, "drift"), Some(1));
        let output = array_subdict(&subdevices, 1);
        assert_eq!(dict_string(&output, "uid"), Some("output-uid".into()));
        assert_eq!(dict_number(&output, "drift"), None);
    }

    #[test]
    fn aggregate_uids_are_unique_per_creation() {
        let first = aggregate_uid(0x11, 0x22);
        let second = aggregate_uid(0x11, 0x22);
        assert_ne!(first, second);
        assert!(first.starts_with(&format!(
            "org.freewheeling.aggregate.{}.",
            std::process::id()
        )));
    }

    #[test]
    fn owned_aggregate_device_is_destroyed_exactly_once() {
        let mut backend = test_backend();
        backend.route = Some(CoreAudioRoute {
            requested_input: 0x11,
            requested_output: 0x22,
            active_device: 0x99,
            owned_aggregate_uid: Some("org.freewheeling.test.owned".into()),
        });
        // The first claim consumes the ownership; a second claim is a no-op,
        // so the HAL destroy call can never run twice for one aggregate.
        assert_eq!(backend.take_owned_aggregate_device(), Some(0x99));
        assert_eq!(backend.take_owned_aggregate_device(), None);
        // destroy_owned_aggregate on an already-claimed route is a no-op too.
        backend.route = None;
        backend.destroy_owned_aggregate();
    }

    #[test]
    fn discovered_route_device_is_never_destroyed() {
        let mut backend = test_backend();
        // Same device for input and output: the route is a discovered physical
        // device, never an aggregate created by FreeWheeling.
        backend.route = Some(CoreAudioRoute {
            requested_input: 0x11,
            requested_output: 0x11,
            active_device: 0x11,
            owned_aggregate_uid: None,
        });
        assert_eq!(backend.take_owned_aggregate_device(), None);
    }

    #[test]
    fn destroy_owned_aggregate_clears_route_and_never_destroys_discovered_device() {
        let mut backend = test_backend();
        backend.route = Some(CoreAudioRoute {
            requested_input: 0x11,
            requested_output: 0x11,
            active_device: 0x11,
            owned_aggregate_uid: None,
        });
        // An externally supplied device (or a matching direct route) must not
        // be destroyed; destroy_owned_aggregate only clears the route.
        backend.destroy_owned_aggregate();
        assert!(backend.route.is_none());
    }
}

