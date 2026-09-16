//! Video lifecycle and frame scheduling.
//!
//! SDL/OpenGL code belongs in an implementation of [`VideoBackend`].  The
//! worker owns that implementation, which is important for backends (notably
//! SDL on macOS) that require all window operations on one thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderMetrics {
    pub logical_width: i32,
    pub logical_height: i32,
    pub drawable_width: i32,
    pub drawable_height: i32,
    pub scale_x: f32,
    pub scale_y: f32,
}

/// Frames the worker may queue before `submit` blocks.
///
/// A frame carries a full pixel buffer, so the queue is deliberately tiny:
/// video only needs the newest frame, and an unbounded queue would grow by
/// megabytes per frame whenever the backend falls behind.
pub const MAX_PENDING_FRAMES: usize = 4;

/// Accessor for the recorded `present` failure.
impl RenderMetrics {
    /// Construct from unsigned sizes (from window metrics where 0 = unset).
    pub fn from_sizes(logical: (u32, u32), drawable: (u32, u32)) -> Self {
        // `new` treats non-positive sizes as "unset", so a huge (or wrapped)
        // size must not become a negative sentinel.
        let clamp = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
        Self::new(
            clamp(logical.0),
            clamp(logical.1),
            clamp(drawable.0),
            clamp(drawable.1),
        )
    }

    /// Construct from possibly-negative sizes (negative = unset sentinel).
    pub fn new(w: i32, h: i32, dw: i32, dh: i32) -> Self {
        let logical_width = if w <= 0 {
            if dw > 0 { dw } else { 1 }
        } else {
            w
        };
        let logical_height = if h <= 0 {
            if dh > 0 { dh } else { 1 }
        } else {
            h
        };
        let drawable_width = if dw <= 0 { logical_width } else { dw };
        let drawable_height = if dh <= 0 { logical_height } else { dh };
        Self {
            logical_width,
            logical_height,
            drawable_width,
            drawable_height,
            scale_x: drawable_width as f32 / logical_width as f32,
            scale_y: drawable_height as f32 / logical_height as f32,
        }
    }

    fn scale_extent(value: i32, scale: f32) -> i32 {
        if value <= 0 {
            return 0;
        }
        if scale <= 0.0 {
            return value;
        }
        ((value as f32 * scale + 0.5) as i32).max(1)
    }

