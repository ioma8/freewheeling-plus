//! Signal reporting and shutdown state, ported from `src/fweelin_signal.c`.

use libc::{c_int, c_void};
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicUsize, Ordering};

pub type SignalWriteFn = fn(*const u8, usize, *mut c_void);
pub type SignalExitFn = fn(c_int, *mut c_void);

static SHUTDOWN_REQUESTED: AtomicI32 = AtomicI32::new(0);
// These mirror the C test hooks without a mutable-static data race when a
// handler runs while a test harness clears them.  Normal production startup
// never installs either hook, so handlers use only write(2) and _exit(2).
static TEST_WRITER: AtomicUsize = AtomicUsize::new(0);
static TEST_EXITER: AtomicUsize = AtomicUsize::new(0);
static TEST_CTX: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn fatal_name(sig: c_int) -> &'static [u8] {
    match sig {
        libc::SIGSEGV => b"SIGSEGV",
        libc::SIGBUS => b"SIGBUS",
        libc::SIGILL => b"SIGILL",
        libc::SIGFPE => b"SIGFPE",
        _ => b"SIGNAL",
    }
}
fn fatal_text(sig: c_int) -> &'static [u8] {
    match sig {
        libc::SIGSEGV => b"Segmentation fault",
        libc::SIGBUS => b"Access to undefined memory object",
        libc::SIGILL => b"Illegal instruction",
        libc::SIGFPE => b"Erroneous arithmetic operation",
        _ => b"Fatal signal received",
    }
}
fn info_text(sig: c_int) -> &'static [u8] {
    match sig {
        libc::SIGUSR1 => b">>> User defined signal 1 (SIGUSR1) received <<<\n",
        libc::SIGUSR2 => b">>> User defined signal 2 (SIGUSR2) received <<<\n",
        _ => b">>> Signal received <<<\n",
    }
}

