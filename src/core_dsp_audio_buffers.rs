//! Audio buffer and input-mixing primitives translated from
//! `fweelin_core_dsp_audio_buffers.cc`.

use std::sync::Arc;

pub type Sample = f32;
pub type NFrames = u32;

const MAX_DVOL: f32 = 1.5;
const DCOFS_MINIMUM_SAMPLE_COUNT: u64 = 10_000;
const DCOFS_LOWPASS_COEFF: f32 = 0.99;
const DCOFS_ONEMINUS_LOWPASS_COEFF: f32 = 0.01;

/// The small part of the application configuration used by `AudioBuffers`.
pub trait AudioBufferConfig {
    fn is_stereo_input(&self, input: usize) -> bool;
    fn is_stereo_output(&self, output: usize) -> bool;
    fn is_stereo_master(&self) -> bool;
    fn sample_rate(&self) -> NFrames;
}

#[derive(Clone, Debug)]
pub struct InputSettings {
    pub selected: Vec<bool>,
    pub input_volumes: Vec<f32>,
    pub delta_input_volumes: Vec<f32>,
    pub input_sums: [Vec<Sample>; 2],
    pub input_averages: [Vec<Sample>; 2],
    pub input_peaks: Vec<Sample>,
    pub input_peak_times: Vec<u64>,
    pub input_counts: Vec<u64>,
}

impl InputSettings {
    pub fn new(numins: usize) -> Self {
        Self {
            selected: vec![true; numins],
            input_volumes: vec![1.0; numins],
            delta_input_volumes: vec![1.0; numins],
            input_sums: [vec![0.0; numins], vec![0.0; numins]],
            input_averages: [vec![0.0; numins], vec![0.0; numins]],
            input_peaks: vec![0.0; numins],
            input_peak_times: vec![0; numins],
            input_counts: vec![0; numins],
        }
    }
    pub fn select_input(&mut self, n: usize, selected: bool) {
        if let Some(v) = self.selected.get_mut(n) {
            *v = selected;
        }
    }
    pub fn input_selected(&self, n: usize) -> bool {
        self.selected.get(n).copied().unwrap_or(false)
    }
    pub fn is_selected_stereo<C: AudioBufferConfig>(&self, config: &C) -> bool {
        self.selected
            .iter()
            .enumerate()
            .any(|(i, selected)| *selected && config.is_stereo_input(i))
    }
    pub fn adjust_input_vol(&mut self, n: usize, adjust: f32, time_scale: f32) {
        if let Some(v) = self.delta_input_volumes.get_mut(n) {
            if *v < MAX_DVOL {
                *v += adjust * time_scale;
            }
            if *v < 0.0 {
                *v = 0.0;
            }
        }
    }
    pub fn copy_from(&mut self, source: &Self) -> bool {
        if self.selected.len() != source.selected.len() {
            return false;
        }
        self.clone_from(source);
        true
    }
}

#[derive(Clone, Debug)]
pub struct AudioBuffers {
    pub numins_ext: usize,
    /// C++'s input-source constructor aliases the source pointer arrays.
    /// `Arc` preserves that shared immutable sample storage in safe Rust;
    /// output slots remain independently owned.
    pub inputs: [Vec<Option<Arc<Vec<Sample>>>>; 2],
    pub outputs: [Vec<Option<Vec<Sample>>>; 2],
}

