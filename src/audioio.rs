//! Audio I/O boundary shared by native and deterministic test backends.
//!
//! Activation transfers ownership of the processor to the backend.  This is
//! intentional: a realtime callback must never acquire a mutex to reach DSP.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use crate::realtime_guard::InstrumentedMutex;
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
    /// C++ `AudioIO::IsTransportRolling()`: whether an external transport
    /// (JACK) is rolling this callback. Backends without a transport
    /// (CPAL, CoreAudio) always pass `false`.
    pub transport_rolling: bool,
}

pub trait AudioProcessor: Send {
    fn process(&mut self, callback: &mut AudioCallback<'_>);
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

/// Transport state reported by backends that support external sync (JACK).
/// Backends without transport (CPAL, CoreAudio) return the default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransportState {
    pub rolling: bool,
    pub frame: u32,
    pub bar: i32,
    pub beat: i32,
    pub bpm: f64,
    pub beats_per_bar: f32,
    pub beat_type: i32,
}

pub trait AudioBackend: Send {
    fn open(&mut self, client_name: &str) -> Result<BackendInfo, String>;
    fn activate(&mut self, callback: AudioCallbackFn) -> Result<(), String>;
    fn close(&mut self);
    fn relocate(&mut self, frame: NFrames);
    fn metrics(&self) -> AudioMetrics {
        AudioMetrics::default()
    }
    fn cpu_load(&self) -> Option<f32> {
        None
    }
    fn input_latency_frames(&self) -> NFrames {
        0
    }
    fn recovery_requested(&self) -> bool {
        false
    }
    fn recover(&mut self) -> Result<BackendInfo, String> {
        Err("audio backend does not support controlled recovery".to_string())
    }
    fn recovery_metrics(&self) -> AudioRecoveryMetrics {
        AudioRecoveryMetrics::default()
    }

    /// Transport state from the backend (JACK). Default impl returns not-rolling.
    fn transport_state(&self) -> TransportState {
        TransportState::default()
    }

    /// Whether this backend can be driven by an external transport at all.
    ///
    /// Backends without one (CPAL, CoreAudio) run on their internal clock and
    /// are therefore the timebase master: `is_timebase_master` reports `true`
    /// for them (docs/cpp-port-gap-analysis.md).
    fn supports_transport(&self) -> bool {
        false
    }

    /// Receive pending MIDI events from backends that integrate MIDI (JACK).
    /// Standalone MIDI backends (Midir) use the separate MidiBackend trait.
    /// Returns None when no event or when MIDI is not integrated.
    fn receive_midi(&mut self) -> Option<crate::midiio::MidiPortMessage> {
        None
    }

    /// Send a MIDI event through the backend (JACK). Returns error when
    /// the backend does not support MIDI output integrated with audio.
    fn send_midi(
        &mut self,
        _msg: crate::midiio::MidiPortMessage,
        _offset: NFrames,
    ) -> Result<(), String> {
        Err("MIDI not supported by this audio backend".into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendInfo {
    pub sample_rate: NFrames,
    pub buffer_size: NFrames,
}

/// Runtime-selected audio backend.
///
/// Wraps one of the platform-specific backends so `AudioIO` can be
/// constructed from a common type regardless of which backend is selected
/// at startup.
pub enum AnyAudioBackend {
    /// Cross-platform CPAL backend (default on Linux; explicit override on
    /// macOS, where it is the split-clock fallback).
    Cpal(Box<crate::audio_native_cpal::CpalAudioBackend>),
    /// JACK backend (Linux and macOS).
    #[cfg(all(
        feature = "jack",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    Jack(Box<crate::jack::JackAudioMidiBackend>),
    /// Native CoreAudio/AudioUnit backend (macOS only).
    #[cfg(target_os = "macos")]
    AudioUnit(Box<crate::macos_audio_unit::MacosAudioUnitBackend>),
}

impl AnyAudioBackend {
    /// Returns `true` when this backend wraps the JACK Audio Connection Kit.
    pub fn is_jack(&self) -> bool {
        match self {
            AnyAudioBackend::Cpal(_) => false,
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(_) => true,
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(_) => false,
        }
    }
}

impl AudioBackend for AnyAudioBackend {
    fn open(&mut self, client_name: &str) -> Result<BackendInfo, String> {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.open(client_name),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.open(client_name),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.open(client_name),
        }
    }

    fn activate(&mut self, callback: AudioCallbackFn) -> Result<(), String> {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.activate(callback),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.activate(callback),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.activate(callback),
        }
    }

    fn close(&mut self) {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.close(),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.close(),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.close(),
        }
    }

