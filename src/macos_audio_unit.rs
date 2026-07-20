//! Low-latency macOS duplex AudioUnit backend.
//!
//! Unlike CPAL's portable backend, this owns one HAL Output AudioUnit.  Its
//! render callback calls `AudioUnitRender` to obtain capture samples and then
//! immediately invokes the DSP with the device's output buffers.  There is no
//! capture/playback queue and consequently no extra callback of monitor or
//! recording latency.

use crate::audio_native_cpal::{
    AudioDeviceInfo, AudioLatency, CpalAudioBackend, CpalAudioOptions, CpalAudioStatus,
    DeviceSelection,
};
use crate::audioio::{
    AudioBackend, AudioCallback, AudioCallbackFn, AudioMetrics, AudioRecoveryMetrics, BackendInfo,
    JackPosition, NFrames, NUM_CHANNELS,
};
use crate::realtime_guard::RealtimeMetrics;
use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

const DEFAULT_RATE: u32 = 48_000;
const MAX_CALLBACK_FRAMES: usize = 16_384;
const NO_ERR: i32 = 0;

// AudioUnit component and property constants. Keeping this tiny FFI surface
// local makes the real-time ownership rules auditable and avoids a second
// abstraction layer around the exact callback shape used by the C++ app.
const K_AUDIO_UNIT_TYPE_OUTPUT: u32 = 0x6175_6f75; // 'auou'
const K_AUDIO_UNIT_SUBTYPE_HAL_OUTPUT: u32 = 0x6168_616c; // 'ahal'
const K_AUDIO_UNIT_MANUFACTURER_APPLE: u32 = 0x6170_706c; // 'appl'
const K_AUDIO_FORMAT_LINEAR_PCM: u32 = 0x6c70_636d; // 'lpcm'
const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1;
const K_AUDIO_FORMAT_FLAG_IS_PACKED: u32 = 1 << 3;
const K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED: u32 = 1 << 5;
const K_AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE: u32 = 2000;
const K_AUDIO_OUTPUT_UNIT_PROPERTY_ENABLE_IO: u32 = 2003;
const K_AUDIO_UNIT_PROPERTY_STREAM_FORMAT: u32 = 8;
const K_AUDIO_UNIT_PROPERTY_MAXIMUM_FRAMES_PER_SLICE: u32 = 14;
const K_AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK: u32 = 23;
const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE: u32 = 0x6449_6e20; // 'dIn '
const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE: u32 = 0x644f_7574; // 'dOut'
const K_AUDIO_DEVICE_PROPERTY_NOMINAL_SAMPLE_RATE: u32 = 0x6e73_7274; // 'nsrt'
const K_AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE: u32 = 0x6673_697a; // 'fsiz'
const K_AUDIO_DEVICE_PROPERTY_LATENCY: u32 = 0x6c74_6e63; // 'ltnc'
const K_AUDIO_DEVICE_PROPERTY_SAFETY_OFFSET: u32 = 0x7361_6674; // 'saft'
const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
const K_AUDIO_OBJECT_SCOPE_GLOBAL: u32 = 0;
const K_AUDIO_OBJECT_ELEMENT_MAIN: u32 = 0;
const K_AUDIO_OBJECT_SCOPE_INPUT: u32 = 0x696e_7074; // 'inpt'
const K_AUDIO_OBJECT_SCOPE_OUTPUT: u32 = 0x6f75_7470; // 'outp'
const K_AUDIO_UNIT_SCOPE_GLOBAL: u32 = 0;
const K_AUDIO_UNIT_SCOPE_INPUT: u32 = 1;
const K_AUDIO_UNIT_SCOPE_OUTPUT: u32 = 2;
const ROUTE_POLL_INTERVAL_MS: u64 = 250;

type AudioUnit = *mut c_void;
type AudioComponent = *mut c_void;

#[repr(C)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}

#[repr(C)]
struct AudioObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioTimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock_time: u64,
    smpte_time: [u8; 32],
    flags: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

// CoreAudio declares one trailing buffer; the fixed two-buffer form is ABI
// compatible and is exactly what the non-interleaved stereo input needs.
#[repr(C)]
struct AudioBufferList2 {
    number_buffers: u32,
    buffers: [AudioBuffer; NUM_CHANNELS],
}

#[repr(C)]
struct AudioBufferList1 {
    number_buffers: u32,
    first_buffer: AudioBuffer,
}

type RenderProc = unsafe extern "C" fn(
    *mut c_void,
    *mut u32,
    *const AudioTimeStamp,
    u32,
    u32,
    *mut AudioBufferList1,
) -> i32;

