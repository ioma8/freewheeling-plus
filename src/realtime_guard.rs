//! Debug/acceptance instrumentation for audio callback safety and timing.
//!
//! Install [`CallbackCountingAllocator`] as the process global allocator in an
//! acceptance binary, enter [`CallbackGuard`] at the very start of each audio
//! callback, and use [`InstrumentedMutex`] where a lock might accidentally
//! become reachable from that callback. Recording uses atomics and thread-local
//! state only; it does not allocate or lock on the callback path.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LockResult, Mutex, MutexGuard, TryLockResult};
use std::time::Instant;

const HISTOGRAM_BUCKETS: usize = 4096;

thread_local! {
    static CALLBACK_DEPTH: Cell<u32> = const { Cell::new(0) };
}

static CALLBACK_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BLOCKING_LOCK_ATTEMPTS: AtomicU64 = AtomicU64::new(0);

fn in_callback() -> bool {
    CALLBACK_DEPTH.with(|depth| depth.get() != 0)
}

/// Global allocator wrapper which counts allocation operations in callbacks.
///
/// Acceptance binaries should declare:
/// `#[global_allocator] static ALLOC: CallbackCountingAllocator = CallbackCountingAllocator;`
pub struct CallbackCountingAllocator;

unsafe impl GlobalAlloc for CallbackCountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if in_callback() {
            CALLBACK_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if in_callback() {
            CALLBACK_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if in_callback() {
            CALLBACK_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// A mutex that makes every callback-thread locking attempt observable.
pub struct InstrumentedMutex<T>(Mutex<T>);

impl<T> InstrumentedMutex<T> {
    pub const fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        if in_callback() {
            BLOCKING_LOCK_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        }
        self.0.lock()
    }

    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        if in_callback() {
            BLOCKING_LOCK_ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        }
        self.0.try_lock()
    }

    pub fn into_inner(self) -> LockResult<T> {
        self.0.into_inner()
    }
}

/// Lock-free callback timing and xrun measurements shared with a control thread.
pub struct RealtimeMetrics {
    started: Instant,
    callback_deadline_ns: u64,
    callbacks: AtomicU64,
    deadline_misses: AtomicU64,
    unexplained_xruns: AtomicU64,
    histogram_us: [AtomicU64; HISTOGRAM_BUCKETS],
    rss_start_bytes: u64,
    rss_peak_bytes: AtomicU64,
}

impl RealtimeMetrics {
    pub fn new(sample_rate_hz: u32, buffer_frames: u32) -> io::Result<Self> {
        if sample_rate_hz == 0 || buffer_frames == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sample rate and buffer frames must be non-zero",
            ));
        }
        let rss = resident_set_bytes()?;
        Ok(Self {
            started: Instant::now(),
            callback_deadline_ns: u64::from(buffer_frames) * 1_000_000_000
                / u64::from(sample_rate_hz),
            callbacks: AtomicU64::new(0),
            deadline_misses: AtomicU64::new(0),
            unexplained_xruns: AtomicU64::new(0),
            histogram_us: std::array::from_fn(|_| AtomicU64::new(0)),
            rss_start_bytes: rss,
            rss_peak_bytes: AtomicU64::new(rss),
        })
    }

    pub fn enter_callback(&self) -> CallbackGuard<'_> {
        CALLBACK_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        CallbackGuard {
            metrics: self,
            started: Instant::now(),
        }
    }

    pub fn record_unexplained_xrun(&self) {
        self.unexplained_xruns.fetch_add(1, Ordering::Relaxed);
    }

    /// Sample RSS from a non-realtime monitoring thread.
    pub fn sample_rss(&self) -> io::Result<u64> {
        let rss = resident_set_bytes()?;
        self.rss_peak_bytes.fetch_max(rss, Ordering::Relaxed);
        Ok(rss)
    }

    pub fn snapshot(&self, sample_rate_hz: u32, buffer_frames: u32) -> PerformanceResult {
        let callbacks = self.callbacks.load(Ordering::Relaxed);
        let target = callbacks.saturating_mul(99).div_ceil(100);
        let mut cumulative = 0;
        let mut p99 = 0;
        for (micros, count) in self.histogram_us.iter().enumerate() {
            cumulative += count.load(Ordering::Relaxed);
            if cumulative >= target.max(1) {
                p99 = micros as u64;
                break;
            }
        }
        PerformanceResult {
            schema_version: 1,
            sample_rate_hz,
            buffer_frames,
            duration_seconds: self.started.elapsed().as_secs_f64(),
            callback_p99_us: p99 as f64,
            callback_deadline_us: self.callback_deadline_ns as f64 / 1_000.0,
            callback_allocations: CALLBACK_ALLOCATIONS.load(Ordering::Relaxed),
            blocking_lock_attempts: BLOCKING_LOCK_ATTEMPTS.load(Ordering::Relaxed),
            unexplained_xruns: self.unexplained_xruns.load(Ordering::Relaxed),
            rss_start_bytes: self.rss_start_bytes,
            rss_peak_bytes: self.rss_peak_bytes.load(Ordering::Relaxed),
            callback_count: callbacks,
            deadline_misses: self.deadline_misses.load(Ordering::Relaxed),
        }
    }
}

