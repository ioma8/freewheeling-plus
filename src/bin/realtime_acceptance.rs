//! Real-hardware realtime acceptance runner.

use freewheeling_plus::audioio::{AudioBackend, AudioCallback};
#[cfg(target_os = "macos")]
use freewheeling_plus::audioio::{AudioCallbackFn, BackendInfo};
use freewheeling_plus::realtime_guard::{
    CallbackCountingAllocator, PerformanceResult, RealtimeMetrics, reset_violation_counters,
};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

#[global_allocator]
static ALLOCATOR: CallbackCountingAllocator = CallbackCountingAllocator;

const DEFAULT_SECONDS: u64 = 10;
const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_BUFFER_FRAMES: u32 = 256;
const RSS_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RequestedFormat {
    sample_rate: u32,
    buffer_frames: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("realtime acceptance failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let output = required_output_path()?;
    let duration = requested_duration()?;
    let requested = requested_format()?;
    let prior_elapsed_seconds = prior_elapsed_seconds()?;
    match fs::remove_file(&output) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot remove stale result {}: {error}",
                output.display()
            ));
        }
    }
    print_device_diagnostics(requested)?;
    let mut backend = native_backend(requested)?;
    let info = backend.open("freewheeling-realtime-acceptance")?;
    eprintln!(
        "realtime acceptance negotiated: sample_rate={} Hz, buffer_frames={}",
        info.sample_rate, info.buffer_size
    );
    if info.sample_rate != requested.sample_rate || info.buffer_size != requested.buffer_frames {
        backend.close();
        return Err(format!(
            "negotiated format differs from request: requested {} Hz / {} frames, got {} Hz / {} frames",
            requested.sample_rate, requested.buffer_frames, info.sample_rate, info.buffer_size
        ));
    }
    let metrics = Arc::new(
        RealtimeMetrics::new(info.sample_rate, info.buffer_size)
            .map_err(|error| format!("cannot initialize realtime metrics: {error}"))?,
    );
    reset_violation_counters();
    backend.set_realtime_metrics(Arc::clone(&metrics));
    backend.activate(Box::new(passthrough))?;

    let started = Instant::now();
    while started.elapsed() < duration {
        thread::sleep(RSS_SAMPLE_INTERVAL.min(duration.saturating_sub(started.elapsed())));
        metrics
            .sample_rss()
            .map_err(|error| format!("cannot sample resident memory: {error}"))?;
    }
    // Opt-in macOS aggregate contract: capture and playback frame counts must
    // stay matched with zero dropped/fabricated input frames, and the private
    // aggregate must disappear once FreeWheeling closes it.
    let aggregate_device = capture_aggregate_device(&backend);
    let stream_diagnostics = capture_stream_diagnostics(&backend);
    backend.close();
    verify_aggregate_cleanup(aggregate_device, stream_diagnostics)?;

    let result = metrics.snapshot(info.sample_rate, info.buffer_size);
    if result.callback_count == 0 {
        return Err(
            "native backend produced no audio callbacks; refusing to write a result".into(),
        );
    }
    if result.duration_seconds + 0.001 < duration.as_secs_f64() {
        return Err("native audio did not run for the requested duration".into());
    }
    let elapsed = prior_elapsed_seconds;
    let total_duration = result.duration_seconds + elapsed;
    let expected_callbacks =
        expected_callback_count(result.duration_seconds, info.sample_rate, info.buffer_size);
    if result.callback_count == 0 || u64::from(result.buffer_frames) == 0 {
        return Err("invalid callback metrics".into());
    }
    if result.callback_count < expected_callbacks {
        return Err(format!(
            "audio callback count too low: observed {}, expected at least {}",
            result.callback_count, expected_callbacks
        ));
    }
    let json = attestation_json(
        &result,
        total_duration,
        elapsed,
        duration,
        expected_callbacks,
    )?;
    atomic_write(&output, json.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", output.display()))
}

