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
    /// The `n`-th marker at or before `offset`, counting backwards.
    ///
    /// Returns `None` when fewer than `n + 1` markers lie at or before the
    /// position. A marker *after* the position is not a valid answer, so the
    /// search never wraps around the end of the loop.
    pub fn nth_before(&self, n: usize, offset: usize) -> Option<TimeMarker> {
        self.markers
            .iter()
            .rev()
            .filter(|marker| marker.offset <= offset)
            .nth(n)
            .copied()
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
    /// Copy `from..to` (clamped to the chain, `to < from` meaning "to the
    /// end") into a new block.
    ///
    /// Returns `None` when `from` is past the end of the chain instead of
    /// silently producing an empty block.
    pub fn generate_subchain(&self, from: usize, to: usize, stereo: bool) -> Option<AudioBlock> {
        let total = self.total_len();
        if from > total {
            return None;
        }
        let end = if to >= from { to.min(total) } else { total };
        let mut out = AudioBlock::new(end - from);
        for i in 0..out.samples.len() {
            out.samples[i] = self.sample(from + i).unwrap_or(0.0);
        }
        if stereo {
            let mut extra = ExtraChannel::new(out.samples.len());
            for i in 0..out.samples.len() {
                extra.samples[i] = self.extra_sample(from + i).unwrap_or(0.0);
            }
            out.extra = Some(extra);
        }
        Some(out)
    }
    fn extra_sample(&self, mut offset: usize) -> Option<Sample> {
        if offset < self.samples.len() {
            self.extra.as_ref()?.samples.get(offset).copied()
        } else {
            offset -= self.samples.len();
            self.next.as_deref()?.extra_sample(offset)
        }
    }
    /// Serialize the whole chain, head first.
    ///
    /// The `FWB2` format records every chain link and the stereo extra
    /// channel, so [`AudioBlock::deserialize`] round-trips a block unchanged.
    /// The older `FWB1` layout (a single flattened mono block) is still
    /// accepted by the reader.
    pub fn serialize<W: Write>(&self, w: &mut W) -> io::Result<()> {
        let blocks = self.chain_len();
        w.write_all(b"FWB2")?;
        w.write_all(&(blocks as u32).to_le_bytes())?;
        let mut node = Some(self);
        while let Some(block) = node {
            let len = block.samples.len();
            let mut flags = 0u8;
            if let Some(extra) = &block.extra {
                if extra.samples.len() != len {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "stereo channel length does not match the block",
                    ));
                }
                flags |= STEREO_FLAG;
            }
            if len == 0 || len > AUDIOBLOCK_MAX_SERIALIZED_SAMPLES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "block length is not serializable",
                ));
            }
            w.write_all(&(len as u32).to_le_bytes())?;
            w.write_all(&[flags])?;
            write_samples(w, &block.samples)?;
            if flags & STEREO_FLAG != 0 {
                let extra = &block.extra.as_ref().expect("flag implies extra").samples;
                write_samples(w, extra)?;
            }
            node = block.next.as_deref();
        }
        Ok(())
    }

    /// Parse the chain written by [`AudioBlock::serialize`].
    ///
    /// Lengths are validated against [`AUDIOBLOCK_MAX_SERIALIZED_SAMPLES`] and
    /// the link count against [`AUDIOBLOCK_MAX_SERIALIZED_BLOCKS`] before any
    /// allocation, so a corrupt header cannot demand unbounded memory ahead of
    /// the first failed read.
    pub fn deserialize<R: Read>(r: &mut R) -> io::Result<Self> {
        let mut magic = [0; 4];
        r.read_exact(&mut magic)?;
        match &magic {
            b"FWB1" => Self::deserialize_legacy(r),
            b"FWB2" => Self::deserialize_chain(r),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "invalid block")),
        }
    }

    fn deserialize_legacy<R: Read>(r: &mut R) -> io::Result<Self> {
        let mut n_buf = [0; 8];
        r.read_exact(&mut n_buf)?;
        let n = check_block_len(u64::from_le_bytes(n_buf))?;
        let mut block = Self::new(n);
        read_samples(r, &mut block.samples)?;
        Ok(block)
    }

    fn deserialize_chain<R: Read>(r: &mut R) -> io::Result<Self> {
        let mut count_buf = [0; 4];
        r.read_exact(&mut count_buf)?;
        let count = u32::from_le_bytes(count_buf) as usize;
        if count == 0 || count > AUDIOBLOCK_MAX_SERIALIZED_BLOCKS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "serialized block chain length is out of range",
            ));
        }
        let mut remaining = AUDIOBLOCK_MAX_SERIALIZED_SAMPLES;
        let mut blocks = Vec::new();
        for _ in 0..count {
            let mut len_buf = [0; 4];
            r.read_exact(&mut len_buf)?;
            let len = check_block_len(u64::from(u32::from_le_bytes(len_buf)))?;
            if len == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "serialized block is empty",
                ));
            }
            remaining = remaining.checked_sub(len).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "serialized chain is too long")
            })?;
            let mut flags = [0; 1];
            r.read_exact(&mut flags)?;
            let mut block = Self::new(len);
            read_samples(r, &mut block.samples)?;
            if flags[0] & STEREO_FLAG != 0 {
                let mut extra = ExtraChannel::new(len);
                read_samples(r, &mut extra.samples)?;
                block.extra = Some(extra);
            }
            blocks.push(block);
        }
        // Link the parsed blocks back to front so the chain keeps its
        // structure instead of being flattened into one block.
        let mut chain: Option<Box<AudioBlock>> = None;
        for mut block in blocks.into_iter().rev() {
            block.next = chain;
            chain = Some(Box::new(block));
        }
        chain
            .map(|block| *block)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "serialized chain is empty"))
    }

    /// Iterate every sample in the chain, head first.
    ///
    /// Each link is visited once, so this stays O(samples) on fragmented
    /// chains instead of re-walking the chain per sample.
    pub fn samples_iter(&self) -> impl Iterator<Item = Sample> + '_ {
        ChainCursor::new(self)
    }

    fn chain_len(&self) -> usize {
        let mut count = 0;
        let mut node = Some(self);
        while let Some(block) = node {
            count += 1;
            node = block.next.as_deref();
        }
        count
    }
}