pub struct CallbackGuard<'a> {
    metrics: &'a RealtimeMetrics,
    started: Instant,
}

impl Drop for CallbackGuard<'_> {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        let nanos = elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
        let bucket = usize::try_from(elapsed.as_micros())
            .unwrap_or(usize::MAX)
            .min(HISTOGRAM_BUCKETS - 1);
        self.metrics.histogram_us[bucket].fetch_add(1, Ordering::Relaxed);
        self.metrics.callbacks.fetch_add(1, Ordering::Relaxed);
        if nanos > self.metrics.callback_deadline_ns {
            self.metrics.deadline_misses.fetch_add(1, Ordering::Relaxed);
        }
        CALLBACK_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

#[derive(Clone, Debug)]
pub struct PerformanceResult {
    pub schema_version: u32,
    pub sample_rate_hz: u32,
    pub buffer_frames: u32,
    pub duration_seconds: f64,
    pub callback_p99_us: f64,
    pub callback_deadline_us: f64,
    pub callback_allocations: u64,
    pub blocking_lock_attempts: u64,
    pub unexplained_xruns: u64,
    pub rss_start_bytes: u64,
    pub rss_peak_bytes: u64,
    pub callback_count: u64,
    pub deadline_misses: u64,
}

impl PerformanceResult {
    pub fn write_json(&self, path: impl AsRef<Path>) -> io::Result<()> {
        fs::write(path, self.to_json())
    }

    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{\n  \"schema_version\": {},\n  \"sample_rate_hz\": {},\n",
                "  \"buffer_frames\": {},\n  \"duration_seconds\": {:.6},\n",
                "  \"callback_p99_us\": {:.3},\n  \"callback_deadline_us\": {:.3},\n",
                "  \"callback_allocations\": {},\n  \"blocking_lock_attempts\": {},\n",
                "  \"unexplained_xruns\": {},\n  \"rss_start_bytes\": {},\n",
                "  \"rss_peak_bytes\": {},\n  \"callback_count\": {},\n",
                "  \"deadline_misses\": {}\n}}\n"
            ),
            self.schema_version,
            self.sample_rate_hz,
            self.buffer_frames,
            self.duration_seconds,
            self.callback_p99_us,
            self.callback_deadline_us,
            self.callback_allocations,
            self.blocking_lock_attempts,
            self.unexplained_xruns,
            self.rss_start_bytes,
            self.rss_peak_bytes,
            self.callback_count,
            self.deadline_misses,
        )
    }
}

#[cfg(target_os = "linux")]
fn resident_set_bytes() -> io::Result<u64> {
    let statm = fs::read_to_string("/proc/self/statm")?;
    let pages = statm.split_whitespace().nth(1).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "missing resident pages in /proc/self/statm",
        )
    })?;
    let pages: u64 = pages
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid resident page count"))?;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pages.saturating_mul(page_size as u64))
}

#[cfg(not(target_os = "linux"))]
fn resident_set_bytes() -> io::Result<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let bytes = unsafe { usage.assume_init() }.ru_maxrss as u64;
    #[cfg(target_os = "macos")]
    return Ok(bytes);
    #[cfg(not(target_os = "macos"))]
    return Ok(bytes.saturating_mul(1024));
}

/// Reset process-global violation counters before starting an acceptance run.
pub fn reset_violation_counters() {
    CALLBACK_ALLOCATIONS.store(0, Ordering::Relaxed);
    BLOCKING_LOCK_ATTEMPTS.store(0, Ordering::Relaxed);
}

pub fn callback_allocations() -> u64 {
    CALLBACK_ALLOCATIONS.load(Ordering::Relaxed)
}
pub fn blocking_lock_attempts() -> u64 {
    BLOCKING_LOCK_ATTEMPTS.load(Ordering::Relaxed)
}
