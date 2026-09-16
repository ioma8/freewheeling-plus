//! OS-neutral concrete audio transport and callback adapter.

use crate::audioio::{
    AudioBackend, AudioCallback, AudioCallbackFn, AudioMetrics, BackendInfo, JackPosition, NFrames,
    NUM_CHANNELS, Sample,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

/// Control-thread view of the transport state.
///
/// The rolling flag is *not* part of this model: it is read on the audio
/// thread through [`AudioIoPlatform::transport_rolling`], which is lock-free.
#[derive(Clone, Debug, Default)]
pub struct TransportModel {
    pub position: JackPosition,
    pub timebase_master: bool,
    pub sync_active: bool,
    pub relocated: Option<NFrames>,
}

impl TransportModel {
    pub fn timebase_callback(&mut self, position: JackPosition, new_position: bool) {
        self.position = position;
        self.sync_active = true;
        if new_position {
            self.relocated = Some(position.frame);
        }
    }
    pub fn relocate(&mut self, frame: NFrames) {
        self.relocated = Some(frame);
        self.position.frame = frame;
    }
}

pub struct AudioIoPlatform {
    info: BackendInfo,
    callback: Option<AudioCallbackFn>,
    /// Realtime-safe rolling flag, shared with the audio callback so it never
    /// has to take the transport lock.
    rolling: Arc<AtomicBool>,
    /// Control-thread transport state. Private: the locking discipline (and
    /// poison recovery) stays inside this module.
    transport: Arc<Mutex<TransportModel>>,
    metrics: AudioMetrics,
}

impl AudioIoPlatform {
    pub fn new(sample_rate: NFrames, buffer_size: NFrames) -> Self {
        Self {
            info: BackendInfo {
                sample_rate,
                buffer_size,
            },
            callback: None,
            rolling: Arc::new(AtomicBool::new(false)),
            transport: Arc::new(Mutex::new(TransportModel::default())),
            metrics: AudioMetrics::default(),
        }
    }

    /// Mean callback load since activation, as a fraction of the callback
    /// period. A transient spike is diluted by the surrounding callbacks; use
    /// [`Self::peak_cpu_load`] to see it.
    pub fn cpu_load(&self) -> f32 {
        if self.metrics.callback_frames == 0 || self.info.sample_rate == 0 {
            return 0.0;
        }
        let period = self.metrics.callback_frames as f64 / self.info.sample_rate as f64;
        (self.metrics.callback_total_nanos as f64 / 1_000_000_000.0 / period) as f32
    }

    /// Worst callback load seen since activation, as a fraction of the nominal
    /// callback period (`buffer_size / sample_rate`).
    pub fn peak_cpu_load(&self) -> f32 {
        if self.info.sample_rate == 0 || self.info.buffer_size == 0 {
            return 0.0;
        }
        let nominal = f64::from(self.info.buffer_size) / f64::from(self.info.sample_rate);
        (self.metrics.callback_peak_nanos as f64 / 1_000_000_000.0 / nominal) as f32
    }

    /// Publish whether an external transport is rolling. Lock-free, so this is
    /// safe for the audio callback to observe.
    pub fn set_transport_rolling(&self, rolling: bool) {
        self.rolling.store(rolling, Ordering::Release);
    }

    /// Whether an external transport is rolling.
    pub fn transport_rolling(&self) -> bool {
        self.rolling.load(Ordering::Acquire)
    }

    /// Copy of the control-thread transport state.
    ///
    /// Never call this from an audio callback: the model is behind a lock (the
    /// realtime path reads only [`Self::transport_rolling`], which is atomic).
    pub fn transport_snapshot(&self) -> TransportModel {
        self.transport_guard().clone()
    }

    /// Publish the position an audio callback ran at.
    ///
    /// Control-thread call; see [`Self::transport_snapshot`] for why the
    /// callback itself cannot do this.
    pub fn publish_position(&self, position: JackPosition) {
        self.transport_guard().position = position;
    }

    /// Record a JACK-style timebase callback.
    pub fn timebase_callback(&self, position: JackPosition, new_position: bool) {
        self.transport_guard()
            .timebase_callback(position, new_position);
    }

    fn transport_guard(&self) -> MutexGuard<'_, TransportModel> {
        match self.transport.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                // A panicking writer may have left a partially updated model.
                // Publish a known-good default instead of torn state, and
                // report the poison: silently recovering would hide the panic
                // that caused it.
                eprintln!(
                    "FreeWheeling: audio transport mutex was poisoned by a panicking writer; \
                     resetting the transport model"
                );
                let mut guard = poisoned.into_inner();
                *guard = TransportModel::default();
                self.transport.clear_poison();
                guard
            }
        }
    }

    pub fn invoke_callback(
        &mut self,
        inputs: [&[Sample]; NUM_CHANNELS],
        outputs: [&mut [Sample]; NUM_CHANNELS],
        nframes: NFrames,
        position: JackPosition,
    ) -> Result<(), String> {
        let frames = nframes as usize;
        if let Some(short) = inputs.iter().find(|slice| slice.len() < frames) {
            return Err(format!(
                "audio callback input is {} frames short of {nframes}",
                frames - short.len()
            ));
        }
        if let Some(short) = outputs.iter().find(|slice| slice.len() < frames) {
            return Err(format!(
                "audio callback output is {} frames short of {nframes}",
                frames - short.len()
            ));
        }
        // Only the lock-free rolling flag crosses into this path: taking the
        // transport lock here would block (and, with std's mutex, allocate) on
        // the realtime thread. See tests/owned_callback_rt_safety.rs.
        let rolling = Arc::clone(&self.rolling);
        let callback = self
            .callback
            .as_mut()
            .ok_or_else(|| "audio callback is not activated".to_string())?;
        let mut cb = AudioCallback {
            inputs,
            outputs,
            nframes,
            position,
            transport_rolling: rolling.load(Ordering::Acquire),
        };
        let started = Instant::now();
        callback(&mut cb);
        let nanos = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.metrics.callbacks = self.metrics.callbacks.saturating_add(1);
        self.metrics.callback_frames = self
            .metrics
            .callback_frames
            .saturating_add(u64::from(nframes));
        self.metrics.callback_total_nanos = self.metrics.callback_total_nanos.saturating_add(nanos);
        self.metrics.callback_peak_nanos = self.metrics.callback_peak_nanos.max(nanos);
        Ok(())
    }
}

