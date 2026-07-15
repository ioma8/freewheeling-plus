//! Optional DSP timing counters, ported from `fweelin_dsp_profile.cc`.

use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Once, OnceLock};
use std::time::Instant;

#[derive(Default)]
struct Counter {
    calls: AtomicU64,
    frames: AtomicU64,
    total_ticks: AtomicU64,
    max_ticks: AtomicU64,
}

struct State {
    enabled: bool,
    output_path: Option<String>,
    snapshot_counter: AtomicU64,
    counters: [Counter; 12], // audio, root, pulse, record, three mixes/write, chains 0..4
}

static START: OnceLock<Instant> = OnceLock::new();
static STATE: OnceLock<State> = OnceLock::new();
static EXIT_HOOK: Once = Once::new();

fn state() -> &'static State {
    STATE.get_or_init(|| {
        let enabled = std::env::var_os("FW_PROFILE_DSP").is_some();
        let output_path =
            std::env::var_os("FW_PROFILE_DSP_OUT").map(|path| path.to_string_lossy().into_owned());
        if enabled {
            EXIT_HOOK.call_once(|| unsafe {
                libc::atexit(report_at_exit);
            });
        }
        State {
            enabled,
            output_path,
            snapshot_counter: AtomicU64::new(0),
            counters: std::array::from_fn(|_| Counter::default()),
        }
    })
}

extern "C" fn report_at_exit() {
    let _ = write_snapshot();
}

fn record(counter: &Counter, ticks: u64, frames: u64) {
    counter.calls.fetch_add(1, Ordering::Relaxed);
    counter.frames.fetch_add(frames, Ordering::Relaxed);
    counter.total_ticks.fetch_add(ticks, Ordering::Relaxed);
    let mut previous = counter.max_ticks.load(Ordering::Relaxed);
    while ticks > previous {
        match counter.max_ticks.compare_exchange_weak(
            previous,
            ticks,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => previous = observed,
        }
    }
}

/// Returns monotonic ticks in nanoseconds, matching the non-macOS C++ implementation.
pub fn now_ticks() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

pub fn enabled() -> bool {
    state().enabled
}

fn record_at(index: usize, ticks: u64, frames: u64) {
    record(&state().counters[index], ticks, frames);
}

pub fn record_audio_callback(ticks: u64, frames: u64) {
    let profile = state();
    record(&profile.counters[0], ticks, frames);
    let snapshot = profile.output_path.is_some()
        && profile
            .snapshot_counter
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
            .is_multiple_of(128);
    if snapshot {
        let _ = write_snapshot();
    }
}
pub fn record_root_process(ticks: u64, frames: u64) {
    record_at(1, ticks, frames);
}
pub fn record_pulse_process(ticks: u64, frames: u64) {
    record_at(2, ticks, frames);
}
pub fn record_record_process(ticks: u64, frames: u64) {
    record_at(3, ticks, frames);
}
pub fn record_record_input_mix(ticks: u64, frames: u64) {
    record_at(4, ticks, frames);
}
pub fn record_record_overdub_mix(ticks: u64, frames: u64) {
    record_at(5, ticks, frames);
}
pub fn record_record_write(ticks: u64, frames: u64) {
    record_at(6, ticks, frames);
}

pub fn record_process_chain(kind: i32, ticks: u64, frames: u64) {
    if (0..=4).contains(&kind) {
        record_at(7 + kind as usize, ticks, frames);
    }
}

fn name(index: usize) -> &'static str {
    [
        "audio_callback",
        "root_process",
        "pulse_process",
        "record_process",
        "record_input_mix",
        "record_overdub_mix",
        "record_write",
        "processchain_default",
        "processchain_global",
        "processchain_global_second",
        "processchain_hipriority",
        "processchain_final",
    ][index]
}

fn report(profile: &State, out: &mut dyn Write) -> io::Result<()> {
    if !profile.enabled {
        return Ok(());
    }
    for (index, counter) in profile.counters.iter().enumerate() {
        let calls = counter.calls.load(Ordering::Relaxed);
        if calls == 0 {
            continue;
        }
        let frames = counter.frames.load(Ordering::Relaxed);
        let total_us = counter.total_ticks.load(Ordering::Relaxed) as f64 / 1000.0;
        let max_us = counter.max_ticks.load(Ordering::Relaxed) as f64 / 1000.0;
        let avg_frame = if frames > 0 {
            total_us * 1000.0 / frames as f64
        } else {
            0.0
        };
        writeln!(
            out,
            "DSP PROFILE: {} calls={} avg_us={:.3} max_us={:.3} avg_ns_per_frame={:.3}",
            name(index),
            calls,
            total_us / calls as f64,
            max_us,
            avg_frame
        )?;
    }
    Ok(())
}

pub fn print_report(out: &mut dyn Write) -> io::Result<()> {
    report(state(), out)
}

fn write_snapshot() -> io::Result<()> {
    let profile = state();
    if !profile.enabled {
        return Ok(());
    }
    if let Some(path) = &profile.output_path {
        let mut file = std::fs::File::create(path)?;
        return report(&profile, &mut file);
    }
    let mut stderr = io::stderr().lock();
    report(&profile, &mut stderr)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ticks_are_monotonic() {
        assert!(now_ticks() <= now_ticks());
    }
    #[test]
    fn process_chain_invalid_kind_is_ignored() {
        record_process_chain(-1, 1, 1);
        record_process_chain(5, 1, 1);
    }
}
