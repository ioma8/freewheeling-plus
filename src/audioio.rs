//! Audio I/O boundary shared by native and deterministic test backends.
//!
//! Activation transfers ownership of the processor to the backend.  This is
//! intentional: a realtime callback must never acquire a mutex to reach DSP.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::ThreadId;

pub type Sample = f32;
pub type NFrames = u32;
pub const NUM_CHANNELS: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct JackPosition {
    pub frame: u32,
    pub valid: u32,
    pub bar: i32,
    pub beat: i32,
    pub beats_per_minute: f64,
    pub beats_per_bar: f32,
    pub beat_type: i32,
    pub ticks_per_beat: i32,
    pub tick: i32,
    pub bar_start_tick: i32,
    pub frame_rate: u32,
}

pub struct AudioCallback<'a> {
    pub inputs: [&'a [Sample]; NUM_CHANNELS],
    pub outputs: [&'a mut [Sample]; NUM_CHANNELS],
    pub nframes: NFrames,
    pub position: JackPosition,
}

pub trait AudioProcessor: Send {
    fn process(&mut self, callback: &mut AudioCallback<'_>);
}

impl<F> AudioProcessor for F
where
    F: for<'a> FnMut(&mut AudioCallback<'a>) + Send,
{
    fn process(&mut self, callback: &mut AudioCallback<'_>) {
        self(callback);
    }
}

/// Owned, mutable realtime callback.  Backends invoke it from exactly one
/// playback thread at a time.
pub type AudioCallbackFn = Box<dyn for<'a> FnMut(&mut AudioCallback<'a>) + Send + 'static>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioMetrics {
    pub capture_overruns: u64,
    pub capture_underruns: u64,
    pub xruns: u64,
    pub stream_errors: u64,
    pub callbacks: u64,
    pub callback_frames: u64,
    pub callback_peak_nanos: u64,
    pub callback_total_nanos: u64,
    pub recovery_requests: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioRecoveryMetrics {
    pub attempts: u64,
    pub failures: u64,
}

pub trait AudioBackend: Send {
    fn open(&mut self, client_name: &str) -> Result<BackendInfo, String>;
    fn activate(&mut self, callback: AudioCallbackFn) -> Result<(), String>;
    fn close(&mut self);
    fn relocate(&mut self, frame: NFrames);
    fn metrics(&self) -> AudioMetrics {
        AudioMetrics::default()
    }
    /// Most recent callback-window DSP load, when the native backend can
    /// measure it.  C++ AudioIO updates this every 16 callbacks.
    fn cpu_load(&self) -> Option<f32> {
        None
    }
    /// True after a route change or device loss. Reopening is deliberately
    /// controlled by the non-realtime owner rather than an audio callback.
    fn recovery_requested(&self) -> bool {
        false
    }
    /// Rebuild active streams on the non-realtime owner thread. Implementors
    /// must retain (or safely return) the callback if rebuilding fails.
    fn recover(&mut self) -> Result<BackendInfo, String> {
        Err("audio backend does not support controlled recovery".to_string())
    }
    fn recovery_metrics(&self) -> AudioRecoveryMetrics {
        AudioRecoveryMetrics::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendInfo {
    pub sample_rate: NFrames,
    pub buffer_size: NFrames,
}

pub struct AudioIO<B: AudioBackend> {
    backend: B,
    sample_rate: AtomicU32,
    buffer_size: AtomicU32,
    position: Arc<Mutex<JackPosition>>,
    sync_active: AtomicBool,
    timebase_master: AtomicBool,
    transport_roll: AtomicBool,
    callback_thread: Arc<OnceLock<ThreadId>>,
    active: bool,
}

impl<B: AudioBackend> AudioIO<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            sample_rate: AtomicU32::new(0),
            buffer_size: AtomicU32::new(0),
            position: Arc::new(Mutex::new(JackPosition::default())),
            sync_active: AtomicBool::new(false),
            timebase_master: AtomicBool::new(false),
            transport_roll: AtomicBool::new(false),
            callback_thread: Arc::new(OnceLock::new()),
            active: false,
        }
    }

    pub fn open(&mut self, name: &str) -> Result<(), String> {
        let info = self.backend.open(name)?;
        self.sample_rate.store(info.sample_rate, Ordering::Release);
        self.buffer_size.store(info.buffer_size, Ordering::Release);
        Ok(())
    }

    /// Transfers `processor` into the realtime backend.
    ///
    /// Migration note: callers must pass an owned processor, not
    /// `Arc<Mutex<P>>`. Control changes should use bounded lock-free queues.
    pub fn activate<P: AudioProcessor + 'static>(
        &mut self,
        mut processor: P,
    ) -> Result<(), String> {
        self.activate_callback(Box::new(move |callback| processor.process(callback)))
    }

    /// Activates a type-erased processor without wrapping it in shared or
    /// mutex-protected ownership. Its final drop happens after backend close.
    pub fn activate_boxed(&mut self, mut processor: Box<dyn AudioProcessor>) -> Result<(), String> {
        self.activate_callback(Box::new(move |callback| processor.process(callback)))
    }

    fn activate_callback(&mut self, callback: AudioCallbackFn) -> Result<(), String> {
        let callback_thread = Arc::clone(&self.callback_thread);
        let mut callback = callback;
        self.backend.activate(Box::new(move |audio| {
            let _ = callback_thread.set(std::thread::current().id());
            callback(audio);
        }))?;
        self.active = true;
        Ok(())
    }

    pub fn close(&mut self) {
        if self.active {
            self.backend.close();
            self.active = false;
        }
    }
    pub fn getbufsz(&self) -> NFrames {
        self.buffer_size.load(Ordering::Acquire)
    }
    pub fn get_srate(&self) -> NFrames {
        self.sample_rate.load(Ordering::Acquire)
    }
    pub fn get_cpu_load(&self) -> f32 {
        if let Some(load) = self.backend.cpu_load() {
            return load;
        }
        let metrics = self.backend.metrics();
        if metrics.callback_frames == 0 || self.get_srate() == 0 {
            return 0.0;
        }
        let available = metrics.callback_frames as f64 / self.get_srate() as f64;
        (metrics.callback_total_nanos as f64 / 1_000_000_000.0 / available) as f32
    }
    pub fn metrics(&self) -> AudioMetrics {
        self.backend.metrics()
    }
    /// Inspect backend-specific non-realtime state such as device route and
    /// callback health. Never call this from an audio callback.
    pub fn backend(&self) -> &B {
        &self.backend
    }
    pub fn recovery_requested(&self) -> bool {
        self.backend.recovery_requested()
    }
    /// Quiesce callbacks and rebuild the active route without moving DSP or
    /// control state out of the backend.
    pub fn recover(&mut self) -> Result<(), String> {
        if !self.active {
            return Err("audio backend is not active".to_string());
        }
        let info = self.backend.recover()?;
        self.sample_rate.store(info.sample_rate, Ordering::Release);
        self.buffer_size.store(info.buffer_size, Ordering::Release);
        Ok(())
    }
    pub fn recovery_metrics(&self) -> AudioRecoveryMetrics {
        self.backend.recovery_metrics()
    }
    pub fn get_position(&self) -> JackPosition {
        *self.position.lock().expect("position poisoned")
    }
    pub fn is_sync(&self) -> bool {
        self.sync_active.load(Ordering::Acquire)
    }
    pub fn is_timebase_master(&self) -> bool {
        self.timebase_master.load(Ordering::Acquire)
    }
    pub fn is_transport_rolling(&self) -> bool {
        self.transport_roll.load(Ordering::Acquire)
    }
    pub fn relocate_transport(&mut self, frame: NFrames) {
        self.backend.relocate(frame);
    }
    pub fn callback_thread(&self) -> Option<ThreadId> {
        self.callback_thread.get().copied()
    }
}

