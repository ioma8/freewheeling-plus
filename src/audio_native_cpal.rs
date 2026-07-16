//! Production CPAL duplex backend.
//!
//! CPAL owns separate capture and playback callbacks. Captured stereo frames
//! cross a bounded `rtrb` queue; DSP remains exclusively owned by playback.

use crate::audioio::{
    AudioBackend, AudioCallback, AudioCallbackFn, AudioMetrics, AudioRecoveryMetrics, BackendInfo,
    JackPosition, NFrames, NUM_CHANNELS,
};
use crate::realtime_guard::RealtimeMetrics;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, ErrorKind, SampleFormat, Stream, StreamConfig, SupportedBufferSize,
};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_RATE: u32 = 48_000;
// Match Config::Config() in the C++ implementation.  At 48 kHz this halves
// one callback period from 5.3 ms to 2.7 ms.
// The tested MacBook built-in route sustains 16 frames at 48 kHz (< 5 ms
// estimated round trip). Other platforms retain the more conservative
// 64-frame request. FWEELIN_AUDIO_BUFFER_FRAMES overrides either default.
#[cfg(target_os = "macos")]
const DEFAULT_BUFFER_FRAMES: u32 = 16;
#[cfg(not(target_os = "macos"))]
const DEFAULT_BUFFER_FRAMES: u32 = 64;
const MIN_RING_PERIODS: usize = 2;
// Capture and playback are separate streams on the non-aggregate macOS
// fallback. Keep enough bounded headroom for the host to arm playback after
// capture has started. Playback trims this safety backlog to one callback
// period before invoking DSP, so the larger capacity is not extra steady-state
// latency.
const DEFAULT_RING_PERIODS: usize = 32;
const MAX_CALLBACK_FRAMES: usize = 16_384;
const ROUTE_POLL_INTERVAL_MS: u64 = 250;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSelection {
    /// Stable CPAL device ID. Preferred over `name` when both are supplied.
    pub input_id: Option<String>,
    pub output_id: Option<String>,
    /// Compatibility fallback for configurations written before stable IDs.
    pub input_name: Option<String>,
    pub output_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// A non-realtime latency estimate. Device values are supplied by platform
/// backends when the host exposes them; the queue value is an explicit upper
/// bound added by this application.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioLatency {
    pub input_device_frames: u32,
    pub output_device_frames: u32,
    pub software_queue_frames: u32,
    pub estimated_round_trip_frames: u32,
}

/// Non-realtime activation/diagnostic snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpalAudioStatus {
    pub active: bool,
    pub input: Option<AudioDeviceInfo>,
    pub output: Option<AudioDeviceInfo>,
    pub format: Option<BackendInfo>,
    pub capture_callbacks: u64,
    pub playback_callbacks: u64,
    pub latency: Option<AudioLatency>,
    pub metrics: AudioMetrics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpalAudioOptions {
    pub preferred_sample_rate: u32,
    pub preferred_buffer_frames: u32,
    pub ring_periods: usize,
}

impl Default for CpalAudioOptions {
    fn default() -> Self {
        Self {
            preferred_sample_rate: DEFAULT_RATE,
            preferred_buffer_frames: std::env::var("FWEELIN_AUDIO_BUFFER_FRAMES")
                .ok()
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|frames| *frames > 0)
                .unwrap_or(DEFAULT_BUFFER_FRAMES),
            ring_periods: DEFAULT_RING_PERIODS,
        }
    }
}

#[derive(Default)]
struct SharedMetrics {
    capture_overruns: AtomicU64,
    capture_underruns: AtomicU64,
    xruns: AtomicU64,
    stream_errors: AtomicU64,
    callbacks: AtomicU64,
    capture_callbacks: AtomicU64,
    callback_frames: AtomicU64,
    callback_peak_nanos: AtomicU64,
    callback_total_nanos: AtomicU64,
    recovery_requests: AtomicU64,
    recovery_requested: AtomicBool,
    recovery_attempts: AtomicU64,
    recovery_failures: AtomicU64,
    cpu_load_bits: AtomicU32,
}

impl SharedMetrics {
    fn cpu_load(&self) -> f32 {
        f32::from_bits(self.cpu_load_bits.load(Ordering::Acquire))
    }
    fn stream_error(&self, xrun: bool) {
        if xrun {
            self.xruns.fetch_add(1, Ordering::Relaxed);
        }
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
        self.recovery_requests.fetch_add(1, Ordering::Relaxed);
        self.recovery_requested.store(true, Ordering::Release);
    }