/// Build the complete attestation JSON document: the measured performance
/// metrics plus the provenance fields recorded around the acceptance run.
/// Constructed as a `serde_json::Value` so the output always parses, even
/// when the set of provenance fields changes.
fn attestation_json(
    result: &PerformanceResult,
    total_duration: f64,
    elapsed: f64,
    duration: Duration,
    expected_callbacks: u64,
) -> Result<String, String> {
    let mut document: serde_json::Value = serde_json::from_str(&result.to_json())
        .map_err(|error| format!("internal performance JSON is invalid: {error}"))?;
    let fields = document
        .as_object_mut()
        .ok_or_else(|| String::from("internal performance JSON is not an object"))?;
    fields.insert(
        "duration_seconds".into(),
        serde_json::json!(total_duration),
    );
    fields.insert(
        "git_revision".into(),
        serde_json::json!(env::var("FWP_ACCEPTANCE_REVISION")
            .unwrap_or_else(|_| "unknown".into())),
    );
    fields.insert(
        "evidence_mode".into(),
        serde_json::json!(env::var("FWP_ACCEPTANCE_EVIDENCE_MODE")
            .unwrap_or_else(|_| "unspecified".into())),
    );
    fields.insert(
        "platform".into(),
        serde_json::json!(if cfg!(target_os = "linux") {
            "linux"
        } else {
            "macos"
        }),
    );
    fields.insert(
        "host".into(),
        serde_json::json!(env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into())),
    );
    fields.insert(
        "recorded_at_unix".into(),
        serde_json::json!(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()),
    );
    fields.insert(
        "requested_duration_seconds".into(),
        serde_json::json!(elapsed + duration.as_secs_f64()),
    );
    fields.insert(
        "prior_elapsed_seconds".into(),
        serde_json::json!(elapsed),
    );
    fields.insert(
        "segment_duration_seconds".into(),
        serde_json::json!(result.duration_seconds),
    );
    fields.insert(
        "expected_minimum_callbacks".into(),
        serde_json::json!(expected_callbacks),
    );
    fields.insert(
        "attestation_complete".into(),
        serde_json::json!(total_duration + 0.001 >= elapsed + duration.as_secs_f64()),
    );
    serde_json::to_string_pretty(&document)
        .map_err(|error| format!("cannot serialize performance result: {error}"))
}

fn prior_elapsed_seconds() -> Result<f64, String> {
    let value = env::var("FWP_REALTIME_ELAPSED_SECONDS").unwrap_or_else(|_| "0".into());
    parse_elapsed_seconds(&value)
}

fn parse_elapsed_seconds(value: &str) -> Result<f64, String> {
    let elapsed = value.parse::<f64>().map_err(|_| {
        "FWP_REALTIME_ELAPSED_SECONDS must be a finite non-negative number".to_string()
    })?;
    if !elapsed.is_finite() || elapsed < 0.0 {
        return Err("FWP_REALTIME_ELAPSED_SECONDS is outside the requested duration".into());
    }
    Ok(elapsed)
}

fn expected_callback_count(seconds: f64, sample_rate: u32, frames: u32) -> u64 {
    (seconds * f64::from(sample_rate) / f64::from(frames) * 0.95).floor() as u64
}

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, contents)?;
    fs::rename(temporary, path)
}

fn passthrough(callback: &mut AudioCallback<'_>) {
    for channel in 0..callback.outputs.len() {
        callback.outputs[channel].copy_from_slice(callback.inputs[channel]);
    }
}

fn required_output_path() -> Result<PathBuf, String> {
    env::var_os("FWP_PERFORMANCE_RESULT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "FWP_PERFORMANCE_RESULT must name the JSON output path".into())
}

fn requested_duration() -> Result<Duration, String> {
    let seconds = env::var("FWP_REALTIME_ACCEPTANCE_SECONDS")
        .unwrap_or_else(|_| DEFAULT_SECONDS.to_string())
        .parse::<u64>()
        .map_err(|_| "FWP_REALTIME_ACCEPTANCE_SECONDS must be an integer".to_string())?;
    if seconds == 0 {
        return Err("FWP_REALTIME_ACCEPTANCE_SECONDS must be non-zero".into());
    }
    Ok(Duration::from_secs(seconds))
}

fn requested_format() -> Result<RequestedFormat, String> {
    requested_format_from(
        optional_env("FWP_REALTIME_SAMPLE_RATE")?.as_deref(),
        optional_env("FWP_REALTIME_BUFFER_FRAMES")?.as_deref(),
    )
}

fn optional_env(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(format!("{name} must contain UTF-8 text")),
    }
}