impl<B: AudioBackend> Drop for AudioIO<B> {
    fn drop(&mut self) {
        self.close();
    }
}
impl<B: AudioBackend> fmt::Debug for AudioIO<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioIO")
            .field("active", &self.active)
            .field("sample_rate", &self.get_srate())
            .field("buffer_size", &self.getbufsz())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        info: BackendInfo,
        activated: bool,
        relocated: Option<NFrames>,
        recoveries: u64,
    }
    impl AudioBackend for Fake {
        fn open(&mut self, _: &str) -> Result<BackendInfo, String> {
            Ok(self.info)
        }
        fn activate(&mut self, mut callback: AudioCallbackFn) -> Result<(), String> {
            let input = [vec![1.0; 4], vec![2.0; 4]];
            let mut left = vec![0.0; 4];
            let mut right = vec![0.0; 4];
            let mut cb = AudioCallback {
                inputs: [&input[0], &input[1]],
                outputs: [&mut left, &mut right],
                nframes: 4,
                position: JackPosition::default(),
            };
            callback(&mut cb);
            assert_eq!(left, vec![2.0; 4]);
            self.activated = true;
            Ok(())
        }
        fn close(&mut self) {}
        fn relocate(&mut self, frame: NFrames) {
            self.relocated = Some(frame);
        }
        fn recover(&mut self) -> Result<BackendInfo, String> {
            self.recoveries += 1;
            Ok(BackendInfo {
                sample_rate: 44_100,
                buffer_size: 8,
            })
        }
        fn recovery_metrics(&self) -> AudioRecoveryMetrics {
            AudioRecoveryMetrics {
                attempts: self.recoveries,
                failures: 0,
            }
        }
    }
    struct Gain;
    impl AudioProcessor for Gain {
        fn process(&mut self, cb: &mut AudioCallback<'_>) {
            for i in 0..cb.nframes as usize {
                cb.outputs[0][i] = cb.inputs[0][i] * 2.0;
            }
        }
    }
    #[test]
    fn backend_owns_mutable_processor_without_a_mutex() {
        let backend = Fake {
            info: BackendInfo {
                sample_rate: 48_000,
                buffer_size: 4,
            },
            activated: false,
            relocated: None,
            recoveries: 0,
        };
        let mut io = AudioIO::new(backend);
        io.open("test").unwrap();
        io.activate(Gain).unwrap();
        assert_eq!(io.get_srate(), 48_000);
        assert_eq!(io.getbufsz(), 4);
        assert!(io.callback_thread().is_some());
        io.recover().unwrap();
        assert_eq!(io.get_srate(), 44_100);
        assert_eq!(io.getbufsz(), 8);
        assert_eq!(io.recovery_metrics().attempts, 1);
    }
}