    fn snapshot(&self) -> AudioMetrics {
        AudioMetrics {
            capture_overruns: self.capture_overruns.load(Ordering::Relaxed),
            capture_underruns: self.capture_underruns.load(Ordering::Relaxed),
            xruns: self.xruns.load(Ordering::Relaxed),
            stream_errors: self.stream_errors.load(Ordering::Relaxed),
            callbacks: self.callbacks.load(Ordering::Relaxed),
            callback_frames: self.callback_frames.load(Ordering::Relaxed),
            callback_peak_nanos: self.callback_peak_nanos.load(Ordering::Relaxed),
            callback_total_nanos: self.callback_total_nanos.load(Ordering::Relaxed),
            recovery_requests: self.recovery_requests.load(Ordering::Relaxed),
        }
    }
}

struct OpenedDevices {
    input: Device,
    output: Device,
    input_config: StreamConfig,
    output_config: StreamConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenedRoute {
    input_id: String,
    output_id: String,
    input_name: String,
    output_name: String,
}

pub struct CpalAudioBackend {
    selection: DeviceSelection,
    options: CpalAudioOptions,
    opened: Option<OpenedDevices>,
    input_stream: Option<Stream>,
    output_stream: Option<Stream>,
    callback_return: Option<Consumer<AudioCallbackFn>>,
    retained_callback: Option<AudioCallbackFn>,
    route: Option<OpenedRoute>,
    metrics: Arc<SharedMetrics>,
    info: Option<BackendInfo>,
    realtime_metrics: Option<Arc<RealtimeMetrics>>,
    route_poll_origin: Instant,
    route_poll_after_ms: AtomicU64,
}

impl CpalAudioBackend {
    pub fn new(selection: DeviceSelection, options: CpalAudioOptions) -> Self {
        Self {
            selection,
            options,
            opened: None,
            input_stream: None,
            output_stream: None,
            callback_return: None,
            retained_callback: None,
            route: None,
            metrics: Arc::new(SharedMetrics::default()),
            info: None,
            realtime_metrics: None,
            route_poll_origin: Instant::now(),
            route_poll_after_ms: AtomicU64::new(0),
        }
    }

    /// Attach acceptance instrumentation before activating the streams.
    pub fn set_realtime_metrics(&mut self, metrics: Arc<RealtimeMetrics>) {
        self.realtime_metrics = Some(metrics);
    }

    pub fn discover_input_devices() -> Result<Vec<AudioDeviceInfo>, String> {
        let host = cpal::default_host();
        let default_id = host
            .default_input_device()
            .and_then(|device| device.id().ok())
            .map(|id| id.to_string());
        host.input_devices()
            .map_err(|error| format!("cannot enumerate audio input devices: {error}"))?
            .map(|device| describe_device(device, default_id.as_deref()))
            .collect()
    }

    pub fn discover_output_devices() -> Result<Vec<AudioDeviceInfo>, String> {
        let host = cpal::default_host();
        let default_id = host
            .default_output_device()
            .and_then(|device| device.id().ok())
            .map(|id| id.to_string());
        host.output_devices()
            .map_err(|error| format!("cannot enumerate audio output devices: {error}"))?
            .map(|device| describe_device(device, default_id.as_deref()))
            .collect()
    }

    /// Non-realtime route monitors may use this to request a controlled
    /// stop/open/activate cycle from the application owner.
    pub fn request_recovery(&self) {
        self.metrics
            .recovery_requests
            .fetch_add(1, Ordering::Relaxed);
        self.metrics
            .recovery_requested
            .store(true, Ordering::Release);
    }

    pub fn clear_recovery_request(&self) {
        self.metrics
            .recovery_requested
            .store(false, Ordering::Release);
    }

    /// Snapshot for non-realtime UI, logging, and watchdog code.
    pub fn status(&self) -> CpalAudioStatus {
        let input = self.route.as_ref().map(|r| AudioDeviceInfo {
            id: r.input_id.clone(),
            name: r.input_name.clone(),
            is_default: false,
        });
        let output = self.route.as_ref().map(|r| AudioDeviceInfo {
            id: r.output_id.clone(),
            name: r.output_name.clone(),
            is_default: false,
        });
        CpalAudioStatus {
            active: self.input_stream.is_some() && self.output_stream.is_some(),
            input,
            output,
            format: self.info,
            capture_callbacks: self.metrics.capture_callbacks.load(Ordering::Relaxed),
            playback_callbacks: self.metrics.callbacks.load(Ordering::Relaxed),
            latency: None,
            metrics: self.metrics.snapshot(),
        }
    }