impl AudioBuffers {
    pub fn new(numins_ext: usize, internal_inputs: usize, numouts: usize) -> Self {
        Self {
            numins_ext,
            inputs: [
                vec![None; numins_ext + internal_inputs],
                vec![None; numins_ext + internal_inputs],
            ],
            outputs: [vec![None; numouts], vec![None; numouts]],
        }
    }
    pub fn num_inputs(&self) -> usize {
        self.inputs[0].len()
    }
    pub fn num_outputs(&self) -> usize {
        self.outputs[0].len()
    }
    /// Construct a buffer set with the input topology of `source` and fresh
    /// output slots, matching the C++ input-source constructor.
    pub fn from_input_source(source: &Self) -> Self {
        Self {
            numins_ext: source.numins_ext,
            // Cloning `Arc`s is the direct Rust equivalent of C++ assigning
            // `ins[channel] = input_source->ins[channel]`.
            inputs: source.inputs.clone(),
            outputs: [
                vec![None; source.num_outputs()],
                vec![None; source.num_outputs()],
            ],
        }
    }
    pub fn input(&self, n: usize, channel: usize) -> Option<&[Sample]> {
        self.inputs
            .get(channel)?
            .get(n)?
            .as_deref()
            .map(Vec::as_slice)
    }
    pub fn output(&self, n: usize, channel: usize) -> Option<&[Sample]> {
        self.outputs.get(channel)?.get(n)?.as_deref()
    }
    pub fn resize(&mut self, len: usize) {
        for channel in &mut self.inputs {
            for buffer in channel.iter_mut().flatten() {
                Arc::make_mut(buffer).resize(len, 0.0);
            }
        }
        for channel in &mut self.outputs {
            for buffer in channel.iter_mut().flatten() {
                buffer.resize(len, 0.0);
            }
        }
    }
    pub fn set_input(&mut self, channel: usize, n: usize, data: Vec<Sample>) {
        if let Some(slot) = self.inputs.get_mut(channel).and_then(|c| c.get_mut(n)) {
            *slot = Some(Arc::new(data));
        }
    }
    pub fn set_output(&mut self, channel: usize, n: usize, data: Vec<Sample>) {
        if let Some(slot) = self.outputs.get_mut(channel).and_then(|c| c.get_mut(n)) {
            *slot = Some(data);
        }
    }
    pub fn is_stereo_input<C: AudioBufferConfig>(&self, c: &C, n: usize) -> bool {
        c.is_stereo_input(n)
    }
    pub fn is_stereo_output<C: AudioBufferConfig>(&self, c: &C, n: usize) -> bool {
        c.is_stereo_output(n)
    }
    pub fn is_stereo_master<C: AudioBufferConfig>(&self, c: &C) -> bool {
        c.is_stereo_master()
    }