fn requested_format_from(
    sample_rate: Option<&str>,
    buffer_frames: Option<&str>,
) -> Result<RequestedFormat, String> {
    let sample_rate = parse_positive_u32(
        "FWP_REALTIME_SAMPLE_RATE",
        sample_rate
            .map(str::to_owned)
            .unwrap_or_else(|| DEFAULT_SAMPLE_RATE.to_string())
            .as_str(),
    )?;
    let buffer_frames = parse_positive_u32(
        "FWP_REALTIME_BUFFER_FRAMES",
        buffer_frames
            .map(str::to_owned)
            .unwrap_or_else(|| DEFAULT_BUFFER_FRAMES.to_string())
            .as_str(),
    )?;
    if !matches!(buffer_frames, 128 | 256) {
        return Err("FWP_REALTIME_BUFFER_FRAMES must be either 128 or 256".into());
    }
    Ok(RequestedFormat {
        sample_rate,
        buffer_frames,
    })
}

fn parse_positive_u32(name: &str, value: &str) -> Result<u32, String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{name} must be non-zero"));
    }
    Ok(parsed)
}

#[cfg(all(target_os = "linux", feature = "jack"))]
type NativeBackend = freewheeling_plus::jack::JackAudioMidiBackend;

#[cfg(all(target_os = "linux", feature = "jack"))]
fn native_backend(requested: RequestedFormat) -> Result<NativeBackend, String> {
    let (client, _) = jack::Client::new(
        "freewheeling-realtime-acceptance-config",
        jack::ClientOptions::NO_START_SERVER,
    )
    .map_err(|error| format!("cannot open JACK configuration client: {error}"))?;
    client
        .set_buffer_size(jack_buffer_frames(requested))
        .map_err(|error| format!("cannot request JACK buffer size: {error}"))?;
    eprintln!(
        "JACK server: sample_rate={} Hz, buffer_frames={}",
        client.sample_rate(),
        client.buffer_size()
    );
    if client.sample_rate() != requested.sample_rate
        || client.buffer_size() != requested.buffer_frames
    {
        return Err(format!(
            "JACK server differs from request: requested {} Hz / {} frames, got {} Hz / {} frames",
            requested.sample_rate,
            requested.buffer_frames,
            client.sample_rate(),
            client.buffer_size()
        ));
    }
    Ok(NativeBackend::new(1, 1))
}

#[cfg(all(target_os = "linux", feature = "jack"))]
fn jack_buffer_frames(requested: RequestedFormat) -> u32 {
    requested.buffer_frames
}

#[cfg(all(target_os = "linux", feature = "jack"))]
fn print_device_diagnostics(requested: RequestedFormat) -> Result<(), String> {
    eprintln!(
        "realtime acceptance request: JACK, sample_rate={} Hz, buffer_frames={}",
        requested.sample_rate, requested.buffer_frames
    );
    Ok(())
}

/// macOS acceptance backend. `FWP_ACCEPTANCE_BACKEND=audiounit` (opt-in)
/// exercises the private CoreAudio Aggregate Device path that the default
/// MacBook mic/speaker route uses; `cpal` (default) keeps the split-clock
/// fallback behavior.
#[cfg(target_os = "macos")]
enum NativeBackend {
    Cpal(Box<freewheeling_plus::audio_native_cpal::CpalAudioBackend>),
    AudioUnit(Box<freewheeling_plus::macos_audio_unit::MacosAudioUnitBackend>),
}

