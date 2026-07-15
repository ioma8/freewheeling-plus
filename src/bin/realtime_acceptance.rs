//! Real-hardware realtime acceptance runner.

use freewheeling_plus::audioio::{AudioBackend, AudioCallback};
use freewheeling_plus::realtime_guard::{
    CallbackCountingAllocator, RealtimeMetrics, reset_violation_counters,
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
    backend.close();

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
    let mut json = result.to_json().replacen(
        &format!("\"duration_seconds\": {:.6}", result.duration_seconds),
        &format!("\"duration_seconds\": {:.6}", total_duration),
        1,
    );
    let binding = format!(
        "  \"git_revision\": \"{}\",\n  \"evidence_mode\": \"{}\",\n  \"platform\": \"{}\",\n  \"host\": \"{}\",\n  \"recorded_at_unix\": {},\n  \"requested_duration_seconds\": {},\n  \"prior_elapsed_seconds\": {:.3},\n  \"segment_duration_seconds\": {:.6},\n  \"expected_minimum_callbacks\": {},\n  \"attestation_complete\": {}\n",
        json_escape(&env::var("FWP_ACCEPTANCE_REVISION").unwrap_or_else(|_| "unknown".into())),
        json_escape(
            &env::var("FWP_ACCEPTANCE_EVIDENCE_MODE").unwrap_or_else(|_| "unspecified".into())
        ),
        if cfg!(target_os = "linux") {
            "linux"
        } else {
            "macos"
        },
        json_escape(&env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into())),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        elapsed + duration.as_secs_f64(),
        elapsed,
        result.duration_seconds,
        expected_callbacks,
        total_duration + 0.001 >= elapsed + duration.as_secs_f64(),
    );
    json.insert_str(json.len() - 2, &binding);
    atomic_write(&output, json.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", output.display()))
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

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
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

#[cfg(target_os = "linux")]
type NativeBackend = freewheeling_plus::linux_native::JackAudioMidiBackend;

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
fn jack_buffer_frames(requested: RequestedFormat) -> u32 {
    requested.buffer_frames
}

#[cfg(target_os = "linux")]
fn print_device_diagnostics(requested: RequestedFormat) -> Result<(), String> {
    eprintln!(
        "realtime acceptance request: JACK, sample_rate={} Hz, buffer_frames={}",
        requested.sample_rate, requested.buffer_frames
    );
    Ok(())
}

#[cfg(not(target_os = "linux"))]
type NativeBackend = freewheeling_plus::audio_native_cpal::CpalAudioBackend;

#[cfg(not(target_os = "linux"))]
fn native_backend(requested: RequestedFormat) -> Result<NativeBackend, String> {
    Ok(NativeBackend::new(
        Default::default(),
        cpal_options(requested),
    ))
}

#[cfg(not(target_os = "linux"))]
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

#[cfg(not(target_os = "linux"))]
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

    #[cfg(target_os = "linux")]
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
}
