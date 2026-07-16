//! Pulse processor, ported from `fweelin_core_dsp_pulse.cc`.
//!
//! The application/event graph is deliberately expressed as traits so this
//! processor remains usable while the surrounding C++ adapters are migrated.

use crate::core_dsp_root::{AudioBuffers, Processor, Sample};

pub const MIDI_CLOCK_FREQUENCY: u32 = 24;
pub const SYNC_BEATS_PER_BAR: u32 = 4;
pub const MAX_SYNC_POS: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncState {
    None,
    Start,
    Beat,
    End,
    Ended,
}

pub trait PulseSyncCallback {
    fn pulse_sync(&mut self, sync_index: usize, actual_pos: u32);
}

pub trait PulseEvents {
    fn pulse_sync(&mut self);
    fn midi_clock(&mut self);
    fn midi_start_stop(&mut self, start: bool);
    fn hi_priority_trigger(&mut self) {}
}

pub trait PulseApp {
    fn fragment_size(&self) -> usize;
    fn sample_rate(&self) -> f64;
    fn transport_rolling(&self) -> bool {
        false
    }
    fn timebase_master(&self) -> bool {
        true
    }
    fn sync_type(&self) -> bool {
        false
    }
    fn sync_speed(&self) -> u32 {
        1
    }
    fn transport_bpm(&self) -> f64 {
        0.0
    }
    fn transport_beats_per_bar(&self) -> f64 {
        SYNC_BEATS_PER_BAR as f64
    }
    fn transport_beat(&self) -> i32 {
        0
    }
    fn transport_bar(&self) -> i32 {
        0
    }
    fn midi_sync_transmit(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy)]
struct SyncPoint {
    callback: Option<*mut dyn PulseSyncCallback>,
    pos: u32,
}

pub struct Pulse<A, E> {
    pub len: u32,
    pub curpos: u32,
    pub lc_len: i32,
    pub lc_cur: i32,
    pub wrapped: bool,
    pub stopped: bool,
    pub metro_active: bool,
    pub metro_volume: f32,
    prev_sync_bb: i32,
    sync_cnt: i32,
    prev_sync_speed: i32,
    prev_sync_type: bool,
    prev_bpm: f64,
    metro: Vec<Sample>,
    metro_hi: Vec<Sample>,
    metro_lo: Vec<Sample>,
    metro_ofs: usize,
    metro_hi_ofs: usize,
    metro_lo_ofs: usize,
    sync_points: Vec<SyncPoint>,
    clockrun: SyncState,
    midi_clock_count: u32,
    midi_beat_count: u32,
    app: A,
    events: E,
}

impl<A: PulseApp, E: PulseEvents> Pulse<A, E> {
    pub const METRONOME_HIT_LEN: usize = 800;
    pub const METRONOME_TONE_LEN: usize = 4400;
    pub const METRONOME_INIT_VOL: f32 = 0.1;