    fn relocate(&mut self, frame: NFrames) {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.relocate(frame),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.relocate(frame),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.relocate(frame),
        }
    }

    fn metrics(&self) -> AudioMetrics {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.metrics(),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.metrics(),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.metrics(),
        }
    }

    fn cpu_load(&self) -> Option<f32> {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.cpu_load(),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.cpu_load(),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.cpu_load(),
        }
    }

    fn input_latency_frames(&self) -> NFrames {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.input_latency_frames(),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.input_latency_frames(),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.input_latency_frames(),
        }
    }

    fn recovery_requested(&self) -> bool {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.recovery_requested(),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.recovery_requested(),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.recovery_requested(),
        }
    }

    fn recover(&mut self) -> Result<BackendInfo, String> {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.recover(),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.recover(),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.recover(),
        }
    }

    fn recovery_metrics(&self) -> AudioRecoveryMetrics {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.recovery_metrics(),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.recovery_metrics(),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.recovery_metrics(),
        }
    }

    fn transport_state(&self) -> TransportState {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.transport_state(),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.transport_state(),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.transport_state(),
        }
    }

    fn supports_transport(&self) -> bool {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.supports_transport(),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.supports_transport(),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.supports_transport(),
        }
    }

    fn receive_midi(&mut self) -> Option<crate::midiio::MidiPortMessage> {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.receive_midi(),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.receive_midi(),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.receive_midi(),
        }
    }

    fn send_midi(
        &mut self,
        msg: crate::midiio::MidiPortMessage,
        offset: NFrames,
    ) -> Result<(), String> {
        match self {
            AnyAudioBackend::Cpal(backend) => backend.send_midi(msg, offset),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(backend) => backend.send_midi(msg, offset),
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => backend.send_midi(msg, offset),
        }
    }
}

impl AnyAudioBackend {
    /// Snapshot of non-realtime diagnostic state. Returns `None` for backends
    /// that do not expose device-level status (JACK).
    pub fn status(&self) -> Option<crate::audio_native_cpal::CpalAudioStatus> {
        match self {
            AnyAudioBackend::Cpal(backend) => Some(backend.status()),
            #[cfg(all(
                feature = "jack",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            AnyAudioBackend::Jack(_) => None,
            #[cfg(target_os = "macos")]
            AnyAudioBackend::AudioUnit(backend) => Some(backend.status()),
        }
    }
}

/// Realtime-safe publication slot for the thread currently running the audio
/// callback.
///
/// The audio thread never blocks here: it only performs a `try_lock` write
/// until the id has been published. The mutex is the instrumented wrapper so
/// that any future change to a blocking acquisition becomes visible to the
/// realtime acceptance counters instead of silently stalling the callback.
#[derive(Default)]
struct CallbackThreadSlot {
    published: AtomicBool,
    thread: InstrumentedMutex<Option<ThreadId>>,
}

impl CallbackThreadSlot {
    /// Audio-thread side. Skips work once the id is published.
    fn publish_from_callback(&self) {
        if self.published.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut thread) = self.thread.try_lock() {
            *thread = Some(std::thread::current().id());
            self.published.store(true, Ordering::Release);
        }
    }

    /// Control-thread side, called once callbacks are quiesced.
    fn clear(&self) {
        self.published.store(false, Ordering::Release);
        self.with_slot(|thread| *thread = None);
    }

    fn get(&self) -> Option<ThreadId> {
        self.with_slot(|thread| *thread)
    }

    fn with_slot<R>(&self, update: impl FnOnce(&mut Option<ThreadId>) -> R) -> R {
        match self.thread.lock() {
            Ok(mut thread) => update(&mut thread),
            Err(poisoned) => {
                // The slot holds one `Copy` value, so a poisoned guard cannot
                // publish torn state; clear the poison and carry on.
                let mut thread = poisoned.into_inner();
                self.thread.inner().clear_poison();
                update(&mut thread)
            }
        }
    }
}