#[cfg(target_os = "macos")]
impl NativeBackend {
    fn open(&mut self, name: &str) -> Result<BackendInfo, String> {
        match self {
            NativeBackend::Cpal(backend) => backend.open(name),
            NativeBackend::AudioUnit(backend) => backend.open(name),
        }
    }

    fn activate(&mut self, callback: AudioCallbackFn) -> Result<(), String> {
        match self {
            NativeBackend::Cpal(backend) => backend.activate(callback),
            NativeBackend::AudioUnit(backend) => backend.activate(callback),
        }
    }

    fn close(&mut self) {
        match self {
            NativeBackend::Cpal(backend) => backend.close(),
            NativeBackend::AudioUnit(backend) => backend.close(),
        }
    }

    fn set_realtime_metrics(&mut self, metrics: Arc<RealtimeMetrics>) {
        match self {
            NativeBackend::Cpal(backend) => backend.set_realtime_metrics(metrics),
            NativeBackend::AudioUnit(backend) => backend.set_realtime_metrics(metrics),
        }
    }
}

#[cfg(target_os = "macos")]
fn native_backend(requested: RequestedFormat) -> Result<NativeBackend, String> {
    let kind = std::env::var("FWP_ACCEPTANCE_BACKEND").unwrap_or_else(|_| "cpal".into());
    match kind.to_lowercase().as_str() {
        "cpal" => Ok(NativeBackend::Cpal(Box::new(
            freewheeling_plus::audio_native_cpal::CpalAudioBackend::new(
                Default::default(),
                cpal_options(requested),
            ),
        ))),
        "audiounit" => Ok(NativeBackend::AudioUnit(Box::new(
            freewheeling_plus::macos_audio_unit::MacosAudioUnitBackend::new(
                Default::default(),
                cpal_options(requested),
            ),
        ))),
        other => Err(format!(
            "unknown FWP_ACCEPTANCE_BACKEND {other:?} (expected \"cpal\" or \"audiounit\")"
        )),
    }
}

#[cfg(target_os = "macos")]
fn print_device_diagnostics(requested: RequestedFormat) -> Result<(), String> {
    use freewheeling_plus::audio_native_cpal::CpalAudioBackend;

    eprintln!(
        "realtime acceptance request: CPAL, sample_rate={} Hz, buffer_frames={}",
        requested.sample_rate, requested.buffer_frames
    );
    for device in CpalAudioBackend::discover_input_devices()? {
        eprintln!(
            "CPAL input device: id={:?}, name={:?}, default={}",
            device.id, device.name, device.is_default
        );
    }
    for device in CpalAudioBackend::discover_output_devices()? {
        eprintln!(
            "CPAL output device: id={:?}, name={:?}, default={}",
            device.id, device.name, device.is_default
        );
    }
    Ok(())
}

/// Aggregate device owned by the backend, for the opt-in macOS cleanup check.
#[cfg(target_os = "macos")]
fn capture_aggregate_device(backend: &NativeBackend) -> Option<u32> {
    match backend {
        NativeBackend::AudioUnit(backend) => backend.owned_aggregate_device(),
        NativeBackend::Cpal(_) => None,
    }
}

/// Reliable-path frame diagnostics (AudioUnit aggregate route only).
#[cfg(target_os = "macos")]
fn capture_stream_diagnostics(
    backend: &NativeBackend,
) -> Option<freewheeling_plus::audio_native_cpal::CpalStreamDiagnostics> {
    match backend {
        NativeBackend::AudioUnit(backend) => Some(backend.status().stream),
        NativeBackend::Cpal(_) => None,
    }
}