    pub fn mix_inputs<C: AudioBufferConfig>(
        &self,
        len: NFrames,
        dest: &mut [&mut [Sample]],
        settings: &mut InputSettings,
        input_vol: f32,
        compute_stats: bool,
        config: &C,
    ) {
        let len = len as usize;
        if dest.is_empty() || dest[0].len() < len {
            return;
        }
        let stereo = dest.len() > 1 && dest[1].len() >= len;
        dest[0][..len].fill(0.0);
        if stereo {
            dest[1][..len].fill(0.0);
        }
        let hold = config.sample_rate() as u64;
        for i in 0..self.num_inputs().min(settings.selected.len()) {
            if !settings.selected[i] {
                continue;
            }
            let Some(in0) = self.inputs[0][i].as_ref() else {
                continue;
            };
            if in0.len() < len {
                continue;
            }
            let in1 = self.inputs[1][i]
                .as_ref()
                .filter(|b| b.len() >= len)
                .unwrap_or(in0);
            let vol = settings.input_volumes[i] * input_vol;
            let mut count = settings.input_counts[i];
            let mut peak_time = settings.input_peak_times[i];
            let mut peak = if compute_stats && count.saturating_sub(peak_time) > hold {
                0.0
            } else {
                settings.input_peaks[i]
            };
            for k in 0..len {
                let s = in0[k];
                if compute_stats {
                    let abs = s.abs();
                    if abs > peak {
                        peak = abs;
                        peak_time = count;
                    }
                    count += 1;
                    settings.input_sums[0][i] += s;
                }
                dest[0][k] += (s - settings.input_averages[0][i]) * vol;
            }
            if compute_stats {
                settings.input_counts[i] = count;
                settings.input_peak_times[i] = peak_time;
                settings.input_peaks[i] = peak;
                if count > DCOFS_MINIMUM_SAMPLE_COUNT {
                    settings.input_averages[0][i] = DCOFS_LOWPASS_COEFF
                        * settings.input_averages[0][i]
                        + DCOFS_ONEMINUS_LOWPASS_COEFF * settings.input_sums[0][i] / count as f32;
                }
            }
            if stereo {
                for k in 0..len {
                    let s = in1[k];
                    dest[1][k] += (s - settings.input_averages[1][i]) * vol;
                    if compute_stats {
                        // C++ keeps one linked per-input peak meter: the
                        // right channel may raise the same peak/time as the
                        // left channel, even though `cnt` advances only
                        // during the left-channel pass.
                        let abs = s.abs();
                        if abs > peak {
                            peak = abs;
                            peak_time = count;
                        }
                        settings.input_sums[1][i] += s;
                    }
                }
                if compute_stats {
                    settings.input_peaks[i] = peak;
                    settings.input_peak_times[i] = peak_time;
                    if count > DCOFS_MINIMUM_SAMPLE_COUNT {
                        settings.input_averages[1][i] = DCOFS_LOWPASS_COEFF
                            * settings.input_averages[1][i]
                            + DCOFS_ONEMINUS_LOWPASS_COEFF * settings.input_sums[1][i]
                                / count as f32;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Config;
    impl AudioBufferConfig for Config {
        fn is_stereo_input(&self, input: usize) -> bool {
            input == 0
        }
        fn is_stereo_output(&self, output: usize) -> bool {
            output == 0
        }
        fn is_stereo_master(&self) -> bool {
            true
        }
        fn sample_rate(&self) -> NFrames {
            48_000
        }
    }

    #[test]
    fn mixes_selected_stereo_input_and_updates_statistics() {
        let mut buffers = AudioBuffers::new(1, 0, 1);
        buffers.set_input(0, 0, vec![1.0, -0.5]);
        buffers.set_input(1, 0, vec![0.25, 0.5]);
        let mut settings = InputSettings::new(1);
        settings.input_volumes[0] = 0.5;
        let mut left = [0.0; 2];
        let mut right = [0.0; 2];
        let mut dest: [&mut [Sample]; 2] = [&mut left, &mut right];

        buffers.mix_inputs(2, &mut dest, &mut settings, 1.0, true, &Config);

        assert_eq!(left, [0.5, -0.25]);
        assert_eq!(right, [0.125, 0.25]);
        assert_eq!(settings.input_counts[0], 2);
        assert_eq!(settings.input_peaks[0], 1.0);
    }

    #[test]
    fn stereo_peak_hold_is_linked_like_cpp_mixinputs() {
        let mut buffers = AudioBuffers::new(1, 0, 1);
        buffers.set_input(0, 0, vec![0.1]);
        buffers.set_input(1, 0, vec![-0.9]);
        let mut settings = InputSettings::new(1);
        let mut left = [0.0];
        let mut right = [0.0];
        let mut dest: [&mut [Sample]; 2] = [&mut left, &mut right];

        buffers.mix_inputs(1, &mut dest, &mut settings, 1.0, true, &Config);

        assert_eq!(settings.input_peaks[0], 0.9);
        // The legacy right-channel pass stamps the already advanced count.
        assert_eq!(settings.input_peak_times[0], 1);
    }

    #[test]
    fn input_source_constructor_aliases_callback_samples_like_cpp() {
        let mut source = AudioBuffers::new(1, 0, 1);
        source.set_input(0, 0, vec![0.25, -0.5]);
        let shared = AudioBuffers::from_input_source(&source);
        let source_samples = source.inputs[0][0].as_ref().unwrap();
        let shared_samples = shared.inputs[0][0].as_ref().unwrap();
        assert!(Arc::ptr_eq(source_samples, shared_samples));
        assert_eq!(shared.input(0, 0), Some(&[0.25, -0.5][..]));
        assert!(shared.outputs[0][0].is_none());
    }

    #[test]
    fn dc_offset_and_peak_hold_follow_cpp_running_counters() {
        let mut buffers = AudioBuffers::new(1, 0, 1);
        buffers.set_input(0, 0, vec![1.0; 10_001]);
        let mut settings = InputSettings::new(1);
        let mut destination = vec![0.0; 10_001];
        let mut dest: [&mut [Sample]; 1] = [&mut destination];
        buffers.mix_inputs(10_001, &mut dest, &mut settings, 1.0, true, &Config);
        // C++ begins its low-pass DC estimate only after the running count
        // passes 10,000: 0.99 * 0 + 0.01 * (10001 / 10001).
        assert!((settings.input_averages[0][0] - 0.01).abs() < 0.000_001);
        assert_eq!(settings.input_counts[0], 10_001);

        buffers.set_input(0, 0, vec![0.0; 1]);
        let mut one = [0.0];
        {
            let mut single: [&mut [Sample]; 1] = [&mut one];
            buffers.mix_inputs(1, &mut single, &mut settings, 1.0, true, &Config);
        }
        // The output uses the pre-update DC estimate exactly like C++.
        assert!((one[0] + 0.01).abs() < 0.000_001);
        assert!((settings.input_averages[0][0] - 0.019_899).abs() < 0.000_001);

        settings.input_counts[0] = 48_002;
        settings.input_peak_times[0] = 0;
        settings.input_peaks[0] = 0.9;
        {
            let mut single: [&mut [Sample]; 1] = [&mut one];
            buffers.mix_inputs(1, &mut single, &mut settings, 1.0, true, &Config);
        }
        assert_eq!(settings.input_peaks[0], 0.0);
    }
}
