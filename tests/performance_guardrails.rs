#[path = "../src/realtime_guard.rs"]
mod realtime_guard;

use realtime_guard::{CallbackCountingAllocator, InstrumentedMutex, RealtimeMetrics};
use std::fs;

#[global_allocator]
static ALLOCATOR: CallbackCountingAllocator = CallbackCountingAllocator;

#[test]
fn callback_violations_are_counted_without_panicking() {
    realtime_guard::reset_violation_counters();
    let metrics = RealtimeMetrics::new(48_000, 128).unwrap();
    let lock = InstrumentedMutex::new(4_u8);
    {
        let _callback = metrics.enter_callback();
        let allocation = Box::new(9_u8);
        assert_eq!(*allocation, 9);
        assert_eq!(*lock.try_lock().unwrap(), 4);
    }
    assert!(realtime_guard::callback_allocations() >= 1);
    assert_eq!(realtime_guard::blocking_lock_attempts(), 1);
    assert_eq!(*lock.lock().unwrap(), 4);
    assert_eq!(lock.into_inner().unwrap(), 4);
}

#[test]
fn snapshot_and_json_match_performance_schema_shape() {
    let metrics = RealtimeMetrics::new(48_000, 256).unwrap();
    for _ in 0..5 {
        let _callback = metrics.enter_callback();
        std::hint::black_box(1 + 1);
    }
    metrics.sample_rss().unwrap();
    metrics.record_unexplained_xrun();
    let result = metrics.snapshot(48_000, 256);
    assert_eq!(result.callback_count, 5);
    assert_eq!(result.unexplained_xruns, 1);
    assert_eq!(result.callback_deadline_us, 5_333.333);
    assert!(result.rss_peak_bytes >= result.rss_start_bytes);

    let path = std::env::temp_dir().join(format!(
        "freewheeling-performance-{}.json",
        std::process::id()
    ));
    result.write_json(&path).unwrap();
    let json = fs::read_to_string(&path).unwrap();
    fs::remove_file(path).unwrap();
    for field in [
        "schema_version",
        "duration_seconds",
        "callback_p99_us",
        "callback_allocations",
        "blocking_lock_attempts",
        "unexplained_xruns",
        "rss_peak_bytes",
    ] {
        assert!(json.contains(&format!("\"{field}\"")));
    }
}

#[test]
fn invalid_metric_configuration_is_rejected() {
    assert!(RealtimeMetrics::new(0, 128).is_err());
    assert!(RealtimeMetrics::new(48_000, 0).is_err());
}
