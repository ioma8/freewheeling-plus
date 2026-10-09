//! Core realtime DSP primitives.
//!
//! The C++ implementation talks to the application, loop storage and event
//! graph through concrete classes.  Rust keeps those dependencies explicit:
//! [`LoopSource`] and [`Processor`] are the adapters used by the
//! processors below.  No processor hides an unavailable operation.

use crate::core_dsp_audio_buffers::{AudioBufferConfig, AudioBuffers, InputSettings};

pub type Sample = f32;
pub type NFrames = u32;

/// Flush an `f32` to exactly zero when it has decayed below the smallest
/// normal value.
///
/// Multiply-chained signals (feedback fades, gain decay) reach IEEE-754
/// subnormal territory on x86-64 and pay a microcode penalty per arithmetic
/// unit on every subsequent operation, which is a well-known real-time audio
/// stall: several silent-but-live samples can push a whole callback past its
/// deadline. Zero decayed-anyway content costs nothing and is inaudible
/// (< -783 dB). Values in the normal range are returned untouched.
#[inline]
pub fn flush_subnormal(value: f32) -> f32 {
    if value != 0.0 && value.abs() < f32::MIN_POSITIVE {
        0.0
    } else {
        value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SyncState {
    None = 0,
    Start = 1,
    Beat = 2,
    End = 3,
    Ended = 4,
}

impl From<i32> for SyncState {
    fn from(value: i32) -> Self {
        match value {
            0 => SyncState::None,
            1 => SyncState::Start,
            2 => SyncState::Beat,
            3 => SyncState::End,
            4 => SyncState::Ended,
            _ => SyncState::None,
        }
    }
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
        // Invariant: after fade, the output converges to the new signal.
        // At n=0: out*0 + pre*1 = pre (old signal).
        // At n=pre_len: out*1 + pre*0 = out (new signal).
        // A shorter output or pre-buffer simply fades over fewer samples; the
        // loop must not index either of them out of bounds.
        for (i, out) in outputs.iter_mut().enumerate() {
            let Some(pre) = self.pre.get(i) else {
                continue;
            };
            for n in 0..self.pre_len.min(out.len()).min(pre.len()) {
                let r = n as f32 / self.pre_len as f32;
                out[n] = out[n] * r + pre[n] * (1.0 - r);
            }
        }
        self.prewritten = false;
    }

    pub fn dopreprocess(
        &mut self,
        outputs: &mut [Vec<Sample>],
        process: &mut dyn FnMut(&mut [Vec<Sample>]),
    ) {
        // Store current output as pre-buffer
        for (i, out) in outputs.iter().enumerate() {
            if let Some(pre) = self.pre.get_mut(i) {
                let len = pre.len().min(out.len());
                pre[..len].copy_from_slice(&out[..len]);
            }
        }
        self.prewritten = true;
        self.prewriting = true;
        process(outputs);
        self.prewriting = false;
    }

    pub fn fade_or_process(&mut self, outputs: &mut [Vec<Sample>], process: &mut dyn FnMut(&mut [Vec<Sample>])) {
        if !self.prewritten {
            process(outputs);
        } else {
            self.fade(outputs);
        }
    }
}

/// Final format-safety ceiling. The limiter's own ceiling is its threshold
/// bounded by this value.
pub const SAFETY_CEILING: f32 = 0.99;

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
    /// Ceiling applied after the gain: the configured threshold, bounded by
    /// the format safety limit. Deriving it from `threshold` keeps the clamp
    /// from truncating below the limit the limiter is aiming for.
    pub fn output_ceiling(&self) -> f32 {
        if self.threshold.is_finite() {
            self.threshold.clamp(0.0, SAFETY_CEILING)
        } else {
            SAFETY_CEILING
        }
    }

    /// Gain ceiling: `max_gain` bounds the released gain, and unity is the
    /// hard upper bound because the limiter must never amplify a block.
    pub fn gain_ceiling(&self) -> f32 {
        if self.max_gain.is_finite() {
            self.max_gain.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    pub fn process_channels(&mut self, left: &mut [Sample], right: Option<&mut [Sample]>) {
        // A right channel that is shorter than the left one cannot be written
        // for the whole block; drop it instead of indexing past its end.
        let mut right = right.filter(|right| right.len() >= left.len());
        let ceiling = self.output_ceiling();
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
                *v = v.clamp(-ceiling, ceiling);
            }
            left[n] = vals[0];
            if let Some(r) = right.as_mut() {
                r[n] = vals[1];
            }
        }
        if !self.frozen && max > 0.0 {
            let ceiling = self.gain_ceiling();
            if clips > 0 || max > self.threshold {
                // Clipping: attenuate so the block fits under the threshold.
                self.target_volume = (self.threshold / max).clamp(0.0, ceiling);
            } else {
                // Material is below the threshold: release the gain back
                // towards the ceiling, otherwise the channel stays attenuated
                // for the rest of the session.
                self.target_volume = ceiling;
            }
        }
        self.current_volume += (self.target_volume - self.current_volume).signum() * self.delta;
        if (self.current_volume - self.target_volume).abs() < self.delta {
            self.current_volume = self.target_volume;
        }
    }
}

