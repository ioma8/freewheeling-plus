//! Audio block chains, iterators, extended data, and block processors.
//!
//! This is the ownership-safe Rust counterpart of `fweelin_block.cc`.  A chain
//! owns its samples and metadata; iterators borrow a chain and managers own
//! only their work state.  The wire format is deliberately small and stable so
//! blocks can be persisted without exposing internal pointers.

use crate::core_dsp::{NFrames, Processor};
use crate::core_dsp_audio_buffers::AudioBuffers;
use crate::mem::Preallocated;
use std::io::{self, Read, Write};

pub type Sample = f32;
pub const AUDIOBLOCK_DEFAULT_LEN: usize = 20_000;
pub const AUDIOBLOCK_SMOOTH_ENDPOINTS_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Unknown = -1,
    Vorbis = 0,
    Wav = 1,
    Flac = 2,
    Au = 3,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockExtendedDataType {
    None,
    ExtraChannel,
    PeaksAvgs,
    MarkerPoints,
}

pub trait BlockExtendedData: Send {
    fn kind(&self) -> BlockExtendedDataType;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtraChannel {
    pub samples: Vec<Sample>,
}
impl ExtraChannel {
    pub fn new(len: usize) -> Self {
        Self {
            samples: vec![0.0; len],
        }
    }
}
impl BlockExtendedData for ExtraChannel {
    fn kind(&self) -> BlockExtendedDataType {
        BlockExtendedDataType::ExtraChannel
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PeaksAvgs {
    pub peaks: AudioBlock,
    pub avgs: AudioBlock,
    pub chunk_size: usize,
}
impl BlockExtendedData for PeaksAvgs {
    fn kind(&self) -> BlockExtendedDataType {
        BlockExtendedDataType::PeaksAvgs
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeMarker {
    pub offset: usize,
    pub data: i64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkerPoints {
    pub markers: Vec<TimeMarker>,
}
impl MarkerPoints {
    pub fn count(&self) -> usize {
        self.markers.len()
    }
    pub fn nth_before(&self, n: usize, offset: usize) -> Option<TimeMarker> {
        self.markers
            .iter()
            .rev()
            .filter(|m| m.offset <= offset)
            .nth(n)
            .copied()
            .or_else(|| self.markers.iter().rev().nth(n).copied())
    }
}
impl BlockExtendedData for MarkerPoints {
    fn kind(&self) -> BlockExtendedDataType {
        BlockExtendedDataType::MarkerPoints
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioBlock {
    pub samples: Vec<Sample>,
    pub extra: Option<ExtraChannel>,
    pub next: Option<Box<AudioBlock>>,
}
impl AudioBlock {
    pub fn new(len: usize) -> Self {
        Self {
            samples: vec![0.0; len],
            extra: None,
            next: None,
        }
    }
    pub fn total_len(&self) -> usize {
        self.samples.len() + self.next.as_deref().map_or(0, Self::total_len)
    }
    pub fn is_stereo(&self) -> bool {
        self.extra.is_some()
    }
    pub fn zero(&mut self) {
        self.samples.fill(0.0);
        if let Some(n) = &mut self.next {
            n.zero();
        }
        if let Some(e) = &mut self.extra {
            e.samples.fill(0.0);
        }
    }
    pub fn chop_chain(&mut self) {
        self.next = None;
    }
    pub fn link(&mut self, block: AudioBlock) {
        self.next = Some(Box::new(block));
    }
    pub fn sample(&self, mut offset: usize) -> Option<Sample> {
        if offset < self.samples.len() {
            Some(self.samples[offset])
        } else {
            offset -= self.samples.len();
            self.next.as_deref()?.sample(offset)
        }
    }
    pub fn generate_subchain(&self, from: usize, to: usize, stereo: bool) -> AudioBlock {
        let total = self.total_len();
        let end = if to >= from { to.min(total) } else { total };
        let mut out = AudioBlock::new(end.saturating_sub(from));
        for i in 0..out.samples.len() {
            out.samples[i] = self.sample((from + i) % total).unwrap_or(0.0);
        }
        if stereo {
            out.extra = Some(ExtraChannel::new(out.samples.len()));
            for i in 0..out.samples.len() {
                out.extra.as_mut().unwrap().samples[i] =
                    self.extra_sample((from + i) % total).unwrap_or(0.0);
            }
        }
        out
    }
    fn extra_sample(&self, mut offset: usize) -> Option<Sample> {
        if offset < self.samples.len() {
            self.extra.as_ref()?.samples.get(offset).copied()
        } else {
            offset -= self.samples.len();
            self.next.as_deref()?.extra_sample(offset)
        }
    }
    pub fn serialize<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(b"FWB1")?;
        write_u64(w, self.total_len() as u64)?;
        for i in 0..self.total_len() {
            w.write_all(&self.sample(i).unwrap().to_le_bytes())?;
        }
        Ok(())
    }
    pub fn deserialize<R: Read>(r: &mut R) -> io::Result<Self> {
        let mut magic = [0; 4];
        r.read_exact(&mut magic)?;
        if &magic != b"FWB1" {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid block"));
        }
        let n = read_u64(r)? as usize;
        let mut b = Self::new(n);
        for s in &mut b.samples {
            let mut x = [0; 4];
            r.read_exact(&mut x)?;
            *s = f32::from_le_bytes(x);
        }
        Ok(b)
    }
}
impl Default for AudioBlock {
    fn default() -> Self {
        Self::new(AUDIOBLOCK_DEFAULT_LEN)
    }
}
impl Preallocated for AudioBlock {
    fn recycle(&mut self) {
        self.zero();
        self.next = None;
    }
}

fn write_u64<W: Write>(w: &mut W, n: u64) -> io::Result<()> {
    w.write_all(&n.to_le_bytes())
}
fn read_u64<R: Read>(r: &mut R) -> io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

pub struct AudioBlockIterator<'a> {
    pub block: &'a mut AudioBlock,
    pub position: usize,
    pub fragment_size: usize,
    stopped: bool,
}
impl<'a> AudioBlockIterator<'a> {
    pub fn new(block: &'a mut AudioBlock, fragment_size: usize) -> Self {
        Self {
            block,
            position: 0,
            fragment_size,
            stopped: false,
        }
    }
    pub fn jump(&mut self, offset: usize) {
        self.position = offset.min(self.block.total_len());
    }
    pub fn get_fragment(&self) -> &[Sample] {
        let mut block: &AudioBlock = &*self.block;
        let mut offset = self.position;
        loop {
            if offset < block.samples.len() {
                let end = (offset + self.fragment_size).min(block.samples.len());
                return &block.samples[offset..end];
            }
            offset = offset.saturating_sub(block.samples.len());
            if let Some(ref next) = block.next {
                block = next;
            } else {
                return &[];
            }
        }
    }
    pub fn put_fragment(&mut self, data: &[Sample]) -> usize {
        let n = data
            .len()
            .min(self.block.total_len().saturating_sub(self.position));
        for (i, v) in data[..n].iter().enumerate() {
            self.set(self.position + i, *v);
        }
        self.position += n;
        n
    }
    pub fn put_fragment_stereo(&mut self, left: &[Sample], right: &[Sample]) -> usize {
        let n = left
            .len()
            .min(right.len())
            .min(self.block.total_len().saturating_sub(self.position));
        for i in 0..n {
            self.set(self.position + i, left[i]);
            self.set_extra(self.position + i, right[i]);
        }
        self.position += n;
        n
    }
    fn set(&mut self, mut p: usize, v: Sample) {
        if p < self.block.samples.len() {
            self.block.samples[p] = v;
        } else {
            p -= self.block.samples.len();
            if let Some(n) = &mut self.block.next {
                let mut i = AudioBlockIterator::new(n, self.fragment_size);
                i.set(p, v);
            }
        }
    }
    fn set_extra(&mut self, mut p: usize, v: Sample) {
        if p < self.block.samples.len() {
            let extra = self
                .block
                .extra
                .get_or_insert_with(|| ExtraChannel::new(self.block.samples.len()));
            extra.samples[p] = v;
        } else {
            p -= self.block.samples.len();
            if let Some(n) = &mut self.block.next {
                let mut i = AudioBlockIterator::new(n, self.fragment_size);
                i.set_extra(p, v);
            }
        }
    }
    pub fn next_fragment(&mut self) {
        self.position = (self.position + self.fragment_size).min(self.block.total_len());
    }
    pub fn stop(&mut self) {
        self.stopped = true;
    }
    pub fn stopped(&self) -> bool {
        self.stopped
    }
}

pub struct PeaksAvgsProcessor {
    pub chunk_size: usize,
    pub cursor: usize,
}
impl PeaksAvgsProcessor {
    pub fn process_block(&mut self, block: &AudioBlock, out: &mut PeaksAvgs) {
        let mut pos = 0;
        while pos < block.total_len() {
            let end = (pos + self.chunk_size).min(block.total_len());
            let vals = (pos..end).map(|i| block.sample(i).unwrap());
            let (mut lo, mut hi, mut sum) = (f32::INFINITY, f32::NEG_INFINITY, 0.0);
            for v in vals {
                lo = lo.min(v);
                hi = hi.max(v);
                sum += v;
            }
            out.peaks.samples.push(hi);
            out.peaks.samples.push(lo);
            out.avgs.samples.push(sum / (end - pos) as f32);
            pos = end;
        }
        self.cursor = pos;
    }
}
impl Processor for PeaksAvgsProcessor {
    fn process(&mut self, _pre: bool, _len: NFrames, _buffers: &mut AudioBuffers) {}
}