/// Verify the aggregate acceptance contract after `close`: every captured
/// frame reached the DSP exactly once (matched capture/playback counts, zero
/// missing/trimmed/mismatched frames) and the private aggregate device has
/// disappeared from the HAL device list.
#[cfg(target_os = "macos")]
fn verify_aggregate_cleanup(
    aggregate_device: Option<u32>,
    diagnostics: Option<freewheeling_plus::audio_native_cpal::CpalStreamDiagnostics>,
) -> Result<(), String> {
    let Some(device) = aggregate_device else {
        return Ok(());
    };
    let Some(stream) = diagnostics else {
        return Err("AudioUnit acceptance run produced no stream diagnostics".into());
    };
    if stream.capture_frames != stream.playback_frames {
        return Err(format!(
            "capture/playback frame counts diverged: capture={} playback={}",
            stream.capture_frames, stream.playback_frames
        ));
    }
    if stream.missing_frames != 0 || stream.trimmed_frames != 0 {
        return Err(format!(
            "duplex callback dropped or fabricated frames: missing={} trimmed={}",
            stream.missing_frames, stream.trimmed_frames
        ));
    }
    if stream.frame_size_mismatches != 0 {
        return Err(format!(
            "duplex callback observed {} buffer size mismatches",
            stream.frame_size_mismatches
        ));
    }
    // Destruction is asynchronous inside the HAL; poll for disappearance.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let present = hal_device_ids()?.contains(&device);
        if !present {
            eprintln!(
                "[AUDIO-DIAG] acceptance: private aggregate device {device} removed after close"
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "private aggregate device {device} still present after close"
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "macos")]
fn hal_device_ids() -> Result<Vec<u32>, String> {
    use coreaudio_sys::{
        AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectPropertyAddress,
        AudioObjectID,
    };
    // kAudioObjectSystemObject, defined locally to avoid SDK binding churn.
    const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
    const K_AUDIO_HARDWARE_PROPERTY_DEVICES: u32 = 0x6465_7623; // 'dev#'
    let address = AudioObjectPropertyAddress {
        mSelector: K_AUDIO_HARDWARE_PROPERTY_DEVICES,
        mScope: 0,
        mElement: 0,
    };
    let mut size = 0u32;
    // SAFETY: first call with a null data pointer only requests the size.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &address,
            0,
            std::ptr::null(),
            &mut size,
        )
    };
    if status != 0 {
        return Err(format!(
            "cannot query CoreAudio device list size (OSStatus {status})"
        ));
    }
    let count = size as usize / std::mem::size_of::<AudioObjectID>();
    let mut ids = vec![0u32; count];
    // SAFETY: the buffer matches the size CoreAudio reported.
    let status = unsafe {
        AudioObjectGetPropertyData(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            ids.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return Err(format!(
            "cannot query CoreAudio device list (OSStatus {status})"
        ));
    }
    Ok(ids)
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", feature = "jack"))))]
type NativeBackend = freewheeling_plus::audio_native_cpal::CpalAudioBackend;

#[cfg(not(any(target_os = "macos", all(target_os = "linux", feature = "jack"))))]
fn native_backend(requested: RequestedFormat) -> Result<NativeBackend, String> {
    Ok(NativeBackend::new(
        Default::default(),
        cpal_options(requested),
    ))
}

#[cfg(not(any(target_os = "macos", all(target_os = "linux", feature = "jack"))))]
fn print_device_diagnostics(requested: RequestedFormat) -> Result<(), String> {
    use freewheeling_plus::audio_native_cpal::CpalAudioBackend;

    eprintln!(
        "realtime acceptance request: CPAL, sample_rate={} Hz, buffer_frames={}",
        requested.sample_rate, requested.buffer_frames
    );
    for device in CpalAudioBackend::discover_input_devices()? {
        eprintln!(
            "CPAL input device: id={:?}, name={:?}, default={}",
            device.id, device.name, device.is_default
        );
    }
    for device in CpalAudioBackend::discover_output_devices()? {
        eprintln!(
            "CPAL output device: id={:?}, name={:?}, default={}",
            device.id, device.name, device.is_default
        );
    }
    Ok(())
}

/// Aggregate device owned by the backend, for the opt-in cleanup check.
#[cfg(not(target_os = "macos"))]
fn capture_aggregate_device(_backend: &NativeBackend) -> Option<u32> {
    None
}

/// Reliable-path frame diagnostics (macOS AudioUnit aggregate route only).
#[cfg(not(target_os = "macos"))]
fn capture_stream_diagnostics(
    _backend: &NativeBackend,
) -> Option<freewheeling_plus::audio_native_cpal::CpalStreamDiagnostics> {
    None
}

/// Aggregate acceptance verification is macOS/AudioUnit-specific.
#[cfg(not(target_os = "macos"))]
fn verify_aggregate_cleanup(
    _aggregate_device: Option<u32>,
    _diagnostics: Option<freewheeling_plus::audio_native_cpal::CpalStreamDiagnostics>,
) -> Result<(), String> {
    Ok(())
}

#[cfg(not(all(target_os = "linux", feature = "jack")))]
fn cpal_options(
    requested: RequestedFormat,
) -> freewheeling_plus::audio_native_cpal::CpalAudioOptions {
    freewheeling_plus::audio_native_cpal::CpalAudioOptions {
        preferred_sample_rate: requested.sample_rate,
        preferred_buffer_frames: requested.buffer_frames,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_128_frame_request_exactly() {
        assert_eq!(
            requested_format_from(Some("48000"), Some("128")).unwrap(),
            RequestedFormat {
                sample_rate: 48_000,
                buffer_frames: 128,
            }
        );
    }

    #[test]
    fn rejects_invalid_realtime_format_values() {
        assert!(requested_format_from(Some("48k"), Some("128")).is_err());
        assert!(requested_format_from(Some("48000"), Some("0")).is_err());
        assert!(requested_format_from(Some("48000"), Some("512")).is_err());
    }

    #[cfg(not(all(target_os = "linux", feature = "jack")))]
    #[test]
    fn requests_128_frames_from_cpal() {
        let options = cpal_options(RequestedFormat {
            sample_rate: 48_000,
            buffer_frames: 128,
        });
        assert_eq!(options.preferred_sample_rate, 48_000);
        assert_eq!(options.preferred_buffer_frames, 128);
    }

    #[test]
    fn resume_elapsed_is_bounded_and_finite() {
        assert_eq!(parse_elapsed_seconds("3600").unwrap(), 3600.0);
        assert!(parse_elapsed_seconds("NaN").is_err());
    }

    #[test]
    fn callback_floor_scales_with_segment_duration_and_format() {
        assert_eq!(expected_callback_count(1.0, 48_000, 128), 356);
        assert_eq!(expected_callback_count(1.0, 48_000, 256), 178);
    }

    #[cfg(all(target_os = "linux", feature = "jack"))]
    #[test]
    fn requests_128_frames_from_jack() {
        assert_eq!(
            jack_buffer_frames(RequestedFormat {
                sample_rate: 48_000,
                buffer_frames: 128,
            }),
            128
        );
    }

    #[test]
    fn attestation_json_parses_and_carries_provenance() {
        let result = PerformanceResult {
            schema_version: 1,
            sample_rate_hz: 48_000,
            buffer_frames: 256,
            duration_seconds: 3.0,
            callback_p99_us: 200.0,
            callback_deadline_us: 5333.0,
            callback_allocations: 0,
            blocking_lock_attempts: 0,
            unexplained_xruns: 0,
            rss_start_bytes: 1000,
            rss_peak_bytes: 2000,
            callback_count: 562,
            deadline_misses: 0,
        };
        // SAFETY: test-only; no other code reads these variables concurrently.
        unsafe {
            std::env::set_var("FWP_ACCEPTANCE_REVISION", "deadbeef");
            std::env::set_var("FWP_ACCEPTANCE_EVIDENCE_MODE", "virtual-jack");
        }
        let json = attestation_json(&result, 3.0, 0.0, Duration::from_secs(3), 500).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object["git_revision"], "deadbeef");
        assert_eq!(object["evidence_mode"], "virtual-jack");
        assert_eq!(object["attestation_complete"], true);
        assert_eq!(object["expected_minimum_callbacks"], 500);
        assert_eq!(object["sample_rate_hz"], 48_000);
    }
}