    /// Validate that both CPAL callbacks have started; call from a
    /// non-realtime owner after allowing the host to run callbacks.
    pub fn callback_health(&self) -> Result<(), String> {
        let status = self.status();
        if !status.active {
            return Err("audio streams are not active".into());
        }
        if status.capture_callbacks == 0 {
            return Err("audio capture callback has not run".into());
        }
        if status.playback_callbacks == 0 {
            return Err("audio playback callback has not run".into());
        }
        Ok(())
    }

    fn open_devices(&self) -> Result<(OpenedDevices, BackendInfo, OpenedRoute), String> {
        let host = cpal::default_host();
        let input = select_device(
            &host,
            true,
            self.selection.input_id.as_deref(),
            self.selection.input_name.as_deref(),
        )?;
        let output = select_device(
            &host,
            false,
            self.selection.output_id.as_deref(),
            self.selection.output_name.as_deref(),
        )?;
        let route = OpenedRoute {
            input_id: device_id(&input, true)?,
            output_id: device_id(&output, false)?,
            input_name: device_name(&input)?,
            output_name: device_name(&output)?,
        };
        let (rate, buffer_frames) = negotiate_format(&input, &output, self.options)?;
        let input_channels = best_channel_count(&input, true, rate)?;
        let output_channels = best_channel_count(&output, false, rate)?;
        let input_config = StreamConfig {
            channels: input_channels,
            sample_rate: rate,
            buffer_size: BufferSize::Fixed(buffer_frames),
        };
        let output_config = StreamConfig {
            channels: output_channels,
            sample_rate: rate,
            buffer_size: BufferSize::Fixed(buffer_frames),
        };
        Ok((
            OpenedDevices {
                input,
                output,
                input_config,
                output_config,
            },
            BackendInfo {
                sample_rate: rate,
                buffer_size: buffer_frames,
            },
            route,
        ))
    }

    fn route_changed(&self) -> bool {
        let Some(route) = &self.route else {
            return false;
        };
        // This runs from the UI/control thread. Device enumeration can touch
        // the HAL, so bound it rather than doing it once per input-event poll.
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
        self.open_devices()
            .map(|(_, _, current)| current != *route)
            .unwrap_or(true)
    }

    fn reclaim_callback(&mut self) {
        if self.retained_callback.is_none()
            && let Some(consumer) = &mut self.callback_return
            && let Ok(callback) = consumer.pop()
        {
            self.retained_callback = Some(callback);
        }
        if self.retained_callback.is_some() {
            self.callback_return = None;
        }
    }
}

impl AudioBackend for CpalAudioBackend {
    fn open(&mut self, _client_name: &str) -> Result<BackendInfo, String> {
        self.close();
        let (opened, info, route) = self.open_devices()?;
        // Upgrade legacy name selections to the stable identity actually
        // opened. Defaults intentionally remain dynamic route selections.
        if self.selection.input_id.is_none() && self.selection.input_name.is_some() {
            self.selection.input_id = Some(route.input_id.clone());
        }
        if self.selection.output_id.is_none() && self.selection.output_name.is_some() {
            self.selection.output_id = Some(route.output_id.clone());
        }
        self.opened = Some(opened);
        self.info = Some(info);
        self.route = Some(route);
        self.clear_recovery_request();
        Ok(info)
    }