/// Writes as much as fits, always NUL-terminating when `buf` is non-empty.
/// Returns the number of bytes copied (excluding the terminator).
pub fn format_signal_message(sig: c_int, buf: &mut [u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    let parts = [
        b">>> FATAL ERROR: ".as_slice(),
        fatal_text(sig),
        b" (",
        fatal_name(sig),
        b") occurred! <<<\n",
    ];
    let mut pos = 0;
    for part in parts {
        let room = buf.len() - 1 - pos;
        let n = room.min(part.len());
        buf[pos..pos + n].copy_from_slice(&part[..n]);
        pos += n;
        if n != part.len() {
            break;
        }
    }
    buf[pos] = 0;
    pos
}

fn dispatch_write(msg: &[u8]) {
    // SAFETY: libc::write is async-signal-safe; the fence pairs with
    // Release in set_signal_test_hooks for consistent hook pointer visibility.
    unsafe {
        std::sync::atomic::fence(Ordering::Acquire);
        let writer = TEST_WRITER.load(Ordering::Relaxed);
        if writer != 0 {
            let writer: SignalWriteFn = std::mem::transmute(writer);
            let ctx = TEST_CTX.load(Ordering::Relaxed);
            writer(msg.as_ptr(), msg.len(), ctx);
            return;
        }
        let mut p = msg.as_ptr();
        let mut n = msg.len();
        while n != 0 {
            let written = libc::write(libc::STDERR_FILENO, p.cast(), n);
            if written <= 0 {
                break;
            }
            p = p.add(written as usize);
            n -= written as usize;
        }
    }
}
fn dispatch_exit(code: c_int) {
    // SAFETY: libc::_exit is async-signal-safe; the fence pairs with
    // Release in set_signal_test_hooks for consistent hook pointer visibility.
    unsafe {
        std::sync::atomic::fence(Ordering::Acquire);
        let exiter = TEST_EXITER.load(Ordering::Relaxed);
        if exiter != 0 {
            let exiter: SignalExitFn = std::mem::transmute(exiter);
            exiter(code, TEST_CTX.load(Ordering::Relaxed));
        } else {
            libc::_exit(code);
        }
    }
}

pub fn log_nonfatal_signal(sig: c_int) {
    dispatch_write(info_text(sig));
}
pub fn request_shutdown_signal_handler(sig: c_int) {
    SHUTDOWN_REQUESTED.store(sig, Ordering::SeqCst);
}
pub fn shutdown_requested() -> c_int {
    SHUTDOWN_REQUESTED.load(Ordering::Acquire)
}
pub fn clear_shutdown_request() {
    SHUTDOWN_REQUESTED.store(0, Ordering::Release);
}

pub fn fatal_signal_handler(sig: c_int) {
    let mut buf = [0u8; 160];
    let len = format_signal_message(sig, &mut buf);
    dispatch_write(&buf[..len]);
    dispatch_write(b"Stack trace generation is deferred to a safe context.\n");
    dispatch_exit(128 + sig);
}

pub fn set_signal_test_hooks(
    writer: Option<SignalWriteFn>,
    exiter: Option<SignalExitFn>,
    ctx: *mut c_void,
) {
    // Publish the context before the callbacks and clear callbacks before a
    // context can be replaced. Hook callers retain the context until clear.
    TEST_CTX.store(ctx, Ordering::Release);
    TEST_WRITER.store(writer.map_or(0, |hook| hook as usize), Ordering::Release);
    TEST_EXITER.store(exiter.map_or(0, |hook| hook as usize), Ordering::Release);
}
pub fn clear_signal_test_hooks() {
    TEST_WRITER.store(0, Ordering::Release);
    TEST_EXITER.store(0, Ordering::Release);
    TEST_CTX.store(std::ptr::null_mut(), Ordering::Release);
}

extern "C" fn fatal_trampoline(sig: c_int) {
    fatal_signal_handler(sig);
}
extern "C" fn info_trampoline(sig: c_int) {
    log_nonfatal_signal(sig);
}
extern "C" fn shutdown_trampoline(sig: c_int) {
    request_shutdown_signal_handler(sig);
}

fn register(handler: extern "C" fn(c_int), signals: &[c_int]) {
    // This is the same no-flags `sigaction` registration used in fweelin.cc.
    // `signal(3)` may have implementation-dependent reset/restart semantics.
    // SAFETY: zeroed sigaction is valid initialization; sigemptyset is
    // async-signal-safe and always succeeds on POSIX systems.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler as usize;
    // SAFETY: sigemptyset is async-signal-safe.
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    for &sig in signals {
        // SAFETY: sigaction is async-signal-safe; handler is a function
        // pointer, not a closure, so it's safe to call from any thread.
        unsafe {
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
    }
}
pub fn register_fatal_signal_handlers() {
    register(
        fatal_trampoline,
        &[libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE],
    );
}
pub fn register_info_signal_handlers() {
    register(info_trampoline, &[libc::SIGUSR1, libc::SIGUSR2]);
}
pub fn register_shutdown_signal_handlers() {
    register(shutdown_trampoline, &[libc::SIGINT, libc::SIGTERM]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static HOOK_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct Hooks {
        bytes: Vec<u8>,
        exit_code: c_int,
    }

    fn capture_write(message: *const u8, len: usize, ctx: *mut c_void) {
        // SAFETY: test context pointer is set up by the test itself and
        // guaranteed valid for the duration of the handler invocation.
        let hooks = unsafe { &mut *ctx.cast::<Hooks>() };
        // SAFETY: message pointer and length come from the test and are valid.
        hooks
            .bytes
            .extend_from_slice(unsafe { std::slice::from_raw_parts(message, len) });
    }
    fn capture_exit(code: c_int, ctx: *mut c_void) {
        // SAFETY: context pointer is test-owned.
        unsafe { (*ctx.cast::<Hooks>()).exit_code = code };
    }
    #[test]
    fn formatting_is_bounded_and_terminated() {
        let mut b = [0xff; 8];
        let n = format_signal_message(libc::SIGSEGV, &mut b);
        assert_eq!(n, 7);
        assert_eq!(b[7], 0);
    }
    #[test]
    fn mapping_and_shutdown_state() {
        let mut b = [0; 160];
        let n = format_signal_message(libc::SIGFPE, &mut b);
        assert_eq!(
            &b[..n],
            b">>> FATAL ERROR: Erroneous arithmetic operation (SIGFPE) occurred! <<<\n"
        );
        request_shutdown_signal_handler(15);
        assert_eq!(shutdown_requested(), 15);
        clear_shutdown_request();
        assert_eq!(shutdown_requested(), 0);
    }

    #[test]
    fn c_fatal_handler_hook_contract() {
        let _lock = HOOK_TEST_LOCK.lock().unwrap();
        let mut hooks = Hooks::default();
        set_signal_test_hooks(
            Some(capture_write),
            Some(capture_exit),
            (&mut hooks as *mut Hooks).cast(),
        );
        fatal_signal_handler(libc::SIGSEGV);
        clear_signal_test_hooks();

        assert!(String::from_utf8_lossy(&hooks.bytes).contains("SIGSEGV"));
        assert!(String::from_utf8_lossy(&hooks.bytes).contains("deferred to a safe context"));
        assert_eq!(hooks.exit_code, 128 + libc::SIGSEGV);
    }
}