pub struct AudioIO<B: AudioBackend> {
    backend: B,
    sample_rate: AtomicU32,
    buffer_size: AtomicU32,
    position: Arc<Mutex<JackPosition>>,
    sync_active: AtomicBool,
    timebase_master: AtomicBool,
    transport_roll: AtomicBool,
    callback_thread: Arc<CallbackThreadSlot>,
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
            callback_thread: Arc::new(CallbackThreadSlot::default()),
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

    fn activate_callback(&mut self, callback: AudioCallbackFn) -> Result<(), String> {
        // A second activation would otherwise strand the running stream.
        if self.active {
            self.backend.close();
            self.active = false;
        }
        self.callback_thread.clear();
        let callback_thread = Arc::clone(&self.callback_thread);
        let mut callback = callback;
        // Publish the intended state before the call: a backend that starts
        // its stream and then reports an error is still torn down.
        self.active = true;
        if let Err(error) = self.backend.activate(Box::new(move |audio| {
            callback_thread.publish_from_callback();
            callback(audio);
        })) {
            self.backend.close();
            self.active = false;
            return Err(error);
        }
        self.refresh_transport();
        Ok(())
    }

    /// Refresh the cached transport diagnostics (`is_sync`,
    /// `is_timebase_master`, `is_transport_rolling`, `get_position`) from the
    /// backend. Call from the control thread, never from a callback.
    pub fn refresh_transport(&mut self) {
        let state = self.backend.transport_state();
        let external_transport = self.backend.supports_transport();
        self.transport_roll.store(state.rolling, Ordering::Release);
        self.sync_active
            .store(external_transport && state.rolling, Ordering::Release);
        // Backends without an external transport clock the session themselves.
        self.timebase_master
            .store(!external_transport, Ordering::Release);
        let mut position = self.position_guard();
        position.frame = state.frame;
        position.bar = state.bar;
        position.beat = state.beat;
        position.beats_per_minute = state.bpm;
        position.beats_per_bar = state.beats_per_bar;
        position.beat_type = state.beat_type;
        position.frame_rate = self.get_srate();
        position.valid = u32::from(state.rolling);
    }