    fn activate(&mut self, callback: AudioCallbackFn) -> Result<(), String> {
        if self.input_stream.is_some() || self.output_stream.is_some() {
            return Err("audio backend is already active".to_string());
        }
        let opened = self
            .opened
            .take()
            .ok_or_else(|| "audio backend must be opened before activation".to_string())?;
        // Hold ownership outside CPAL until the output closure is built. This
        // also makes capture-build failures retryable during recovery.
        self.retained_callback = Some(callback);
        let info = self.info.expect("opened audio backend has format info");
        let periods = self.options.ring_periods.max(MIN_RING_PERIODS);
        let ring_frames = info.buffer_size as usize * periods;
        let (mut producer, consumer) = RingBuffer::<[f32; NUM_CHANNELS]>::new(ring_frames);

        let input_metrics = Arc::clone(&self.metrics);
        let input_realtime_metrics = self.realtime_metrics.clone();
        let input_channels = opened.input_config.channels as usize;
        let input_stream = opened
            .input
            .build_input_stream(
                opened.input_config,
                move |data: &[f32], _| {
                    let _guard = input_realtime_metrics
                        .as_ref()
                        .map(|metrics| metrics.enter_callback());
                    input_metrics
                        .capture_callbacks
                        .fetch_add(1, Ordering::Relaxed);
                    capture_callback(data, input_channels, &mut producer, &input_metrics)
                },
                stream_error_callback(Arc::clone(&self.metrics), self.realtime_metrics.clone()),
                None,
            )
            .map_err(|error| format!("cannot build audio capture stream: {error}"))?;

        let output_channels = opened.output_config.channels as usize;
        let output_metrics = Arc::clone(&self.metrics);
        let (callback_sender, callback_return) = RingBuffer::<AudioCallbackFn>::new(1);
        self.callback_return = Some(callback_return);
        let callback = self
            .retained_callback
            .take()
            .expect("activation retained its callback");
        let output_stream = opened
            .output
            .build_output_stream(
                opened.output_config,
                playback_callback(
                    consumer,
                    callback,
                    callback_sender,
                    output_channels,
                    info.sample_rate,
                    info.buffer_size as usize,
                    Arc::clone(&output_metrics),
                    self.realtime_metrics.clone(),
                ),
                stream_error_callback(Arc::clone(&self.metrics), self.realtime_metrics.clone()),
                None,
            )
            .map_err(|error| {
                self.reclaim_callback();
                format!("cannot build audio playback stream: {error}")
            })?;

        // Start capture before playback. These are separate streams on the
        // fallback route; starting playback first can consume several empty
        // periods before CoreAudio schedules the input callback, which shows
        // up as a permanent startup underrun in diagnostics and loses the
        // first live input frames. Give capture a short non-realtime startup
        // window to publish at least one frame before arming playback. This
        // does not add steady-state latency: playback still trims the queue
        // to at most one callback period below.
        let capture_callbacks_before_start = self.metrics.capture_callbacks.load(Ordering::Acquire);
        if let Err(error) = input_stream.play() {
            self.reclaim_callback();
            return Err(format!("cannot start audio capture stream: {error}"));
        }
        let capture_deadline = Instant::now() + Duration::from_millis(100);
        while self.metrics.capture_callbacks.load(Ordering::Acquire)
            == capture_callbacks_before_start
            && Instant::now() < capture_deadline
        {
            thread::sleep(Duration::from_millis(1));
        }
        let playback_callbacks_before_start = self.metrics.callbacks.load(Ordering::Acquire);
        if let Err(error) = output_stream.play() {
            let _ = input_stream.pause();
            drop(output_stream);
            self.reclaim_callback();
            return Err(format!("cannot start audio playback stream: {error}"));
        }
        // C++ `AudioIO::activate` busy-waits for its single duplex callback
        // to run at least once before returning, so callers can trust the
        // realtime path is live. CPAL's split streams mean capture and
        // playback need separate confirmation; only capture was awaited
        // above, so `callback_health()` could pass on a backend whose
        // playback callback had never actually run yet.
        let playback_deadline = Instant::now() + Duration::from_millis(100);
        while self.metrics.callbacks.load(Ordering::Acquire) == playback_callbacks_before_start
            && Instant::now() < playback_deadline
        {
            thread::sleep(Duration::from_millis(1));
        }
        self.input_stream = Some(input_stream);
        self.output_stream = Some(output_stream);
        self.retained_callback = None;
        Ok(())
    }

    fn close(&mut self) {
        // Pause first so callbacks quiesce before streams (and their callback
        // closures, ring endpoints, and processor) are reclaimed.
        if let Some(stream) = self.output_stream.take() {
            let _ = stream.pause();
            drop(stream);
        }
        self.reclaim_callback();
        if let Some(stream) = self.input_stream.take() {
            let _ = stream.pause();
            drop(stream);
        }
        self.opened = None;
        self.route = None;
    }

    fn relocate(&mut self, _frame: NFrames) {
        // CPAL has no transport. Higher layers keep the portable position.
    }

    fn metrics(&self) -> AudioMetrics {
        self.metrics.snapshot()
    }

    fn cpu_load(&self) -> Option<f32> {
        Some(self.metrics.cpu_load())
    }

    fn recovery_requested(&self) -> bool {
        if self.route_changed() && !self.metrics.recovery_requested.swap(true, Ordering::AcqRel) {
            self.metrics
                .recovery_requests
                .fetch_add(1, Ordering::Relaxed);
        }
        self.metrics.recovery_requested.load(Ordering::Acquire)
    }

