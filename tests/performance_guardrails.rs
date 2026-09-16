// The crate's own module: a `#[path]` copy would compile a second set of
// counters into this binary and could drift from the real one.
use freewheeling_plus::realtime_guard::{
    CallbackCountingAllocator, InstrumentedMutex, RealtimeMetrics,
};

#[path = "support/scratch.rs"]
mod scratch;

use scratch::ScratchDir;
use std::fs;

#[global_allocator]
static ALLOCATOR: CallbackCountingAllocator = CallbackCountingAllocator;

#[test]
fn callback_violations_are_counted_without_panicking() {
    freewheeling_plus::realtime_guard::reset_violation_counters();
    let metrics = RealtimeMetrics::new(48_000, 128).unwrap();
    let lock = InstrumentedMutex::new(4_u8);
    {
        let _callback = metrics.enter_callback();
        let allocation = Box::new(9_u8);
        assert_eq!(*allocation, 9);
        assert_eq!(*lock.try_lock().unwrap(), 4);
    }
    assert!(freewheeling_plus::realtime_guard::callback_allocations() >= 1);
    // `try_lock` never blocks, so it must not be reported as a violation.
    assert_eq!(freewheeling_plus::realtime_guard::blocking_lock_attempts(), 0);
    {
        let _callback = metrics.enter_callback();
        // A blocking `lock` inside a callback is the pattern the counter
        // exists for.
        assert_eq!(*lock.lock().unwrap(), 4);
    }
    assert_eq!(freewheeling_plus::realtime_guard::blocking_lock_attempts(), 1);
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
    let result = metrics.snapshot();
    assert_eq!(result.callback_count, 5);
    assert_eq!(result.unexplained_xruns, 1);
    assert_eq!(result.callback_deadline_us, 5_333.333);
    assert!(result.rss_peak_bytes >= result.rss_start_bytes);

    let scratch = ScratchDir::new("performance-result");
    let path = scratch.join("metrics.json");
    result.write_json(&path).unwrap();
    let json = fs::read_to_string(&path).unwrap();
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