    pub fn x(&self, v: i32) -> i32 {
        Self::scale_extent(v, self.scale_x)
    }
    pub fn y(&self, v: i32) -> i32 {
        Self::scale_extent(v, self.scale_y)
    }
    pub fn scale_x(&self, value: i32) -> i32 {
        Self::scale_extent(value, self.scale_x)
    }
    pub fn scale_y(&self, value: i32) -> i32 {
        Self::scale_extent(value, self.scale_y)
    }
    pub fn extent(&self, v: i32, s: f32) -> i32 {
        Self::scale_extent(v, s)
    }
    pub fn scale_font(&self, points: i32) -> i32 {
        self.scale_y(points)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoMode {
    pub fullscreen: bool,
    pub windowed_size: (u32, u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoFrame {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub timestamp: f64,
}

/// The concrete SDL/OpenGL adapter implements this trait.  `open` and
/// `set_mode` must leave the backend ready for `present`; `close` releases it.
pub trait VideoBackend: Send + 'static {
    /// Whether this backend may only be owned and operated by the process
    /// main thread. Generic `VideoIO` uses a worker thread, so it refuses
    /// these backends before opening or moving them.
    fn requires_main_thread() -> bool {
        false
    }
    fn open(&mut self, mode: VideoMode) -> Result<RenderMetrics, String>;
    fn set_mode(&mut self, mode: VideoMode) -> Result<RenderMetrics, String>;
    fn present(&mut self, frame: &VideoFrame) -> Result<(), String>;
    fn close(&mut self);
}


enum Command {
    Frame(VideoFrame),
    Mode(VideoMode, mpsc::Sender<Result<RenderMetrics, String>>),
    Stop,
}

pub struct VideoIO<B: VideoBackend> {
    tx: Option<mpsc::SyncSender<Command>>,
    thread: Option<JoinHandle<()>>,
    active: Arc<AtomicBool>,
    mode: Arc<Mutex<VideoMode>>,
    metrics: Arc<Mutex<RenderMetrics>>,
    video_time: Arc<Mutex<f64>>,
    /// Why the worker stopped, set when a `present` failed.
    present_failure: Arc<Mutex<Option<String>>>,
    backend: Option<B>,
}

impl<B: VideoBackend> VideoIO<B> {
    pub fn new(backend: B, windowed_size: (u32, u32)) -> Self {
        Self {
            tx: None,
            thread: None,
            active: Arc::new(AtomicBool::new(false)),
            mode: Arc::new(Mutex::new(VideoMode {
                fullscreen: false,
                windowed_size,
            })),
            metrics: Arc::new(Mutex::new(RenderMetrics::from_sizes(
                windowed_size,
                windowed_size,
            ))),
            video_time: Arc::new(Mutex::new(0.0)),
            present_failure: Arc::new(Mutex::new(None)),
            backend: Some(backend),
        }
    }
    pub fn activate<F: FnMut(&mut VideoFrame) + Send + 'static>(&mut self, mut renderer: F) -> Result<(), String> {
        if B::requires_main_thread() {
            return Err(
                "video backend requires the Cocoa main thread and cannot run in generic VideoIO"
                    .into(),
            );
        }
        if self.active.swap(true, Ordering::AcqRel) {
            // Reporting success would imply the new renderer was installed; it
            // is dropped here instead.
            return Err("video is already active; the renderer was not replaced".to_string());
        }
        // Bounded so a slow backend applies back-pressure instead of queueing
        // multi-megabyte frames until the process runs out of memory.
        let (tx, rx) = mpsc::sync_channel(MAX_PENDING_FRAMES);
        let active = Arc::clone(&self.active);
        let metrics = Arc::clone(&self.metrics);
        let metrics_ready = Arc::clone(&self.metrics);
        let time = Arc::clone(&self.video_time);
        let present_failure = Arc::clone(&self.present_failure);
        *present_failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let mode = *self.mode.lock().unwrap_or_else(|e| e.into_inner());
        let mut backend = match self.backend.take() {
            Some(backend) => backend,
            None => {
                // `active` was set above; without a backend there is no worker
                // either, so the flag must not stay set.
                self.active.store(false, Ordering::Release);
                return Err(
                    "video backend was already consumed by a previous activate/close".to_string(),
                );
            }
        };
        // Every backend call runs on the worker thread: window and GL contexts
        // are thread-affine, so `open` must not happen on the caller's thread.
        let (ready_tx, ready_rx) = mpsc::channel();
        self.tx = Some(tx);
        self.thread = Some(thread::spawn(move || {
            let start = Instant::now();
            let open_result = backend.open(mode);
            if let Err(error) = open_result {
                // Hand the backend back so a retry can reuse it; the open
                // failure restored the previous contract.
                let _ = ready_tx.send(Err((error, backend)));
                active.store(false, Ordering::Release);
                return;
            }
            if let Ok(opened) = open_result
                && ready_tx.send(Ok(opened)).is_err()
            {
                // The caller gave up waiting: nothing to present.
                backend.close();
                active.store(false, Ordering::Release);
                return;
            }
            while active.load(Ordering::Acquire) {
                match rx.recv() {
                    Ok(Command::Frame(mut frame)) => {
                        renderer(&mut frame);
                        if let Err(error) = backend.present(&frame) {
                            // Report why video stopped (and stop immediately):
                            // `is_active` alone cannot tell the caller.
                            eprintln!("FreeWheeling: video present failed: {error}");
                            *present_failure
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                Some(error);
                            active.store(false, Ordering::Release);
                            break;
                        }
                        *time.lock().unwrap_or_else(|e| e.into_inner()) = start.elapsed().as_secs_f64();
                    }
                    Ok(Command::Mode(m, reply)) => {
                        let result = backend.set_mode(m);
                        if let Ok(ref value) = result {
                            *metrics.lock().unwrap_or_else(|e| e.into_inner()) = *value;
                        }
                        let _ = reply.send(result);
                    }
                    Ok(Command::Stop) | Err(_) => break,
                }
            }
            backend.close();
            active.store(false, Ordering::Release);
        }));
        // Wait for `open` so the caller still learns about a failure.
        match ready_rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(opened)) => {
                *metrics_ready.lock().unwrap_or_else(|e| e.into_inner()) = opened;
                Ok(())
            }
            Ok(Err((error, backend))) => {
                self.backend = Some(backend);
                self.tx = None;
                self.thread = None;
                self.active.store(false, Ordering::Release);
                Err(error)
            }
            Err(error) => {
                self.tx = None;
                self.thread = None;
                self.active.store(false, Ordering::Release);
                Err(format!("video backend did not open in time: {error}"))
            }
        }
    }
    /// Queue a frame for presentation.
    ///
    /// Blocks when the worker is `MAX_PENDING_FRAMES` behind: dropping the
    /// frame instead would show stale video while a growing queue would be
    /// unbounded.
    pub fn submit(&self, frame: VideoFrame) -> Result<(), String> {
        self.tx
            .as_ref()
            .ok_or("video is not active".to_string())?
            .send(Command::Frame(frame))
            .map_err(|e| e.to_string())
    }
    pub fn set_video_mode(&self, fullscreen: bool) -> Result<RenderMetrics, String> {
        let mode = VideoMode {
            fullscreen,
            windowed_size: self.mode.lock().unwrap_or_else(|e| e.into_inner()).windowed_size,
        };
        let (tx, rx) = mpsc::channel();
        self.tx
            .as_ref()
            .ok_or("video is not active".to_string())?
            .send(Command::Mode(mode, tx))
            .map_err(|e| e.to_string())?;
        // A worker that exited (or is stuck) must not hang the caller.
        let metrics = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| format!("video mode change timed out or worker exited: {e}"))??;
        // Cache the mode only after the worker confirmed it, otherwise
        // `fullscreen()` and the next `activate` diverge from the backend.
        *self.mode.lock().unwrap_or_else(|e| e.into_inner()) = mode;
        Ok(metrics)
    }
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Why the video worker stopped, or `None` while it is running or was
    /// stopped deliberately.
    pub fn present_failure(&self) -> Option<String> {
        self.present_failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
    pub fn video_time(&self) -> f64 {
        *self.video_time.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn render_metrics(&self) -> RenderMetrics {
        *self.metrics.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn fullscreen(&self) -> bool {
        self.mode.lock().unwrap_or_else(|e| e.into_inner()).fullscreen
    }
    pub fn close(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Command::Stop);
        }
        if let Some(thread) = self.thread.take() {
            crate::event::join_with_timeout(thread);
        }
    }
}
impl<B: VideoBackend> Drop for VideoIO<B> {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        modes: usize,
        frames: usize,
    }
    impl VideoBackend for Fake {
        fn open(&mut self, m: VideoMode) -> Result<RenderMetrics, String> {
            self.modes += 1;
            Ok(RenderMetrics::from_sizes(m.windowed_size, m.windowed_size))
        }
        fn set_mode(&mut self, m: VideoMode) -> Result<RenderMetrics, String> {
            self.modes += 1;
            Ok(RenderMetrics::from_sizes(m.windowed_size, m.windowed_size))
        }
        fn present(&mut self, _: &VideoFrame) -> Result<(), String> {
            self.frames += 1;
            Ok(())
        }
        fn close(&mut self) {}
    }
    #[test]
    fn lifecycle_mode_and_frame_flow() {
        let mut v = VideoIO::new(
            Fake {
                modes: 0,
                frames: 0,
            },
            (640, 480),
        );
        v.activate(|f| f.timestamp += 1.0).unwrap();
        v.submit(VideoFrame {
            pixels: vec![],
            width: 0,
            height: 0,
            stride: 0,
            timestamp: 0.0,
        })
        .unwrap();
        assert!(v.set_video_mode(true).unwrap().scale_x > 0.0);
        assert!(v.is_active());
        v.close();
        assert!(!v.is_active());
    }

    struct MainThreadOnly;
    impl VideoBackend for MainThreadOnly {
        fn requires_main_thread() -> bool {
            true
        }
        fn open(&mut self, _: VideoMode) -> Result<RenderMetrics, String> {
            panic!("main-thread backend must be rejected before open")
        }
        fn set_mode(&mut self, _: VideoMode) -> Result<RenderMetrics, String> {
            unreachable!()
        }
        fn present(&mut self, _: &VideoFrame) -> Result<(), String> {
            unreachable!()
        }
        fn close(&mut self) {
            panic!("main-thread backend must not be moved to the worker")
        }
    }

    #[test]
    fn rejects_main_thread_backend_before_starting_worker() {
        let mut video = VideoIO::new(MainThreadOnly, (640, 480));
        let error = video.activate(|f| f.timestamp += 1.0).unwrap_err();
        assert!(error.contains("requires the Cocoa main thread"));
        assert!(!video.is_active());
    }
    #[test]
    fn metrics_scale_logical_coordinates() {
        let m = RenderMetrics::from_sizes((640, 480), (1280, 960));
        assert_eq!(m.scale_x(35), 70);
        assert_eq!(m.scale_font(10), 20);
    }

    #[test]
    fn metrics_match_cpp_zero_extent_and_scale_rules() {
        let m = RenderMetrics::from_sizes((0, 0), (1280, 960));
        assert_eq!((m.logical_width, m.logical_height), (1280, 960));
        assert_eq!((m.drawable_width, m.drawable_height), (1280, 960));
        let m = RenderMetrics::from_sizes((640, 480), (0, 0));
        assert_eq!((m.drawable_width, m.drawable_height), (640, 480));
        assert_eq!(m.scale_x(0), 0);
        assert_eq!(m.scale_font(-1), 0);
        let zero_scale = RenderMetrics {
            scale_x: 0.0,
            scale_y: 0.0,
            ..m
        };
        assert_eq!(zero_scale.scale_x(7), 7);
    }
}