    fn recover(&mut self) -> Result<BackendInfo, String> {
        self.metrics
            .recovery_attempts
            .fetch_add(1, Ordering::Relaxed);
        self.close();
        let callback = self.retained_callback.take().ok_or_else(|| {
            self.metrics
                .recovery_failures
                .fetch_add(1, Ordering::Relaxed);
            "audio callback was not returned after streams quiesced".to_string()
        })?;
        let info = match self.open("freewheeling") {
            Ok(info) => info,
            Err(error) => {
                self.retained_callback = Some(callback);
                self.metrics
                    .recovery_failures
                    .fetch_add(1, Ordering::Relaxed);
                return Err(error);
            }
        };
        let result = self.activate(callback).map(|()| info);
        if result.is_err() {
            self.reclaim_callback();
            self.metrics
                .recovery_failures
                .fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    fn recovery_metrics(&self) -> AudioRecoveryMetrics {
        AudioRecoveryMetrics {
            attempts: self.metrics.recovery_attempts.load(Ordering::Relaxed),
            failures: self.metrics.recovery_failures.load(Ordering::Relaxed),
        }
    }
}

fn device_id(device: &Device, input: bool) -> Result<String, String> {
    device
        .id()
        .map(|id| id.to_string())
        .map_err(|error| format!("cannot query audio {} device ID: {error}", direction(input)))
}

fn device_name(device: &Device) -> Result<String, String> {
    device
        .description()
        .map(|description| description.name().to_owned())
        .map_err(|error| format!("cannot query audio device name: {error}"))
}

fn describe_device(device: Device, default_id: Option<&str>) -> Result<AudioDeviceInfo, String> {
    let id = device
        .id()
        .map_err(|error| format!("cannot query audio device ID: {error}"))?
        .to_string();
    let name = device
        .description()
        .map(|description| description.name().to_owned())
        .unwrap_or_else(|_| id.clone());
    Ok(AudioDeviceInfo {
        is_default: default_id == Some(id.as_str()),
        id,
        name,
    })
}

fn select_device(
    host: &cpal::Host,
    input: bool,
    selected_id: Option<&str>,
    selected_name: Option<&str>,
) -> Result<Device, String> {
    if selected_id.is_none() && selected_name.is_none() {
        return if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        }
        .ok_or_else(|| format!("no default audio {} device", direction(input)));
    }
    let devices = if input {
        host.input_devices()
    } else {
        host.output_devices()
    }
    .map_err(|error| {
        format!(
            "cannot enumerate audio {} devices: {error}",
            direction(input)
        )
    })?;
    for device in devices {
        let id_matches =
            selected_id.is_some_and(|wanted| device.id().is_ok_and(|id| id.to_string() == wanted));
        let name_matches = selected_name.is_some_and(|wanted| {
            device
                .description()
                .is_ok_and(|description| description.name() == wanted)
        });
        if id_matches || (selected_id.is_none() && name_matches) {
            return Ok(device);
        }
    }
    Err(format!(
        "selected audio {} device is unavailable (id={selected_id:?}, name={selected_name:?})",
        direction(input)
    ))
}

fn direction(input: bool) -> &'static str {
    if input { "input" } else { "output" }
}

fn negotiate_format(
    input: &Device,
    output: &Device,
    options: CpalAudioOptions,
) -> Result<(u32, u32), String> {
    let input_ranges: Vec<_> = input
        .supported_input_configs()
        .map_err(|error| format!("cannot query input formats: {error}"))?
        .filter(|range| range.sample_format() == SampleFormat::F32)
        .collect();
    let output_ranges: Vec<_> = output
        .supported_output_configs()
        .map_err(|error| format!("cannot query output formats: {error}"))?
        .filter(|range| range.sample_format() == SampleFormat::F32)
        .collect();
    let preferred = options.preferred_sample_rate.max(1);
    let mut candidates = [preferred, DEFAULT_RATE, 44_100, 96_000];
    candidates.sort_by_key(|rate| rate.abs_diff(preferred));
    let rate = candidates
        .into_iter()
        .find(|rate| supports_rate(&input_ranges, *rate) && supports_rate(&output_ranges, *rate))
        .or_else(|| {
            input_ranges.iter().find_map(|input_range| {
                let low = input_range.min_sample_rate();
                let high = input_range.max_sample_rate();
                output_ranges.iter().find_map(|output_range| {
                    let common_low = low.max(output_range.min_sample_rate());
                    let common_high = high.min(output_range.max_sample_rate());
                    (common_low <= common_high).then_some(preferred.clamp(common_low, common_high))
                })
            })
        })
        .ok_or_else(|| "input and output have no common f32 sample rate".to_string())?;
    let requested = options.preferred_buffer_frames.max(1);
    let input_buffer = common_buffer_limit(&input_ranges, rate);
    let output_buffer = common_buffer_limit(&output_ranges, rate);
    let buffer = choose_buffer_frames(requested, input_buffer, output_buffer)
        .ok_or_else(|| "input and output have no common fixed buffer size".to_string())?;
    Ok((rate, buffer))
}

fn supports_rate(ranges: &[cpal::SupportedStreamConfigRange], rate: u32) -> bool {
    ranges
        .iter()
        .any(|range| range.min_sample_rate() <= rate && rate <= range.max_sample_rate())
}

fn common_buffer_limit(
    ranges: &[cpal::SupportedStreamConfigRange],
    rate: u32,
) -> Option<(u32, u32)> {
    ranges
        .iter()
        .filter(|range| range.min_sample_rate() <= rate && rate <= range.max_sample_rate())
        .map(|range| match range.buffer_size() {
            SupportedBufferSize::Range { min, max } => (*min, *max),
            SupportedBufferSize::Unknown => (1, MAX_CALLBACK_FRAMES as u32),
        })
        .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
}

fn choose_buffer_frames(
    preferred: u32,
    input: Option<(u32, u32)>,
    output: Option<(u32, u32)>,
) -> Option<u32> {
    let input = input?;
    let output = output?;
    let min = input.0.max(output.0);
    let max = input.1.min(output.1).min(MAX_CALLBACK_FRAMES as u32);
    if min <= max {
        Some(preferred.clamp(min, max))
    } else {
        None
    }
}

fn best_channel_count(device: &Device, input: bool, rate: u32) -> Result<u16, String> {
    let channels: Vec<u16> = if input {
        device
            .supported_input_configs()
            .map_err(|error| format!("cannot query audio input formats: {error}"))?
            .filter(|range| {
                range.sample_format() == SampleFormat::F32
                    && range.min_sample_rate() <= rate
                    && rate <= range.max_sample_rate()
            })
            .map(|range| range.channels())
            .collect()
    } else {
        device
            .supported_output_configs()
            .map_err(|error| format!("cannot query audio output formats: {error}"))?
            .filter(|range| {
                range.sample_format() == SampleFormat::F32
                    && range.min_sample_rate() <= rate
                    && rate <= range.max_sample_rate()
            })
            .map(|range| range.channels())
            .collect()
    };
    channels
        .into_iter()
        .min_by_key(|channels| channels.abs_diff(NUM_CHANNELS as u16))
        .ok_or_else(|| format!("audio {} has no negotiated f32 format", direction(input)))
}

fn capture_callback(
    data: &[f32],
    channels: usize,
    producer: &mut Producer<[f32; NUM_CHANNELS]>,
    metrics: &SharedMetrics,
) {
    if channels == 0 {
        return;
    }
    let mut dropped = 0u64;
    for frame in data.chunks_exact(channels) {
        let stereo = [frame[0], frame.get(1).copied().unwrap_or(frame[0])];
        if producer.push(stereo).is_err() {
            dropped += 1;
        }
    }
    if dropped != 0 {
        metrics
            .capture_overruns
            .fetch_add(dropped, Ordering::Relaxed);
    }
}

fn playback_callback(
    mut consumer: Consumer<[f32; NUM_CHANNELS]>,
    processor: AudioCallbackFn,
    callback_sender: Producer<AudioCallbackFn>,
    channels: usize,
    sample_rate: u32,
    expected_frames: usize,
    metrics: Arc<SharedMetrics>,
    realtime_metrics: Option<Arc<RealtimeMetrics>>,
) -> impl FnMut(&mut [f32], &cpal::OutputCallbackInfo) + Send + 'static {
    struct ReturnedCallback {
        callback: Option<AudioCallbackFn>,
        sender: Producer<AudioCallbackFn>,
    }
    impl Drop for ReturnedCallback {
        fn drop(&mut self) {
            if let Some(callback) = self.callback.take() {
                let _ = self.sender.push(callback);
            }
        }
    }
    let mut returned = ReturnedCallback {
        callback: Some(processor),
        sender: callback_sender,
    };
    let mut left = vec![0.0; MAX_CALLBACK_FRAMES];
    let mut right = vec![0.0; MAX_CALLBACK_FRAMES];
    let mut output_left = vec![0.0; MAX_CALLBACK_FRAMES];
    let mut output_right = vec![0.0; MAX_CALLBACK_FRAMES];
    let mut frame_position = 0u64;
    let mut cpu_sample_count = 0u32;
    let mut cpu_sample_frames = 0u64;
    let mut cpu_sample_start = None;
    move |data, _| {
        let _guard = realtime_metrics
            .as_ref()
            .map(|metrics| metrics.enter_callback());
        let started = Instant::now();
        if cpu_sample_count == 0 {
            cpu_sample_start = Some(started);
        }
        if channels == 0 {
            data.fill(0.0);
            return;
        }
        let frame_count = data.len() / channels;
        // C++ `AudioIO::process` treats any callback whose frame count
        // differs from the negotiated fixed buffer size as fatal, since
        // every downstream buffer assumes a constant fragment size. CPAL
        // gives no such guarantee across host/device changes; surface a
        // mismatch as a counted condition instead of silently chunking
        // through it unnoticed.
        if frame_count != expected_frames {
            metrics.xruns.fetch_add(1, Ordering::Relaxed);
        }
        // Capture and playback are separate CPAL streams.  Capture is started
        // first and can fill several ring periods while playback starts.  If
        // retained, that startup backlog becomes permanent monitor latency.
        // Keep at most one callback queued: enough phase tolerance without
        // allowing the ring's safety capacity to become audible delay.
        let queued = consumer.slots();
        let max_queued = frame_count;
        let trimmed = queued.saturating_sub(max_queued);
        for _ in 0..trimmed {
            let _ = consumer.pop();
        }
        // Capture and playback run on independent hardware clocks with no
        // drift compensation, unlike C++'s single duplex callback which has
        // none. This periodic trim is an implicit, uncontrolled resampler:
        // count what it discards so clock drift is observable rather than
        // silently glitching audio.
        if trimmed != 0 {
            metrics
                .capture_overruns
                .fetch_add(trimmed as u64, Ordering::Relaxed);
        }
        for output in data.chunks_mut(channels * MAX_CALLBACK_FRAMES) {
            let frames = output.len() / channels;
            let mut missing = 0u64;
            for frame in 0..frames {
                match consumer.pop() {
                    Ok(capture) => {
                        left[frame] = capture[0];
                        right[frame] = capture[1];
                    }
                    Err(_) => {
                        left[frame] = 0.0;
                        right[frame] = 0.0;
                        missing += 1;
                    }
                }
            }
            let input_left = &left[..frames];
            let input_right = &right[..frames];
            output_left[..frames].fill(0.0);
            output_right[..frames].fill(0.0);
            let mut callback = AudioCallback {
                inputs: [input_left, input_right],
                outputs: [&mut output_left[..frames], &mut output_right[..frames]],
                nframes: frames as NFrames,
                position: JackPosition {
                    frame: frame_position.min(u32::MAX as u64) as u32,
                    frame_rate: sample_rate,
                    ..JackPosition::default()
                },
                transport_rolling: false,
            };
            (returned.callback.as_mut().expect("callback retained"))(&mut callback);
            for (frame, destination) in output.chunks_exact_mut(channels).enumerate() {
                destination[0] = output_left[frame];
                if channels > 1 {
                    destination[1] = output_right[frame];
                    destination[2..].fill(0.0);
                }
            }
            frame_position = frame_position.wrapping_add(frames as u64);
            if missing != 0 {
                metrics
                    .capture_underruns
                    .fetch_add(missing, Ordering::Relaxed);
            }
        }
        let nanos = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        metrics.callbacks.fetch_add(1, Ordering::Relaxed);
        metrics
            .callback_frames
            .fetch_add(frame_count as u64, Ordering::Relaxed);
        metrics
            .callback_total_nanos
            .fetch_add(nanos, Ordering::Relaxed);
        metrics
            .callback_peak_nanos
            .fetch_max(nanos, Ordering::Relaxed);
        cpu_sample_count += 1;
        cpu_sample_frames += frame_count as u64;
        if cpu_sample_count >= 16 {
            let elapsed = cpu_sample_start
                .take()
                .expect("CPU window starts with its first callback")
                .elapsed()
                .as_nanos() as f64;
            let period = cpu_sample_frames as f64 / sample_rate.max(1) as f64 * 1_000_000_000.0;
            if period > 0.0 {
                metrics.cpu_load_bits.store(
                    ((elapsed / period).clamp(0.0, f32::MAX as f64) as f32).to_bits(),
                    Ordering::Release,
                );
            }
            cpu_sample_count = 0;
            cpu_sample_frames = 0;
        }
    }
}