impl AudioBackend for AudioIoPlatform {
    fn open(&mut self, _: &str) -> Result<BackendInfo, String> {
        Ok(self.info)
    }
    fn activate(&mut self, callback: AudioCallbackFn) -> Result<(), String> {
        self.callback = Some(callback);
        Ok(())
    }
    fn close(&mut self) {
        self.callback = None;
    }
    fn relocate(&mut self, frame: NFrames) {
        self.transport_guard().relocate(frame);
    }
    fn metrics(&self) -> AudioMetrics {
        self.metrics
    }
    /// Without this override `AudioIO::get_cpu_load` reports `None` for this
    /// backend even though the same computation exists here.
    fn cpu_load(&self) -> Option<f32> {
        Some(AudioIoPlatform::cpu_load(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_callback_and_relocation_preserve_state() {
        let mut t = TransportModel::default();
        let p = JackPosition {
            frame: 42,
            bar: 3,
            beat: 2,
            ..Default::default()
        };
        t.timebase_callback(p, true);
        assert_eq!(t.position, p);
        assert!(t.sync_active);
        assert_eq!(t.relocated, Some(42));
        t.relocate(99);
        assert_eq!(t.position.frame, 99);
    }
    #[test]
    fn callback_adapter_returns_error_before_activation_and_runs_after() {
        let mut b = AudioIoPlatform::new(48_000, 4);
        let mut out = [vec![0.0; 4], vec![0.0; 4]];
        let input = [vec![1.0; 4], vec![2.0; 4]];
        let (out_left, out_right) = out.split_at_mut(1);
        assert!(
            b.invoke_callback(
                [&input[0], &input[1]],
                [&mut out_left[0], &mut out_right[0]],
                4,
                Default::default()
            )
            .is_err()
        );
        b.activate(Box::new(|cb| cb.outputs[0].fill(cb.inputs[0][0])))
            .unwrap();
        b.invoke_callback(
            [&input[0], &input[1]],
            [&mut out_left[0], &mut out_right[0]],
            4,
            Default::default(),
        )
        .unwrap();
        assert_eq!(out[0], vec![1.0; 4]);
    }
    #[test]
    fn callback_rejects_short_buffers_instead_of_panicking_in_dsp() {
        let mut b = AudioIoPlatform::new(48_000, 8);
        b.activate(Box::new(|cb| cb.outputs[0][cb.nframes as usize - 1] = 1.0))
            .unwrap();
        let input = [vec![0.0; 8], vec![0.0; 8]];
        let mut out = [vec![0.0; 4], vec![0.0; 8]];
        let (out_left, out_right) = out.split_at_mut(1);
        let error = b
            .invoke_callback(
                [&input[0], &input[1]],
                [&mut out_left[0], &mut out_right[0]],
                8,
                Default::default(),
            )
            .unwrap_err();
        assert!(error.contains("output is 4 frames short"), "{error}");
    }
    #[test]
    fn rolling_flag_reaches_the_callback_and_cpu_load_is_reported() {
        let mut b = AudioIoPlatform::new(48_000, 8);
        let seen = Arc::new(Mutex::new(None));
        let observed = Arc::clone(&seen);
        b.activate(Box::new(move |cb| {
            *observed.lock().unwrap() = Some((cb.transport_rolling, cb.position.frame));
        }))
        .unwrap();
        b.set_transport_rolling(true);
        assert!(b.transport_rolling());
        let input = [vec![0.0; 8], vec![0.0; 8]];
        let mut out = [vec![0.0; 8], vec![0.0; 8]];
        let (out_left, out_right) = out.split_at_mut(1);
        b.invoke_callback(
            [&input[0], &input[1]],
            [&mut out_left[0], &mut out_right[0]],
            8,
            JackPosition {
                frame: 4096,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(*seen.lock().unwrap(), Some((true, 4096)));
        // The callback path never takes the control-thread lock: the caller
        // publishes the position it handed to the callback.
        b.publish_position(JackPosition {
            frame: 4096,
            ..Default::default()
        });
        assert_eq!(b.transport_snapshot().position.frame, 4096);
        assert!(AudioBackend::cpu_load(&b).is_some());
    }
    #[test]
    fn poisoned_transport_is_reset_to_a_known_good_model() {
        let b = AudioIoPlatform::new(48_000, 8);
        b.timebase_callback(
            JackPosition {
                frame: 7,
                ..Default::default()
            },
            false,
        );
        assert_eq!(b.transport_snapshot().position.frame, 7);
        let transport = Arc::clone(&b.transport);
        let _ = std::thread::spawn(move || {
            let _guard = transport.lock().unwrap();
            panic!("poison the transport model");
        })
        .join();
        let snapshot = b.transport_snapshot();
        assert_eq!(snapshot.position, JackPosition::default());
        assert!(!snapshot.sync_active);
        assert_eq!(b.transport_snapshot().position.frame, 0);
    }
}