#[repr(C)]
struct AURenderCallbackStruct {
    input_proc: Option<RenderProc>,
    input_proc_ref_con: *mut c_void,
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(
        component: AudioComponent,
        desc: *const AudioComponentDescription,
    ) -> AudioComponent;
    fn AudioComponentInstanceNew(component: AudioComponent, unit: *mut AudioUnit) -> i32;
    fn AudioComponentInstanceDispose(unit: AudioUnit) -> i32;
    fn AudioUnitSetProperty(
        unit: AudioUnit,
        id: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        data_size: u32,
    ) -> i32;
    fn AudioUnitInitialize(unit: AudioUnit) -> i32;
    fn AudioUnitUninitialize(unit: AudioUnit) -> i32;
    fn AudioUnitRender(
        unit: AudioUnit,
        flags: *mut u32,
        timestamp: *const AudioTimeStamp,
        bus: u32,
        frames: u32,
        data: *mut AudioBufferList1,
    ) -> i32;
    fn AudioOutputUnitStart(unit: AudioUnit) -> i32;
    fn AudioOutputUnitStop(unit: AudioUnit) -> i32;
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectSetPropertyData(
        object: u32,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: u32,
        data: *const c_void,
    ) -> i32;
}

#[derive(Default)]
struct SharedMetrics {
    xruns: AtomicU64,
    stream_errors: AtomicU64,
    callbacks: AtomicU64,
    callback_frames: AtomicU64,
    callback_peak_nanos: AtomicU64,
    callback_total_nanos: AtomicU64,
    recovery_requests: AtomicU64,
    callback_panics: AtomicU64,
    active: AtomicBool,
    cpu_load_bits: AtomicU32,
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
            number_buffers: 2,
            buffers: [
                AudioBuffer {
                    number_channels: 1,
                    data_byte_size: 0,
                    data: input_left.as_mut_ptr().cast(),
                },
                AudioBuffer {
                    number_channels: 1,
                    data_byte_size: 0,
                    data: input_right.as_mut_ptr().cast(),
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
    selection: DeviceSelection,
    options: CpalAudioOptions,
    /// A HAL Output unit cannot use one physical device for output and another
    /// one for capture. For that ordinary macOS setup, retain a functional
    /// CPAL duplex route instead of silently returning capture errors forever.
    fallback: Option<CpalAudioBackend>,
    unit: AudioUnit,
    state: Option<Box<CallbackState>>,
    info: Option<BackendInfo>,
    latency: Option<AudioLatency>,
    buffer_restores: Vec<BufferRestore>,
    input_device: Option<u32>,
    output_device: Option<u32>,
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
    pub fn new(selection: DeviceSelection, options: CpalAudioOptions) -> Self {
        Self {
            selection,
            options,
            fallback: None,
            unit: ptr::null_mut(),
            state: None,
            info: None,
            latency: None,
            buffer_restores: Vec::new(),
            input_device: None,
            output_device: None,
            metrics: Arc::new(SharedMetrics::default()),
            realtime_metrics: None,
            route_poll_origin: Instant::now(),
            route_poll_after_ms: AtomicU64::new(0),
        }
    }

    pub fn set_realtime_metrics(&mut self, metrics: Arc<RealtimeMetrics>) {
        if let Some(fallback) = &mut self.fallback {
            fallback.set_realtime_metrics(Arc::clone(&metrics));
        }
        self.realtime_metrics = Some(metrics);
    }

    pub fn status(&self) -> CpalAudioStatus {
        if let Some(fallback) = &self.fallback {
            let mut status = fallback.status();
            status.latency = self.latency;
            return status;
        }
        let input = self.input_device.map(device_info);
        let output = self.output_device.map(device_info);
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
        }
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

    fn configure(&mut self) -> Result<BackendInfo, String> {
        let desc = AudioComponentDescription {
            component_type: K_AUDIO_UNIT_TYPE_OUTPUT,
            component_sub_type: K_AUDIO_UNIT_SUBTYPE_HAL_OUTPUT,
            component_manufacturer: K_AUDIO_UNIT_MANUFACTURER_APPLE,
            component_flags: 0,
            component_flags_mask: 0,
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
            let input = default_device(K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE)?;
            let output = default_device(K_AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE)?;
            set_property(
                unit,
                K_AUDIO_OUTPUT_UNIT_PROPERTY_CURRENT_DEVICE,
                K_AUDIO_UNIT_SCOPE_GLOBAL,
                0,
                &output,
                "select output device",
            )?;
            let rate = nominal_rate(output)
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
            let (previous_frames, frames) =
                set_low_latency_buffer(output, self.requested_frames())?;
            if previous_frames != frames {
                self.buffer_restores.push(BufferRestore {
                    device: output,
                    previous_frames,
                    applied_frames: frames,
                });
            }
            let max_frames = (MAX_CALLBACK_FRAMES as u32).max(frames);
            set_property(
                unit,
                K_AUDIO_UNIT_PROPERTY_MAXIMUM_FRAMES_PER_SLICE,
                K_AUDIO_UNIT_SCOPE_GLOBAL,
                0,
                &max_frames,
                "set maximum frames per slice",
            )?;
            let mut state = Box::new(CallbackState::new(
                rate,
                Arc::clone(&self.metrics),
                self.realtime_metrics.clone(),
            ));
            state.unit = unit;
            let callback = AURenderCallbackStruct {
                input_proc: Some(render_callback),
                input_proc_ref_con: (&mut *state as *mut CallbackState).cast(),
            };
            set_property(
                unit,
                K_AUDIO_UNIT_PROPERTY_SET_RENDER_CALLBACK,
                K_AUDIO_UNIT_SCOPE_INPUT,
                0,
                &callback,
                "set duplex render callback",
            )?;
            check(
                unsafe { AudioUnitInitialize(unit) },
                "initialize HAL audio unit",
            )?;
            self.input_device = Some(input);
            self.output_device = Some(output);
            self.latency = latency_estimate(input, output, frames, 0);
            self.state = Some(state);
            Ok(BackendInfo {
                sample_rate: rate,
                buffer_size: frames,
            })
        })();
        if setup.is_err() {
            self.dispose_unit();
            self.restore_device_buffers();
        }
        setup
    }

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
        self.input_device = None;
        self.output_device = None;
    }

