//! OS-neutral concrete audio transport and callback adapter.

use crate::audioio::{
    AudioBackend, AudioCallback, AudioCallbackFn, AudioMetrics, BackendInfo, JackPosition, NFrames,
    NUM_CHANNELS, Sample,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub struct TransportModel {
    pub position: JackPosition,
    pub timebase_master: bool,
    pub sync_active: bool,
    pub rolling: bool,
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
    pub transport: Arc<Mutex<TransportModel>>,
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
            transport: Arc::new(Mutex::new(TransportModel::default())),
            metrics: AudioMetrics::default(),
        }
    }
    pub fn cpu_load(&self) -> f32 {
        if self.metrics.callback_frames == 0 || self.info.sample_rate == 0 {
            return 0.0;
        }
        let period = self.metrics.callback_frames as f64 / self.info.sample_rate as f64;
        (self.metrics.callback_total_nanos as f64 / 1_000_000_000.0 / period) as f32
    }
    pub fn invoke_callback(
        &mut self,
        inputs: [&[Sample]; NUM_CHANNELS],
        outputs: [&mut [Sample]; NUM_CHANNELS],
        nframes: NFrames,
        position: JackPosition,
    ) -> Result<(), String> {
        let callback = self
            .callback
            .as_mut()
            .ok_or_else(|| "audio callback is not activated".to_string())?;
        let mut cb = AudioCallback {
            inputs,
            outputs,
            nframes,
            position,
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
        self.transport
            .lock()
            .expect("transport poisoned")
            .relocate(frame);
    }
    fn metrics(&self) -> AudioMetrics {
        self.metrics
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
}