    pub fn new(app: A, events: E, len: u32, startpos: u32) -> Self {
        let sr = app.sample_rate();
        let mut seed: u32 = 0x9e3779b9;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) - 0.5
        };
        let metro = (0..Self::METRONOME_HIT_LEN)
            .map(|i| noise() * (1.0 - i as f32 / Self::METRONOME_HIT_LEN as f32))
            .collect();
        let hi = (0..Self::METRONOME_TONE_LEN)
            .map(|i| {
                1.5 * (880.0 * i as f64 * 2.0 * std::f64::consts::PI / sr).sin() as f32
                    * (1.0 - i as f32 / Self::METRONOME_TONE_LEN as f32)
            })
            .collect();
        let lo = (0..Self::METRONOME_TONE_LEN)
            .map(|i| {
                (440.0 * i as f64 * 2.0 * std::f64::consts::PI / sr).sin() as f32
                    * (1.0 - i as f32 / Self::METRONOME_TONE_LEN as f32)
            })
            .collect();
        Self {
            len,
            curpos: startpos,
            lc_len: 1,
            lc_cur: 0,
            wrapped: false,
            stopped: false,
            metro_active: false,
            metro_volume: 0.1,
            prev_sync_bb: 0,
            sync_cnt: 0,
            prev_sync_speed: -1,
            prev_sync_type: false,
            prev_bpm: 0.,
            metro,
            metro_hi: hi,
            metro_lo: lo,
            metro_ofs: 800,
            metro_hi_ofs: 4400,
            metro_lo_ofs: 4400,
            sync_points: Vec::new(),
            clockrun: SyncState::None,
            midi_clock_count: 0,
            midi_beat_count: 0,
            app,
            events,
        }
    }
    pub fn quantize_length(&self, src: u32) -> u32 {
        // Matches the C++ `Pulse::QuantizeLength` exactly, including its
        // division by `len` with no zero-length guard: a zero-length pulse
        // is not expected to occur in practice.
        let f = src as f32 / self.len as f32;
        (if f < 0.5 { 1. } else { f.round() }) as u32 * self.len
    }
    pub fn set_midi_clock(&mut self, start: bool) {
        if self.app.midi_sync_transmit() {
            self.clockrun = if start {
                SyncState::Start
            } else {
                self.events.midi_start_stop(false);
                SyncState::None
            };
        }
    }
    pub fn extend_long_count(&mut self, beats: i32, end_justify: bool) -> i32 {
        if beats > 0 {
            let n = crate::core_dsp::math_lcm(self.lc_len, beats);
            if end_justify && n > self.lc_len {
                self.lc_cur = n - (self.lc_len - self.lc_cur);
            }
            self.lc_len = n;
        }
        self.lc_len
    }
    pub fn add_sync(&mut self, callback: &mut dyn PulseSyncCallback, pos: u32) -> Option<usize> {
        let pos = pos.min(self.len.saturating_sub(1));
        let callback = unsafe {
            std::mem::transmute::<&mut dyn PulseSyncCallback, *mut dyn PulseSyncCallback>(callback)
        };
        if let Some(i) = self.sync_points.iter().position(|p| p.callback.is_none()) {
            self.sync_points[i] = SyncPoint {
                callback: Some(callback),
                pos,
            };
            Some(i)
        } else if self.sync_points.len() < MAX_SYNC_POS {
            self.sync_points.push(SyncPoint {
                callback: Some(callback),
                pos,
            });
            Some(self.sync_points.len() - 1)
        } else {
            None
        }
    }
    pub fn del_sync(&mut self, i: usize) {
        if i < self.sync_points.len() {
            self.sync_points[i].callback = None;
            while self
                .sync_points
                .last()
                .is_some_and(|p| p.callback.is_none())
            {
                self.sync_points.pop();
            }
        }
    }
    pub fn wrapped(&mut self) -> bool {
        let w = self.wrapped;
        self.wrapped = false;
        w
    }
    pub fn process_with_events(
        &mut self,
        pre: bool,
        requested: u32,
        buffers: &mut AudioBuffers<'_>,
    ) {
        self.process(pre, requested as usize, buffers);
    }
}

