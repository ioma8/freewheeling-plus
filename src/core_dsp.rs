//! Core realtime DSP primitives.
//!
//! The C++ implementation talks to the application, loop storage and event
//! graph through concrete classes.  Rust keeps those dependencies explicit:
//! [`DspApp`], [`LoopSource`] and [`Processor`] are the adapters used by the
//! processors below.  No processor hides an unavailable operation.

use crate::core_dsp_audio_buffers::{AudioBufferConfig, AudioBuffers, InputSettings};

pub type Sample = f32;
pub type NFrames = u32;

pub const SS_NONE: i32 = 0;
pub const SS_START: i32 = 1;
pub const SS_BEAT: i32 = 2;
pub const SS_END: i32 = 3;
pub const SS_ENDED: i32 = 4;

pub fn math_gcd(a: i32, b: i32) -> i32 {
    if b == 0 { a } else { math_gcd(b, a % b) }
}
pub fn math_lcm(a: i32, b: i32) -> i32 {
    a * b / math_gcd(a, b)
}

const DB_FLOOR: f32 = -1000.0;
fn iec_db_to_fader(db: f32) -> f32 {
    if db < -70.0 {
        0.0
    } else if db < -60.0 {
        (db + 70.0) * 0.25
    } else if db < -50.0 {
        (db + 60.0) * 0.5 + 2.5
    } else if db < -40.0 {
        (db + 50.0) * 0.75 + 7.5
    } else if db < -30.0 {
        (db + 40.0) * 1.5 + 15.0
    } else if db < -20.0 {
        (db + 30.0) * 2.0 + 30.0
    } else {
        (db + 20.0) * 2.5 + 50.0
    }
}
fn iec_fader_to_db(def: f32) -> f32 {
    if def >= 50.0 {
        (def - 50.0) / 2.5 - 20.0
    } else if def >= 30.0 {
        (def - 30.0) / 2.0 - 30.0
    } else if def >= 15.0 {
        (def - 15.0) / 1.5 - 40.0
    } else if def >= 7.5 {
        (def - 7.5) / 0.75 - 50.0
    } else if def >= 2.5 {
        (def - 2.5) / 0.5 - 60.0
    } else {
        def / 0.25 - 70.0
    }
}

pub struct AudioLevel;
impl AudioLevel {
    pub fn fader_to_db(level: f32, max_db: f32) -> f32 {
        if level == 0.0 {
            DB_FLOOR
        } else {
            iec_fader_to_db(level * iec_db_to_fader(max_db))
        }
    }
    pub fn db_to_fader(db: f32, max_db: f32) -> f32 {
        if db == DB_FLOOR {
            return 0.0;
        }
        (iec_db_to_fader(db) / iec_db_to_fader(max_db)).clamp(0.0, 1.0)
    }
}

pub trait DspApp {
    fn time_scale(&self) -> f32 {
        1.0
    }
}

pub trait Processor {
    fn process(&mut self, pre: bool, len: NFrames, buffers: &mut AudioBuffers);
    fn halt(&mut self) {}
    fn preprocess(&mut self) {}
}

/// Stateful smoothing shared by processors which change topology or gain.
pub struct SmoothState {
    pub pre_len: usize,
    pub prewritten: bool,
    pub prewriting: bool,
    pub pre: Vec<Vec<Sample>>,
}
impl SmoothState {
    pub fn new(outputs: usize, stereo: bool) -> Self {
        Self {
            pre_len: 64,
            prewritten: false,
            prewriting: false,
            pre: (0..outputs * if stereo { 2 } else { 1 })
                .map(|_| vec![0.0; 64])
                .collect(),
        }
    }
    pub fn fade(&mut self, outputs: &mut [Vec<Sample>]) {
        if !self.prewritten {
            return;
        }
        for (i, out) in outputs.iter_mut().enumerate() {
            for n in 0..self.pre_len.min(out.len()) {
                let r = n as f32 / self.pre_len as f32;
                out[n] = out[n] * r + self.pre[i][n] * (1.0 - r);
            }
        }
        self.prewritten = false;
    }
}