fn stream_error_callback(
    metrics: Arc<SharedMetrics>,
    realtime_metrics: Option<Arc<RealtimeMetrics>>,
) -> impl FnMut(cpal::Error) + Send + 'static {
    move |error| {
        let xrun = error.kind() == ErrorKind::Xrun;
        metrics.stream_error(xrun);
        if xrun && let Some(metrics) = &realtime_metrics {
            metrics.record_unexplained_xrun();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_negotiation_clamps_to_common_range() {
        assert_eq!(
            choose_buffer_frames(128, Some((64, 512)), Some((32, 256))),
            Some(128)
        );
        assert_eq!(
            choose_buffer_frames(16, Some((64, 512)), Some((32, 256))),
            Some(64)
        );
        assert_eq!(
            choose_buffer_frames(1024, Some((64, 512)), Some((32, 256))),
            Some(256)
        );
        assert_eq!(
            choose_buffer_frames(128, Some((512, 1024)), Some((64, 256))),
            None
        );
    }

    #[test]
    fn capture_overrun_is_bounded_and_observable() {
        let (mut producer, mut consumer) = RingBuffer::new(1);
        let metrics = SharedMetrics::default();
        capture_callback(&[1.0, 2.0, 3.0, 4.0], 2, &mut producer, &metrics);
        assert_eq!(consumer.pop().unwrap(), [1.0, 2.0]);
        assert_eq!(metrics.snapshot().capture_overruns, 1);
    }

    #[test]
    fn playback_callback_records_realtime_callback() {
        let (mut producer, consumer) = RingBuffer::new(4);
        let (callback_sender, mut callback_return) = RingBuffer::new(1);
        producer.push([0.25, -0.25]).unwrap();
        let metrics = Arc::new(SharedMetrics::default());
        let realtime = Arc::new(RealtimeMetrics::new(48_000, 1).unwrap());
        let mut callback = playback_callback(
            consumer,
            Box::new(|audio| {
                audio.outputs[0].copy_from_slice(audio.inputs[0]);
                audio.outputs[1].copy_from_slice(audio.inputs[1]);
            }),
            callback_sender,
            2,
            48_000,
            1,
            metrics,
            Some(Arc::clone(&realtime)),
        );
        let mut output = [0.0; 2];
        callback(
            &mut output,
            &cpal::OutputCallbackInfo::new(cpal::OutputStreamTimestamp {
                callback: cpal::StreamInstant::ZERO,
                playback: cpal::StreamInstant::ZERO,
            }),
        );
        assert_eq!(output, [0.25, -0.25]);
        assert_eq!(realtime.snapshot(48_000, 1).callback_count, 1);
        drop(callback);
        assert!(callback_return.pop().is_ok());
    }

    #[test]
    fn playback_discards_excess_capture_backlog() {
        let (mut producer, consumer) = RingBuffer::new(8);
        let (callback_sender, _callback_return) = RingBuffer::new(1);
        for sample in 0..6 {
            producer.push([sample as f32, sample as f32]).unwrap();
        }
        let mut callback = playback_callback(
            consumer,
            Box::new(|audio| {
                audio.outputs[0].copy_from_slice(audio.inputs[0]);
                audio.outputs[1].copy_from_slice(audio.inputs[1]);
            }),
            callback_sender,
            2,
            48_000,
            2,
            Arc::new(SharedMetrics::default()),
            None,
        );
        let mut output = [0.0; 4];
        callback(
            &mut output,
            &cpal::OutputCallbackInfo::new(cpal::OutputStreamTimestamp {
                callback: cpal::StreamInstant::ZERO,
                playback: cpal::StreamInstant::ZERO,
            }),
        );
        assert_eq!(output, [4.0, 4.0, 5.0, 5.0]);
    }

    #[test]
    fn cpal_xrun_reaches_acceptance_metrics() {
        let metrics = Arc::new(SharedMetrics::default());
        let realtime = Arc::new(RealtimeMetrics::new(48_000, 256).unwrap());
        let mut callback = stream_error_callback(metrics, Some(Arc::clone(&realtime)));
        callback(cpal::Error::with_message(ErrorKind::Xrun, "test xrun"));
        assert_eq!(realtime.snapshot(48_000, 256).unexplained_xruns, 1);
    }
}
