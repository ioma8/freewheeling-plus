//! Loop, record and file-stream processors.
//!
//! The old implementation reached into `Loop`, `AudioBlock` and the encoder
//! directly.  Those are intentionally traits here: the realtime state machine
//! is in this module, while allocation, persistence and codecs remain outside
//! the audio callback.

use crate::core_dsp::{NFrames, Processor, SS_BEAT, SS_ENDED, SS_NONE, SS_START, Sample};
use crate::core_dsp_audio_buffers::AudioBuffers;

pub trait LoopSource {
    fn frames(&self) -> usize;
    fn stereo(&self) -> bool {
        false
    }
    fn volume(&self) -> f32 {
        1.0
    }
    fn read(&self, pos: usize, left: &mut [Sample], right: Option<&mut [Sample]>) -> usize;
    fn write(&mut self, pos: usize, left: &[Sample], right: Option<&[Sample]>) -> usize;
    fn grow(&mut self, _frames: usize) -> bool {
        false
    }
    fn finish(&mut self) {}
}

pub trait PulseSync {
    fn add(&mut self, callback: &mut dyn SyncCallback, position: usize) -> Option<usize>;
    fn remove(&mut self, index: usize);
    fn position(&self) -> usize;
    fn length(&self) -> usize;
}
pub trait SyncCallback {
    fn pulse_sync(&mut self, position: usize);
}

fn outputs(b: &mut AudioBuffers, len: usize) -> (&mut [Sample], Option<&mut [Sample]>) {
    let [left_channels, right_channels] = &mut b.outputs;
    let left = left_channels
        .first_mut()
        .and_then(Option::as_deref_mut)
        .expect("processor requires left output");
    let right = right_channels.first_mut().and_then(Option::as_deref_mut);
    (&mut left[..len], right.map(|r| &mut r[..len]))
}

pub struct PlayProcessor<L> {
    pub loop_source: L,
    pub position: usize,
    pub play_volume: f32,
    pub stopped: bool,
    pub sync_state: i32,
    pub curbeat: usize,
}
impl<L: LoopSource> PlayProcessor<L> {
    pub fn new(loop_source: L, play_volume: f32, start: usize) -> Self {
        Self {
            loop_source,
            position: start,
            play_volume,
            stopped: false,
            sync_state: SS_NONE,
            curbeat: 0,
        }
    }
    pub fn sync_up(&mut self) {
        self.sync_state = SS_START;
    }
    pub fn pulse_sync(&mut self) {
        if self.sync_state == SS_START {
            self.position = 0;
            self.stopped = false;
            self.sync_state = SS_BEAT;
        }
    }
    pub fn played_length(&self) -> usize {
        self.position
    }
}
impl<L: LoopSource> Processor for PlayProcessor<L> {
    fn process(&mut self, _pre: bool, len: NFrames, b: &mut AudioBuffers) {
        let n = len as usize;
        let (left, mut right) = outputs(b, n);
        left.fill(0.0);
        if let Some(r) = right.as_deref_mut() {
            r.fill(0.0);
        }
        if self.stopped || self.loop_source.frames() == 0 {
            return;
        }
        let mut got = 0;
        while got < n {
            let remain = self.loop_source.frames() - self.position.min(self.loop_source.frames());
            let take = (n - got).min(remain.max(1));
            let l = &mut left[got..got + take];
            let r = right.as_deref_mut().map(|x| &mut x[got..got + take]);
            let read = self.loop_source.read(self.position, l, r);
            if read == 0 {
                break;
            }
            for x in &mut l[..read] {
                *x *= self.play_volume * self.loop_source.volume();
            }
            if let Some(r) = right.as_deref_mut() {
                for x in &mut r[got..got + read] {
                    *x *= self.play_volume * self.loop_source.volume();
                }
            }
            got += read;
            self.position = (self.position + read) % self.loop_source.frames();
        }
    }
    fn halt(&mut self) {
        self.stopped = true;
        self.sync_state = SS_ENDED;
    }
}