    fn position_guard(&self) -> std::sync::MutexGuard<'_, JackPosition> {
        match self.position.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                // A panicking writer may have left a torn snapshot. Publish an
                // unknown position rather than partial frame/bar/beat data.
                let mut guard = poisoned.into_inner();
                *guard = JackPosition::default();
                self.position.clear_poison();
                guard
            }
        }
    }

    pub fn close(&mut self) {
        if self.active {
            self.backend.close();
            self.active = false;
        }
        self.callback_thread.clear();
        self.refresh_transport();
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
    /// Capture-to-DSP alignment delay reported by the active backend.
    pub fn input_latency_frames(&self) -> NFrames {
        self.backend.input_latency_frames()
    }
    /// Inspect backend-specific non-realtime state such as device route and
    /// callback health. Never call this from an audio callback.
    pub fn backend(&self) -> &B {
        &self.backend
    }
    /// Mutable access to the backend for control-thread operations that
    /// don't need the full `AudioIO` lifecycle (MIDI polling, diagnostics).
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
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
        let info = match self.backend.recover() {
            Ok(info) => info,
            Err(error) => {
                // The backend may have torn the route down before failing, so
                // callers must not keep believing the route is live.
                self.backend.close();
                self.active = false;
                self.callback_thread.clear();
                self.refresh_transport();
                return Err(error);
            }
        };
        self.sample_rate.store(info.sample_rate, Ordering::Release);
        self.buffer_size.store(info.buffer_size, Ordering::Release);
        self.callback_thread.clear();
        self.refresh_transport();
        Ok(())
    }
    pub fn recovery_metrics(&self) -> AudioRecoveryMetrics {
        self.backend.recovery_metrics()
    }
    pub fn get_position(&self) -> JackPosition {
        *self.position_guard()
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
        self.refresh_transport();
    }
    /// Thread currently running the audio callback, or `None` when callbacks
    /// are not running. Cleared by `close()` and `recover()`.
    pub fn callback_thread(&self) -> Option<ThreadId> {
        self.callback_thread.get()
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
        closes: u32,
        relocated: Option<NFrames>,
        recoveries: u64,
        fail_activate: bool,
        transport: TransportState,
        external_transport: bool,
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
                transport_rolling: false,
            };
            callback(&mut cb);
            assert_eq!(left, vec![2.0; 4]);
            if self.fail_activate {
                return Err("device unavailable".into());
            }
            self.activated = true;
            Ok(())
        }
        fn close(&mut self) {
            self.closes += 1;
            self.activated = false;
        }
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
        fn transport_state(&self) -> TransportState {
            self.transport
        }
        fn supports_transport(&self) -> bool {
            self.external_transport
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
    fn fake(info: BackendInfo) -> Fake {
        Fake {
            info,
            activated: false,
            closes: 0,
            relocated: None,
            recoveries: 0,
            fail_activate: false,
            transport: TransportState::default(),
            external_transport: false,
        }
    }
    #[test]
    fn backend_owns_mutable_processor_without_a_mutex() {
        let mut io = AudioIO::new(fake(BackendInfo {
            sample_rate: 48_000,
            buffer_size: 4,
        }));
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
    #[test]
    fn failed_activation_and_close_release_the_stream() {
        let mut io = AudioIO::new(fake(BackendInfo {
            sample_rate: 48_000,
            buffer_size: 4,
        }));
        io.open("test").unwrap();
        io.activate(Gain).unwrap();
        // A second activation must close the stream that is already running.
        io.activate(Gain).unwrap();
        assert_eq!(io.backend().closes, 1);
        io.close();
        assert_eq!(io.backend().closes, 2);
        assert_eq!(io.callback_thread(), None);

        let mut failing = fake(BackendInfo {
            sample_rate: 48_000,
            buffer_size: 4,
        });
        failing.fail_activate = true;
        let mut io = AudioIO::new(failing);
        io.open("test").unwrap();
        assert_eq!(io.activate(Gain).unwrap_err(), "device unavailable");
        // The backend started its stream before failing, so it must be closed.
        assert_eq!(io.backend().closes, 1);
        assert!(io.recover().is_err());
    }
    #[test]
    fn transport_diagnostics_follow_the_backend() {
        let mut backend = fake(BackendInfo {
            sample_rate: 48_000,
            buffer_size: 4,
        });
        backend.external_transport = true;
        backend.transport = TransportState {
            rolling: true,
            frame: 9_600,
            bar: 5,
            beat: 2,
            bpm: 128.0,
            beats_per_bar: 4.0,
            beat_type: 4,
        };
        let mut io = AudioIO::new(backend);
        io.open("test").unwrap();
        io.activate(Gain).unwrap();
        assert!(io.is_transport_rolling());
        assert!(io.is_sync());
        assert!(!io.is_timebase_master());
        let position = io.get_position();
        assert_eq!((position.frame, position.bar, position.beat), (9_600, 5, 2));
        assert_eq!(position.beats_per_minute, 128.0);

        // A backend without an external transport clocks the session itself.
        let mut io = AudioIO::new(fake(BackendInfo {
            sample_rate: 48_000,
            buffer_size: 4,
        }));
        io.open("test").unwrap();
        io.activate(Gain).unwrap();
        assert!(!io.is_transport_rolling());
        assert!(!io.is_sync());
        assert!(io.is_timebase_master());
    }
}