pub struct SyncPosition {
    pub position: NFrames,
    pub callback: Box<dyn FnMut(i32, NFrames) + Send>,
    pub index: i32,
}

pub struct Pulse {
    pub len: NFrames,
    pub curpos: NFrames,
    pub wrapped: bool,
    pub stopped: bool,
    pub metro_active: bool,
    pub metro_volume: f32,
    sync_positions: Vec<SyncPosition>,
    max_sync_positions: usize,
    /// Monotonic id source: reusing the vector length would hand out an index
    /// that an existing sync position still holds after `del_sync`.
    next_sync_index: i32,
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
            sync_positions: Vec::new(),
            max_sync_positions: 1000,
            next_sync_index: 0,
        }
    }
    pub fn quantize_length(&self, src: NFrames) -> NFrames {
        if self.len == 0 {
            src
        } else {
            // Round to the nearest whole pulse length in u64: frame counts are
            // wide enough to overflow the u32 product.
            let pulses = (u64::from(src) + u64::from(self.len) / 2) / u64::from(self.len);
            pulses
                .saturating_mul(u64::from(self.len))
                .min(u64::from(u32::MAX)) as NFrames
        }
    }
    pub fn wrap(&mut self) {
        self.curpos = self.len;
    }
    pub fn set_pos(&mut self, p: NFrames) {
        self.curpos = p
    }
    pub fn process_clock(&mut self, n: NFrames) {
        let prev_curpos = self.curpos;
        // A zero-length pulse has nothing to wrap to; `curpos %= len` would
        // divide by zero on the realtime clock path.
        if !self.stopped && self.len > 0 {
            self.curpos += n;
            if self.curpos >= self.len {
                self.curpos %= self.len;
                self.wrapped = true;
            }
        }
        self.fire_syncs(prev_curpos, self.curpos);
    }
    pub fn take_wrapped(&mut self) -> bool {
        let v = self.wrapped;
        self.wrapped = false;
        v
    }
    pub fn add_sync(&mut self, pos: NFrames, cb: Box<dyn FnMut(i32, NFrames) + Send>) -> Result<i32, String> {
        if self.sync_positions.len() >= self.max_sync_positions {
            return Err("max sync positions reached".into());
        }
        let idx = self.next_sync_index;
        self.next_sync_index = self.next_sync_index.saturating_add(1);
        self.sync_positions.push(SyncPosition { position: pos, callback: cb, index: idx });
        Ok(idx)
    }
    pub fn del_sync(&mut self, index: i32) -> bool {
        let before = self.sync_positions.len();
        self.sync_positions.retain(|sp| sp.index != index);
        self.sync_positions.len() != before
    }
    pub fn fire_syncs(&mut self, prev_pos: NFrames, cur_pos: NFrames) {
        for sp in &mut self.sync_positions {
            if sp.position > prev_pos && sp.position <= cur_pos {
                (sp.callback)(sp.index, sp.position);
            } else if cur_pos < prev_pos && (sp.position > prev_pos || sp.position <= cur_pos) {
                // wrapped around
                (sp.callback)(sp.index, sp.position);
            }
        }
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
    fn zero_length_pulse_does_not_divide_by_zero() {
        let mut pulse = Pulse::new(0, 0);
        pulse.process_clock(128);
        assert_eq!(pulse.curpos, 0);
        assert!(!pulse.take_wrapped());
    }

    #[test]
    fn sync_indices_stay_unique_after_a_deletion() {
        let mut pulse = Pulse::new(480, 0);
        let first = pulse.add_sync(10, Box::new(|_, _| {})).unwrap();
        let second = pulse.add_sync(20, Box::new(|_, _| {})).unwrap();
        assert!(pulse.del_sync(first));
        // The next index must not reuse `second`'s index, otherwise deleting it
        // would remove both positions.
        let third = pulse.add_sync(30, Box::new(|_, _| {})).unwrap();
        assert_ne!(third, first);
        assert_ne!(third, second);
        assert!(pulse.del_sync(third));
        assert_eq!(pulse.sync_positions.len(), 1);
        assert!(pulse.del_sync(second));
        assert!(pulse.sync_positions.is_empty());
    }

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

#[cfg(test)]
mod sync_state_tests {
    use super::*;

    #[test]
    fn sync_state_repr_values() {
        assert_eq!(SyncState::None as i32, 0);
        assert_eq!(SyncState::Start as i32, 1);
        assert_eq!(SyncState::Beat as i32, 2);
        assert_eq!(SyncState::End as i32, 3);
        assert_eq!(SyncState::Ended as i32, 4);
    }

    #[test]
    fn sync_state_from_i32() {
        assert_eq!(SyncState::from(0), SyncState::None);
        assert_eq!(SyncState::from(3), SyncState::End);
        assert_eq!(SyncState::from(5), SyncState::None);
    }
}