    fn remember_fallback_buffers(&mut self, input: u32, output: u32, frames: u32) {
        for device in [input, output] {
            if self
                .buffer_restores
                .iter()
                .any(|restore| restore.device == device)
            {
                continue;
            }
            let Some(previous_frames) = device_u32(
                device,
                K_AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE,
                K_AUDIO_OBJECT_SCOPE_GLOBAL,
            )
            .ok() else {
                continue;
            };
            // CPAL applies the requested fixed size when building streams.
            // Save the system setting first and record the applied value only
            // after it is observable below.
            self.buffer_restores.push(BufferRestore {
                device,
                previous_frames,
                applied_frames: frames,
            });
        }
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
        let (Some(input), Some(output), Some(info)) =
            (self.input_device, self.output_device, self.info)
        else {
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
            Ok(current) if current == input
        ) || !matches!(
            default_device(K_AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE),
            Ok(current) if current == output
        );
        let device_format_changed = nominal_rate(output)
            .map(|rate| rate.round() as u32 != info.sample_rate)
            .unwrap_or(true)
            || device_u32(
                output,
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
        if input != output {
            eprintln!(
                "FreeWheeling audio: default input device {input} and output device {output} differ; \
                 using the default macOS duplex mic/speaker route (expected on MacBooks). \
                 A matching duplex or Aggregate Device only enables the optional single-callback path."
            );
            let mut fallback = CpalAudioBackend::new(self.selection.clone(), self.options);
            if let Some(metrics) = &self.realtime_metrics {
                fallback.set_realtime_metrics(Arc::clone(metrics));
            }
            self.remember_fallback_buffers(input, output, self.requested_frames());
            let info = match fallback.open("FreeWheeling") {
                Ok(info) => info,
                Err(error) => {
                    self.restore_device_buffers();
                    return Err(error);
                }
            };
            // A device may clamp the request. Restore only the value CPAL
            // actually applied, not an optimistic requested value.
            for restore in &mut self.buffer_restores {
                if let Ok(actual) = device_u32(
                    restore.device,
                    K_AUDIO_DEVICE_PROPERTY_BUFFER_FRAME_SIZE,
                    K_AUDIO_OBJECT_SCOPE_GLOBAL,
                ) {
                    restore.applied_frames = actual;
                }
            }
            self.input_device = Some(input);
            self.output_device = Some(output);
            self.latency = latency_estimate(input, output, info.buffer_size, info.buffer_size);
            self.fallback = Some(fallback);
            return Ok(info);
        }
        let info = self.configure()?;
        self.info = Some(info);
        Ok(info)
    }

    fn activate(&mut self, callback: AudioCallbackFn) -> Result<(), String> {
        if let Some(fallback) = &mut self.fallback {
            return fallback.activate(callback);
        }
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
        if let Some(mut fallback) = self.fallback.take() {
            fallback.close();
        }
        self.dispose_unit();
        self.state = None;
        self.restore_device_buffers();
    }

    fn relocate(&mut self, _frame: NFrames) {}

    fn metrics(&self) -> AudioMetrics {
        if let Some(fallback) = &self.fallback {
            return fallback.metrics();
        }
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
        if let Some(fallback) = &self.fallback {
            return fallback.cpu_load();
        }
        Some(self.metrics.cpu_load())
    }

    fn recovery_requested(&self) -> bool {
        if let Some(fallback) = &self.fallback {
            return fallback.recovery_requested();
        }
        if self.direct_route_changed() {
            self.metrics
                .recovery_requests
                .fetch_add(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn recover(&mut self) -> Result<BackendInfo, String> {
        if let Some(fallback) = &mut self.fallback {
            return fallback.recover();
        }
        let callback = self
            .take_callback()
            .ok_or("audio callback is unavailable for recovery")?;
        self.close();
        let info = self.open("FreeWheeling")?;
        self.activate(callback).map(|()| info)
    }

    fn recovery_metrics(&self) -> AudioRecoveryMetrics {
        if let Some(fallback) = &self.fallback {
            return fallback.recovery_metrics();
        }
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
    io_data: *mut AudioBufferList1,
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
    state.capture.buffers[0].data_byte_size = frames * std::mem::size_of::<f32>() as u32;
    state.capture.buffers[1].data_byte_size = frames * std::mem::size_of::<f32>() as u32;
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
        state.input_left[..count].fill(0.0);
        state.input_right[..count].fill(0.0);
        state.metrics.stream_errors.fetch_add(1, Ordering::Relaxed);
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: HAL is configured for non-interleaved f32 stereo. The
        // callback's `frames` cannot exceed either supplied buffer size.
        let output = unsafe { &mut *io_data };
        let left_buffer = unsafe { buffer_at(io_data, 0) };
        let left = left_buffer.data.cast::<f32>();
        if left.is_null() || left_buffer.data_byte_size < frames * 4 {
            return;
        }
        let left = unsafe { std::slice::from_raw_parts_mut(left, count) };
        let right = if output.number_buffers > 1 {
            let right_buffer = unsafe { buffer_at(io_data, 1) };
            let pointer = right_buffer.data.cast::<f32>();
            if pointer.is_null() || right_buffer.data_byte_size < frames * 4 {
                None
            } else {
                Some(unsafe { std::slice::from_raw_parts_mut(pointer, count) })
            }
        } else {
            None
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
    }));
    if result.is_err() {
        state
            .metrics
            .callback_panics
            .fetch_add(1, Ordering::Relaxed);
        state.metrics.stream_errors.fetch_add(1, Ordering::Relaxed);
        zero_output(io_data);
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

fn zero_output(io_data: *mut AudioBufferList1) {
    if io_data.is_null() {
        return;
    }
    // SAFETY: CoreAudio owns `io_data`; only clear the buffers it reports.
    unsafe {
        let output = &mut *io_data;
        for index in 0..output.number_buffers.min(2) as usize {
            let buffer = buffer_at(io_data, index);
            if !buffer.data.is_null() {
                ptr::write_bytes(buffer.data, 0, buffer.data_byte_size as usize);
            }
        }
    }
}

/// # Safety
/// `list` must point to a CoreAudio AudioBufferList containing `index + 1`
/// buffers. The caller checks `number_buffers` before requesting an index.
unsafe fn buffer_at<'a>(list: *mut AudioBufferList1, index: usize) -> &'a mut AudioBuffer {
    // SAFETY: AudioBufferList stores its trailing array immediately after the
    // count field. `first_buffer` begins at the same location as mBuffers[0].
    unsafe { &mut *(ptr::addr_of_mut!((*list).first_buffer).add(index)) }
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

fn default_device(selector: u32) -> Result<u32, String> {
    let address = AudioObjectPropertyAddress {
        selector,
        scope: K_AUDIO_OBJECT_SCOPE_GLOBAL,
        element: K_AUDIO_OBJECT_ELEMENT_MAIN,
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
        selector: K_AUDIO_DEVICE_PROPERTY_NOMINAL_SAMPLE_RATE,
        scope: K_AUDIO_OBJECT_SCOPE_GLOBAL,
        element: K_AUDIO_OBJECT_ELEMENT_MAIN,
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
        selector,
        scope,
        element: K_AUDIO_OBJECT_ELEMENT_MAIN,
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
        selector,
        scope,
        element: K_AUDIO_OBJECT_ELEMENT_MAIN,
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
        sample_rate: rate as f64,
        format_id: K_AUDIO_FORMAT_LINEAR_PCM,
        format_flags: K_AUDIO_FORMAT_FLAG_IS_FLOAT
            | K_AUDIO_FORMAT_FLAG_IS_PACKED
            | K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED,
        bytes_per_packet: 4,
        frames_per_packet: 1,
        bytes_per_frame: 4,
        channels_per_frame: 2,
        bits_per_channel: 32,
        reserved: 0,
    }
}