impl<A: PulseApp, E: PulseEvents> Processor for Pulse<A, E> {
    fn process(&mut self, pre: bool, requested: usize, b: &mut AudioBuffers<'_>) {
        let mut l = requested.min(self.app.fragment_size());
        if self.len == 0 {
            b.outputs[0][..l].fill(0.);
            return;
        }
        if self.app.transport_rolling() && !self.app.timebase_master() {
            // Matches C++ `Pulse::process`: `sync_speed` is used unclamped,
            // so a transient `GetSyncSpeed() == 0` wraps on every beat/bar
            // change and `clocksperpulse` degenerates, exactly as upstream.
            let speed = self.app.sync_speed() as i32;
            let kind = self.app.sync_type();
            if kind != self.prev_sync_type || speed != self.prev_sync_speed {
                self.prev_bpm = 0.;
                self.prev_sync_bb = -1;
                self.prev_sync_type = kind;
                self.prev_sync_speed = speed;
            }
            let bpm = self.app.transport_bpm();
            if bpm != self.prev_bpm {
                let multiplier = if kind {
                    speed as f64
                } else {
                    self.app.transport_beats_per_bar() * speed as f64
                };
                self.len = (60. * self.app.sample_rate() * multiplier / bpm) as u32;
                self.prev_bpm = bpm;
            }
            let beat = if kind {
                self.app.transport_beat()
            } else {
                self.app.transport_bar()
            };
            if beat != self.prev_sync_bb {
                self.sync_cnt += 1;
                if self.sync_cnt >= speed {
                    self.sync_cnt = 0;
                    self.curpos = self.len;
                }
                self.prev_sync_bb = beat;
            }
        }
        let old = self.curpos;
        self.wrapped = false;
        if !pre && !self.stopped {
            // C++ advances only to the pulse boundary before emitting MIDI
            // clocks and wrap notifications; the remainder belongs to this
            // fragment's post-wrap output, not the next pulse position.
            let remaining = self.len.saturating_sub(self.curpos);
            self.curpos = self.curpos.saturating_add((l as u32).min(remaining));
            if self.clockrun != SyncState::None && self.app.midi_sync_transmit() {
                let speed = self.app.sync_speed();
                let clocks_per_pulse = MIDI_CLOCK_FREQUENCY
                    * speed
                    * if self.app.sync_type() {
                        1
                    } else {
                        SYNC_BEATS_PER_BAR
                    };
                let frames_per_clock = self.len as f32 / clocks_per_pulse as f32;
                let old_clock = (old as f32 / frames_per_clock) as u32;
                let new_clock = (self.curpos as f32 / frames_per_clock) as u32;
                let crossed_clock = self.clockrun == SyncState::Beat && new_clock != old_clock;
                if (crossed_clock || self.curpos >= self.len) && self.clockrun == SyncState::Start {
                    self.metro_hi_ofs = 0;
                    self.midi_clock_count = 0;
                    self.midi_beat_count = 0;
                    self.events.midi_start_stop(true);
                    self.clockrun = SyncState::Beat;
                } else if crossed_clock || self.curpos >= self.len {
                    self.midi_clock_count += 1;
                    if self.midi_clock_count >= MIDI_CLOCK_FREQUENCY {
                        self.midi_clock_count = 0;
                        self.midi_beat_count += 1;
                        if self.midi_beat_count >= clocks_per_pulse / MIDI_CLOCK_FREQUENCY {
                            self.midi_beat_count = 0;
                            self.metro_hi_ofs = 0;
                        } else {
                            self.metro_lo_ofs = 0;
                        }
                    }
                    self.events.midi_clock();
                }
            }
            if self.curpos >= self.len {
                self.curpos = 0;
                self.wrapped = true;
                self.lc_cur = (self.lc_cur + 1) % self.lc_len;
                self.events.pulse_sync();
                self.events.hi_priority_trigger();
                self.metro_ofs = 0;
            }
        }
        let mut ofs = 0;
        if self.wrapped {
            let rem = (self.len - old) as usize;
            b.outputs[0][..rem.min(l)].fill(0.);
            ofs = rem.min(l);
            l -= ofs;
        }
        for n in 0..l {
            let i = ofs + n;
            b.outputs[0][i] = if self.metro_active && self.metro_ofs + n < self.metro.len() {
                self.metro[self.metro_ofs + n] * self.metro_volume
            } else {
                0.
            };
            if self.metro_active && self.metro_hi_ofs + n < self.metro_hi.len() {
                b.outputs[0][i] += self.metro_hi[self.metro_hi_ofs + n] * self.metro_volume;
            }
            if self.metro_active && self.metro_lo_ofs + n < self.metro_lo.len() {
                b.outputs[0][i] += self.metro_lo[self.metro_lo_ofs + n] * self.metro_volume;
            }
        }
        if !pre {
            self.metro_ofs += l;
            self.metro_hi_ofs += l;
            self.metro_lo_ofs += l;
        }
        // `Pulse::process` invokes position callbacks only on its realtime
        // (non-preprocess) pass. Calling them from `pre` lets a processor
        // advance twice for one fragment.
        if !pre && !self.stopped {
            for (i, p) in self.sync_points.iter_mut().enumerate() {
                if let Some(cb) = p.callback
                    && ((old < p.pos && self.curpos >= p.pos)
                        || (self.wrapped && p.pos <= self.curpos))
                {
                    unsafe {
                        (*cb).pulse_sync(i, self.curpos);
                    }
                }
            }
        }
        // `AudioBuffers` always carries two Rust slices, whereas C++ uses a
        // null right-channel pointer for mono.  Only duplicate when the
        // caller supplied a real right output of this fragment's length.
        if b.outputs[1].len() >= requested.min(self.app.fragment_size()) {
            let (left, right) = b.outputs.split_at_mut(1);
            right[0][..requested.min(self.app.fragment_size())]
                .copy_from_slice(&left[0][..requested.min(self.app.fragment_size())]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct App;
    impl PulseApp for App {
        fn fragment_size(&self) -> usize {
            48
        }
        fn sample_rate(&self) -> f64 {
            48_000.0
        }
        fn midi_sync_transmit(&self) -> bool {
            true
        }
        fn sync_type(&self) -> bool {
            true
        }
    }

    #[derive(Default)]
    struct Counts {
        starts: usize,
        clocks: usize,
        stops: usize,
    }
    struct Events(Arc<Mutex<Counts>>);
    impl PulseEvents for Events {
        fn pulse_sync(&mut self) {}
        fn midi_clock(&mut self) {
            self.0.lock().unwrap().clocks += 1;
        }
        fn midi_start_stop(&mut self, start: bool) {
            let mut counts = self.0.lock().unwrap();
            if start {
                counts.starts += 1
            } else {
                counts.stops += 1
            }
        }
    }

    struct Callback(Arc<Mutex<usize>>);
    impl PulseSyncCallback for Callback {
        fn pulse_sync(&mut self, _: usize, _: u32) {
            *self.0.lock().unwrap() += 1;
        }
    }

    #[test]
    fn midi_start_and_clock_follow_original_pulse_boundaries() {
        let counts = Arc::new(Mutex::new(Counts::default()));
        let mut pulse = Pulse::new(App, Events(Arc::clone(&counts)), 48, 0);
        pulse.set_midi_clock(true);
        let mut left = [0.0; 48];
        let mut right = [0.0; 48];
        let mut empty_left = [];
        let mut empty_right = [];
        let mut buffers = AudioBuffers {
            inputs: [&mut empty_left, &mut empty_right],
            outputs: [&mut left, &mut right],
            num_inputs: 0,
            num_outputs: 1,
        };

        pulse.process(false, 48, &mut buffers);
        assert_eq!(counts.lock().unwrap().starts, 1);
        pulse.process(false, 2, &mut buffers);
        assert_eq!(counts.lock().unwrap().clocks, 1);
        pulse.set_midi_clock(false);
        assert_eq!(counts.lock().unwrap().stops, 1);
    }

    #[test]
    fn preprocess_does_not_dispatch_cpp_pulse_sync_callbacks() {
        let counts = Arc::new(Mutex::new(Counts::default()));
        let mut pulse = Pulse::new(App, Events(counts), 48, 0);
        let called = Arc::new(Mutex::new(0));
        let mut callback = Callback(Arc::clone(&called));
        pulse.add_sync(&mut callback, 4).unwrap();
        let mut left = [0.0; 48];
        let mut right = [0.0; 48];
        let mut empty_left = [];
        let mut empty_right = [];
        let mut buffers = AudioBuffers {
            inputs: [&mut empty_left, &mut empty_right],
            outputs: [&mut left, &mut right],
            num_inputs: 0,
            num_outputs: 1,
        };

        pulse.process(true, 4, &mut buffers);
        assert_eq!(*called.lock().unwrap(), 0);
        pulse.process(false, 4, &mut buffers);
        assert_eq!(*called.lock().unwrap(), 1);
    }

    #[test]
    fn mono_output_is_the_cpp_null_right_channel_case() {
        let counts = Arc::new(Mutex::new(Counts::default()));
        let mut pulse = Pulse::new(App, Events(counts), 48, 47);
        pulse.metro_active = true;
        let mut left = [0.0; 4];
        let empty_left = [];
        let empty_right = [];
        let mut no_right = [];
        let mut buffers = AudioBuffers {
            inputs: [&empty_left, &empty_right],
            outputs: [&mut left, &mut no_right],
            num_inputs: 0,
            num_outputs: 1,
        };

        pulse.process(false, 4, &mut buffers);
        assert!(buffers.outputs[0][1..].iter().any(|sample| *sample != 0.0));
    }

    #[test]
    fn wrap_zeroes_the_pre_boundary_fragment_before_the_metronome_hit() {
        let counts = Arc::new(Mutex::new(Counts::default()));
        // The C++ process path advances only to `remaining`, zeros that
        // prefix, wraps, then renders the metronome in the rest of the same
        // audio fragment. It does not advance the new pulse position through
        // that rest a second time.
        let mut pulse = Pulse::new(App, Events(counts), 4, 3);
        pulse.metro_active = true;
        let mut left = [1.0; 4];
        let mut right = [1.0; 4];
        let mut empty_left = [];
        let mut empty_right = [];
        let mut buffers = AudioBuffers {
            inputs: [&mut empty_left, &mut empty_right],
            outputs: [&mut left, &mut right],
            num_inputs: 0,
            num_outputs: 1,
        };

        pulse.process(false, 4, &mut buffers);
        assert_eq!(pulse.curpos, 0);
        assert!(pulse.wrapped());
        assert_eq!(buffers.outputs[0][0], 0.0);
        assert_eq!(buffers.outputs[1][0], 0.0);
        assert!(buffers.outputs[0][1..].iter().any(|sample| *sample != 0.0));
        assert_eq!(buffers.outputs[0], buffers.outputs[1]);
    }
}