pub struct AutoLimitProcessor {
    pub current_volume: f32,
    pub target_volume: f32,
    pub delta: f32,
    pub threshold: f32,
    pub max_gain: f32,
    pub frozen: bool,
}
impl AutoLimitProcessor {
    pub fn new(threshold: f32, release_rate: f32, max_gain: f32) -> Self {
        Self {
            current_volume: 1.0,
            target_volume: 1.0,
            delta: release_rate,
            threshold,
            max_gain,
            frozen: false,
        }
    }
    pub fn reset(&mut self) {
        self.current_volume = 1.0;
        self.target_volume = 1.0;
        self.frozen = false;
    }
    pub fn process_channels(&mut self, left: &mut [Sample], mut right: Option<&mut [Sample]>) {
        let mut max: f32 = 0.0;
        let mut clips = 0;
        for n in 0..left.len() {
            let mut vals = [left[n], right.as_ref().map_or(0.0, |r| r[n])];
            for v in &mut vals {
                let a = v.abs();
                max = max.max(a);
                *v *= self.current_volume;
                if v.abs() > self.threshold {
                    clips += 1;
                }
                *v = v.clamp(-0.99, 0.99);
            }
            left[n] = vals[0];
            if let Some(r) = right.as_mut() {
                r[n] = vals[1];
            }
        }
        if !self.frozen && (clips > 0 || max > self.threshold) && max > 0.0 {
            self.target_volume = (self.threshold / max).min(self.max_gain);
        }
        self.current_volume += (self.target_volume - self.current_volume).signum() * self.delta;
        if (self.current_volume - self.target_volume).abs() < self.delta {
            self.current_volume = self.target_volume;
        }
    }
}

pub struct Pulse {
    pub len: NFrames,
    pub curpos: NFrames,
    pub wrapped: bool,
    pub stopped: bool,
    pub metro_active: bool,
    pub metro_volume: f32,
}
impl Pulse {
    pub const METRONOME_HIT_LEN: NFrames = 800;
    pub const METRONOME_TONE_LEN: NFrames = 4400;
    pub const METRONOME_INIT_VOL: f32 = 0.1;
    pub fn new(len: NFrames, startpos: NFrames) -> Self {
        Self {
            len,
            curpos: startpos,
            wrapped: false,
            stopped: false,
            metro_active: false,
            metro_volume: 0.1,
        }
    }
    pub fn quantize_length(&self, src: NFrames) -> NFrames {
        if self.len == 0 {
            src
        } else {
            ((src as f32 / self.len as f32).round() as NFrames) * self.len
        }
    }
    pub fn wrap(&mut self) {
        self.curpos = self.len;
    }
    pub fn set_pos(&mut self, p: NFrames) {
        self.curpos = p
    }
    pub fn process_clock(&mut self, n: NFrames) {
        if !self.stopped {
            self.curpos += n;
            if self.curpos >= self.len {
                self.curpos %= self.len;
                self.wrapped = true;
            }
        }
    }
    pub fn take_wrapped(&mut self) -> bool {
        let v = self.wrapped;
        self.wrapped = false;
        v
    }
}

pub struct PassthroughProcessor<'a, C: AudioBufferConfig> {
    pub settings: &'a mut InputSettings,
    pub config: C,
    pub input_volume: f32,
}
impl<'a, C: AudioBufferConfig> PassthroughProcessor<'a, C> {
    pub fn process(&mut self, len: NFrames, source: &AudioBuffers, dest: &mut [&mut [Sample]]) {
        source.mix_inputs(
            len,
            dest,
            self.settings,
            self.input_volume,
            false,
            &self.config,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fader_round_trip() {
        for db in [-60.0, -40.0, -20.0, 0.0] {
            let f = AudioLevel::db_to_fader(db, 0.);
            assert!((AudioLevel::fader_to_db(f, 0.) - db).abs() < 0.01);
        }
    }
    #[test]
    fn pulse_wraps() {
        let mut p = Pulse::new(4, 0);
        p.process_clock(4);
        assert!(p.take_wrapped());
        assert_eq!(p.curpos, 0);
    }
}
