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
    pub logical_width: u32,
    pub logical_height: u32,
    pub drawable_width: u32,
    pub drawable_height: u32,
    pub scale_x: f32,
    pub scale_y: f32,
}

impl RenderMetrics {
    pub fn from_sizes(logical: (u32, u32), drawable: (u32, u32)) -> Self {
        // This is FweelinComputeVideoScale verbatim in unsigned-size form:
        // an absent logical extent inherits a drawable extent, and vice versa.
        // The previous `max(1)` denominator retained zero drawable sizes and
        // therefore diverged while a window was being minimized or resized.
        let logical_width = if logical.0 == 0 {
            drawable.0.max(1)
        } else {
            logical.0
        };
        let logical_height = if logical.1 == 0 {
            drawable.1.max(1)
        } else {
            logical.1
        };
        let drawable_width = if drawable.0 == 0 {
            logical_width
        } else {
            drawable.0
        };
        let drawable_height = if drawable.1 == 0 {
            logical_height
        } else {
            drawable.1
        };
        let sx = drawable_width as f32 / logical_width as f32;
        let sy = drawable_height as f32 / logical_height as f32;
        Self {
            logical_width,
            logical_height,
            drawable_width,
            drawable_height,
            scale_x: sx,
            scale_y: sy,
        }
    }

    fn scale_extent(value: i32, scale: f32) -> i32 {
        if value <= 0 {
            return 0;
        }
        if scale <= 0.0 {
            return value;
        }
        // C++ casts `value * scale + .5f` to int (truncate toward zero) and
        // retains one drawable pixel for every positive logical extent.
        ((value as f32 * scale + 0.5) as i32).max(1)
    }

    pub fn scale_x(&self, value: i32) -> i32 {
        Self::scale_extent(value, self.scale_x)
    }
    pub fn scale_y(&self, value: i32) -> i32 {
        Self::scale_extent(value, self.scale_y)
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

pub trait VideoRenderer: Send + 'static {
    fn render(&mut self, frame: &mut VideoFrame);
}

enum Command {
    Frame(VideoFrame),
    Mode(VideoMode, mpsc::Sender<Result<RenderMetrics, String>>),
    Stop,
}

pub struct VideoIO<B: VideoBackend> {
    tx: Option<mpsc::Sender<Command>>,
    thread: Option<JoinHandle<()>>,
    active: Arc<AtomicBool>,
    mode: Arc<Mutex<VideoMode>>,
    metrics: Arc<Mutex<RenderMetrics>>,
    video_time: Arc<Mutex<f64>>,
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
            backend: Some(backend),
        }
    }
    pub fn activate<R: VideoRenderer>(&mut self, mut renderer: R) -> Result<(), String> {
        if B::requires_main_thread() {
            return Err(
                "video backend requires the Cocoa main thread and cannot run in generic VideoIO"
                    .into(),
            );
        }
        if self.active.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel();
        let active = Arc::clone(&self.active);
        let metrics = Arc::clone(&self.metrics);
        let time = Arc::clone(&self.video_time);
        let mode = *self.mode.lock().expect("video mode poisoned");
        let mut backend = self
            .backend
            .take()
            .ok_or("video backend already active".to_string())?;
        let opened = match backend.open(mode) {
            Ok(metrics) => metrics,
            Err(error) => {
                self.backend = Some(backend);
                self.active.store(false, Ordering::Release);
                return Err(error);
            }
        };
        *metrics.lock().expect("video metrics poisoned") = opened;
        self.tx = Some(tx);
        self.thread = Some(thread::spawn(move || {
            let start = Instant::now();
            while active.load(Ordering::Acquire) {
                match rx.recv() {
                    Ok(Command::Frame(mut frame)) => {
                        renderer.render(&mut frame);
                        if backend.present(&frame).is_err() {
                            active.store(false, Ordering::Release);
                        }
                        *time.lock().expect("video time poisoned") = start.elapsed().as_secs_f64();
                    }
                    Ok(Command::Mode(m, reply)) => {
                        let result = backend.set_mode(m);
                        if let Ok(ref value) = result {
                            *metrics.lock().expect("video metrics poisoned") = *value;
                        }
                        let _ = reply.send(result);
                    }
                    Ok(Command::Stop) | Err(_) => break,
                }
            }
            backend.close();
            active.store(false, Ordering::Release);
        }));
        Ok(())
    }
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
            windowed_size: self.mode.lock().expect("video mode poisoned").windowed_size,
        };
        *self.mode.lock().expect("video mode poisoned") = mode;
        let (tx, rx) = mpsc::channel();
        self.tx
            .as_ref()
            .ok_or("video is not active".to_string())?
            .send(Command::Mode(mode, tx))
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    pub fn video_time(&self) -> f64 {
        *self.video_time.lock().expect("video time poisoned")
    }
    pub fn render_metrics(&self) -> RenderMetrics {
        *self.metrics.lock().expect("video metrics poisoned")
    }
    pub fn fullscreen(&self) -> bool {
        self.mode.lock().expect("video mode poisoned").fullscreen
    }
    pub fn close(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Command::Stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
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
    struct Identity;
    impl VideoRenderer for Identity {
        fn render(&mut self, f: &mut VideoFrame) {
            f.timestamp += 1.0;
        }
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
        v.activate(Identity).unwrap();
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
        let error = video.activate(Identity).unwrap_err();
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
