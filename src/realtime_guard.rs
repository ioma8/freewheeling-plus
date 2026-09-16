//! Debug/acceptance instrumentation for audio callback safety and timing.
//!
//! Install [`CallbackCountingAllocator`] as the process global allocator in an
//! acceptance binary, enter [`CallbackGuard`] at the very start of each audio
//! callback, and use [`InstrumentedMutex`] where a lock might accidentally
//! become reachable from that callback. Recording uses atomics and thread-local
//! state only; it does not allocate or lock on the callback path.

use serde::Serialize;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::io;
use std::marker::PhantomData;
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

/// Reads a const-initialized, destructor-free thread-local, so this never
/// allocates and never panics even when called from inside the global
/// allocator. Keep `CALLBACK_DEPTH` free of any payload that needs `Drop`.
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

    /// Non-blocking lock for callback paths.
    ///
    /// Not counted as a blocking attempt: `try_lock` never blocks, and
    /// counting it would report the recommended non-blocking pattern as a
    /// violation.
    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        self.0.try_lock()
    }

    pub fn into_inner(self) -> LockResult<T> {
        self.0.into_inner()
    }
}

/// Lock-free callback timing and xrun measurements shared with a control thread.
pub struct RealtimeMetrics {
    started: Instant,
    sample_rate_hz: u32,
    buffer_frames: u32,
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
            sample_rate_hz,
            buffer_frames,
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
            _not_send: PhantomData,
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

    /// Take a report of everything measured so far.
    ///
    /// The stream format comes from the constructor, so the report cannot mix
    /// a deadline derived from one format with fields from another.
    pub fn snapshot(&self) -> PerformanceResult {
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
            sample_rate_hz: self.sample_rate_hz,
            buffer_frames: self.buffer_frames,
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
    /// Pins the guard to the thread that called `enter_callback`: `Drop`
    /// decrements *that* thread's `CALLBACK_DEPTH`. Moving it to another
    /// thread (or leaking it with `mem::forget`) would leave the entering
    /// thread flagged as "in callback" forever, corrupting the counters.
    _not_send: PhantomData<*const ()>,
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

#[derive(Clone, Debug, Serialize)]
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
        fs::write(path, self.to_json()?)
    }

    /// Serialize the report.
    ///
    /// A serialization failure is returned: emitting `{}` instead would leave
    /// an acceptance harness reading a well-formed but empty report.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
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

/// Current resident set size.
///
/// `getrusage` reports `ru_maxrss`, which is the *peak* and therefore not
/// comparable with the Linux current-RSS value (the acceptance report compares
/// the startup sample against later ones). `task_info` with `MACH_TASK_BASIC_INFO`
/// gives the current footprint instead.
#[cfg(target_os = "macos")]
fn resident_set_bytes() -> io::Result<u64> {
    const MACH_TASK_BASIC_INFO: u32 = 20;
    #[repr(C)]
    struct MachTaskBasicInfo {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: [u32; 2],
        system_time: [u32; 2],
        policy: i32,
        suspend_count: i32,
    }
    let mut info = std::mem::MaybeUninit::<MachTaskBasicInfo>::zeroed();
    let mut count = (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<u32>()) as u32;
    // SAFETY: `mach_task_self()` is the calling task, and `info`/`count`
    // describe a valid buffer of the requested size.
    // `libc::mach_task_self` is deprecated in favour of the `mach2` crate; the
    // port does not take that dependency, and the deprecated item is the same
    // syscall, so the call is allowed locally rather than pulling in a crate
    // for one function.
    #[allow(deprecated)]
    let task = unsafe { libc::mach_task_self() };
    let result = unsafe {
        libc::task_info(
            task,
            MACH_TASK_BASIC_INFO,
            info.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if result != 0 {
        return Err(io::Error::other(format!(
            "task_info failed with kern_return_t {result}"
        )));
    }
    // SAFETY: the call above filled the buffer.
    Ok(unsafe { info.assume_init() }.resident_size)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn resident_set_bytes() -> io::Result<u64> {
    Ok(0)
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