/// `flags` bit marking a serialized block that carries a stereo extra channel.
const STEREO_FLAG: u8 = 0x01;

/// Sample budget for a serialized chain. Far above any real loop (about 23
/// minutes of 48 kHz audio) while still bounding what a corrupt length header
/// can allocate.
pub const AUDIOBLOCK_MAX_SERIALIZED_SAMPLES: usize = 1 << 26;

/// Chain-link budget for a serialized chain.
pub const AUDIOBLOCK_MAX_SERIALIZED_BLOCKS: usize = 1 << 20;

fn check_block_len(len: u64) -> io::Result<usize> {
    let len = usize::try_from(len)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "block length does not fit"))?;
    if len > AUDIOBLOCK_MAX_SERIALIZED_SAMPLES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "block length exceeds the serialization limit",
        ));
    }
    Ok(len)
}

fn read_samples<R: Read>(r: &mut R, samples: &mut [Sample]) -> io::Result<()> {
    let mut bytes = vec![0u8; std::mem::size_of_val(samples)];
    r.read_exact(&mut bytes)?;
    for (sample, chunk) in samples.iter_mut().zip(bytes.chunks_exact(4)) {
        *sample = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    Ok(())
}

fn write_samples<W: Write>(w: &mut W, samples: &[Sample]) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    w.write_all(&bytes)
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

pub struct AudioBlockIterator<'a> {
    pub block: &'a mut AudioBlock,
    pub position: usize,
    pub fragment_size: usize,
}
impl<'a> AudioBlockIterator<'a> {
    pub fn new(block: &'a mut AudioBlock, fragment_size: usize) -> Self {
        Self {
            block,
            position: 0,
            fragment_size,
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
}

pub struct PeaksAvgsProcessor {
    pub chunk_size: usize,
    pub cursor: usize,
}
impl PeaksAvgsProcessor {
    /// Recompute `out` over `block`.
    ///
    /// The output is cleared first: repeated calls describe the current block
    /// instead of appending to the previous result. A `chunk_size` of 0 is
    /// treated as 1 so the chunk loop always advances.
    ///
    /// Layout per chunk, matching the C++ `PeaksAvgsManager`: two `peaks`
    /// samples (maximum, then minimum) and one `avgs` sample (mean absolute
    /// amplitude).
    pub fn process_block(&mut self, block: &AudioBlock, out: &mut PeaksAvgs) {
        let chunk_size = self.chunk_size.max(1);
        let total = block.total_len();
        let chunks = total.div_ceil(chunk_size);
        out.peaks.samples.clear();
        out.peaks.samples.reserve(chunks * 2);
        out.avgs.samples.clear();
        out.avgs.samples.reserve(chunks);
        out.chunk_size = chunk_size;
        let mut samples = ChainCursor::new(block);
        let mut pos = 0;
        while pos < total {
            let end = (pos + chunk_size).min(total);
            let (mut lo, mut hi, mut sum) = (f32::INFINITY, f32::NEG_INFINITY, 0.0);
            for _ in pos..end {
                let value = samples.next().unwrap_or(0.0);
                lo = lo.min(value);
                hi = hi.max(value);
                // C++ `PeaksAvgsManager` averages the absolute amplitude.
                sum += value.abs();
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

/// Flat cursor over a block chain: each link is visited once, so walking the
/// chain costs O(samples) instead of re-walking it per sample.
struct ChainCursor<'a> {
    node: &'a AudioBlock,
    index: usize,
}
impl<'a> ChainCursor<'a> {
    fn new(node: &'a AudioBlock) -> Self {
        Self { node, index: 0 }
    }
}
impl Iterator for ChainCursor<'_> {
    type Item = Sample;
    fn next(&mut self) -> Option<Sample> {
        loop {
            if self.index < self.node.samples.len() {
                let value = self.node.samples[self.index];
                self.index += 1;
                return Some(value);
            }
            self.node = self.node.next.as_deref()?;
            self.index = 0;
        }
    }
}