pub struct RecordProcessor<L> {
    pub loop_source: L,
    pub position: usize,
    pub stopped: bool,
    pub sync_state: i32,
    pub input_volume: f32,
    pub overdub_feedback: f32,
}
impl<L: LoopSource> RecordProcessor<L> {
    pub fn new(loop_source: L, input_volume: f32) -> Self {
        Self {
            loop_source,
            position: 0,
            stopped: false,
            sync_state: SS_NONE,
            input_volume,
            overdub_feedback: 0.5,
        }
    }
    pub fn end_now(&mut self) {
        self.stopped = true;
        self.loop_source.finish();
    }
    pub fn recorded_length(&self) -> usize {
        self.position
    }
}
impl<L: LoopSource> Processor for RecordProcessor<L> {
    fn process(&mut self, _pre: bool, len: NFrames, b: &mut AudioBuffers) {
        if self.stopped {
            return;
        }
        let n = len as usize;
        let input = b.inputs[0]
            .first()
            .and_then(Option::as_deref)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let right_in = b.inputs[1]
            .first()
            .and_then(Option::as_deref)
            .map(Vec::as_slice);
        let take = n.min(input.len());
        if take == 0 {
            return;
        }
        // Keep callback work bounded and allocation-free. A stack block also
        // lets us apply the input gain consistently to both channels.
        const BLOCK: usize = 256;
        let mut left = [0.0; BLOCK];
        let mut right = [0.0; BLOCK];
        let mut written = 0;
        while written < take {
            let count = (take - written).min(BLOCK);
            for i in 0..count {
                left[i] = input[written + i] * self.input_volume;
                right[i] = right_in
                    .and_then(|samples| samples.get(written + i))
                    .copied()
                    .unwrap_or(input[written + i])
                    * self.input_volume;
            }
            let stereo = right_in.map(|_| &right[..count]);
            if self
                .loop_source
                .write(self.position + written, &left[..count], stereo)
                == 0
                && !self.loop_source.grow(count)
            {
                self.end_now();
                return;
            }
            written += count;
        }
        self.position += take;
    }
    fn halt(&mut self) {
        self.end_now();
        self.sync_state = SS_ENDED;
    }
}

pub trait StreamWriter {
    fn start(&mut self, name: &str) -> bool;
    fn write(&mut self, left: &[Sample], right: Option<&[Sample]>);
    fn stop(&mut self);
}
pub struct FileStreamer<W> {
    pub writer: W,
    pub input_index: usize,
    pub stereo: bool,
    pub status: u8,
    pub output_size: usize,
}
impl<W: StreamWriter> FileStreamer<W> {
    pub const STOPPED: u8 = 0;
    pub const RUNNING: u8 = 1;
    pub const STOP_PENDING: u8 = 2;
    pub fn new(writer: W, input_index: usize, stereo: bool) -> Self {
        Self {
            writer,
            input_index,
            stereo,
            status: 0,
            output_size: 0,
        }
    }
    pub fn start_writing(&mut self, name: &str) -> bool {
        if self.writer.start(name) {
            self.status = Self::RUNNING;
            true
        } else {
            false
        }
    }
    pub fn stop_writing(&mut self) {
        if self.status == Self::RUNNING {
            self.status = Self::STOP_PENDING;
        }
    }
}
impl<W: StreamWriter> Processor for FileStreamer<W> {
    fn process(&mut self, _: bool, len: NFrames, b: &mut AudioBuffers) {
        if self.status == Self::STOP_PENDING {
            self.writer.stop();
            self.status = Self::STOPPED;
            return;
        }
        if self.status != Self::RUNNING {
            return;
        }
        let n = len as usize;
        let l = b.inputs[0]
            .get(self.input_index)
            .and_then(Option::as_deref)
            .map(|x| &x[..n.min(x.len())])
            .unwrap_or(&[]);
        let r = if self.stereo {
            b.inputs[1]
                .get(self.input_index)
                .and_then(Option::as_deref)
                .map(|x| &x[..n.min(x.len())])
        } else {
            None
        };
        self.writer.write(l, r);
        self.output_size += l.len();
    }
    fn halt(&mut self) {
        self.stop_writing();
    }
}
