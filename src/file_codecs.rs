//! Stateful, bounded WAV, Ogg Vorbis, FLAC, and AU codecs.

use crate::block::{AudioBlockIterator, Codec, Sample};
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::num::{NonZeroU8, NonZeroU32};
use std::path::Path;

const FLAC_BLOCK_SIZE: usize = 4096;
pub const MAX_STREAMING_FRAMES: usize = 16_384;

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}
trait WriteSeek: Write + Seek {}
impl<T: Write + Seek> WriteSeek for T {}

pub trait IFileEncoder {
    fn setup_file_for_writing<W: Write + Seek + 'static>(&mut self, out: W) -> io::Result<()>;
    fn write_samples_to_disk(
        &mut self,
        left: &[Sample],
        right: Option<&[Sample]>,
    ) -> io::Result<usize>;
    fn prepare_file_for_closing(&mut self) -> io::Result<()>;
}

pub trait IFileDecoder {
    fn read_from_file<R: Read + Seek + 'static>(&mut self, input: R) -> io::Result<()>;
    fn read_samples(
        &mut self,
        it: &mut AudioBlockIterator<'_>,
        max_len: usize,
    ) -> io::Result<usize>;
    fn stop(&mut self);
    fn stereo(&self) -> bool;
}

pub struct SndFileEncoder {
    pub sample_rate: u32,
    pub stereo: bool,
    format: Codec,
    output: Option<EncoderOutput>,
}

enum EncoderOutput {
    Wav(hound::WavWriter<Box<dyn WriteSeek>>),
    Vorbis(Box<vorbis_rs::VorbisEncoder<Box<dyn WriteSeek>>>),
    Flac(Box<FlacEncoder>),
    Au {
        out: Box<dyn WriteSeek>,
        data_len: u32,
    },
}

struct FlacEncoder {
    out: Box<dyn WriteSeek>,
    config: flacenc::error::Verified<flacenc::config::Encoder>,
    info: flacenc::component::StreamInfo,
    context: flacenc::source::Context,
    frame: flacenc::source::FrameBuf,
    samples: Vec<i32>,
    frame_number: usize,
    channels: usize,
}

impl FlacEncoder {
    fn new(mut out: Box<dyn WriteSeek>, rate: u32, channels: usize) -> io::Result<Self> {
        use flacenc::component::BitRepr;
        use flacenc::error::Verify;
        let config = flacenc::config::Encoder::default()
            .into_verified()
            .map_err(|(_, error)| ioerr(format!("{error:?}")))?;
        let info =
            flacenc::component::StreamInfo::new(rate as usize, channels, 24).map_err(ioerr)?;
        let stream = flacenc::component::Stream::with_stream_info(info.clone());
        let mut sink = flacenc::bitsink::ByteSink::new();
        stream.write(&mut sink).map_err(ioerr)?;
        out.write_all(sink.as_slice())?;
        Ok(Self {
            out,
            config,
            info,
            context: flacenc::source::Context::new(24, channels),
            frame: flacenc::source::FrameBuf::with_size(channels, FLAC_BLOCK_SIZE)
                .map_err(ioerr)?,
            samples: Vec::with_capacity(FLAC_BLOCK_SIZE * channels),
            frame_number: 0,
            channels,
        })
    }

    fn push(&mut self, left: &[Sample], right: Option<&[Sample]>) -> io::Result<()> {
        for i in 0..left.len() {
            self.samples.push(float_to_i24(left[i]));
            if let Some(right) = right {
                self.samples.push(float_to_i24(right[i]));
            }
            if self.samples.len() == FLAC_BLOCK_SIZE * self.channels {
                self.flush_frame()?;
            }
        }
        Ok(())
    }

    fn flush_frame(&mut self) -> io::Result<()> {
        use flacenc::component::BitRepr;
        use flacenc::source::Fill;
        if self.samples.is_empty() {
            return Ok(());
        }
        self.frame.fill_interleaved(&self.samples).map_err(ioerr)?;
        self.context
            .fill_interleaved(&self.samples)
            .map_err(ioerr)?;
        let encoded = flacenc::encode_fixed_size_frame(
            &self.config,
            &self.frame,
            self.frame_number,
            &self.info,
        )
        .map_err(ioerr)?;
        self.info.update_frame_info(&encoded);
        let mut sink = flacenc::bitsink::ByteSink::new();
        encoded.write(&mut sink).map_err(ioerr)?;
        self.out.write_all(sink.as_slice())?;
        self.samples.clear();
        self.frame_number += 1;
        Ok(())
    }

    fn finish(mut self) -> io::Result<()> {
        use flacenc::component::BitRepr;
        self.flush_frame()?;
        self.info.set_md5_digest(&self.context.md5_digest());
        let stream = flacenc::component::Stream::with_stream_info(self.info);
        let mut sink = flacenc::bitsink::ByteSink::new();
        stream.write(&mut sink).map_err(ioerr)?;
        self.out.seek(SeekFrom::Start(0))?;
        self.out.write_all(sink.as_slice())?;
        self.out.flush()
    }
}

impl SndFileEncoder {
    pub fn new(sample_rate: u32, stereo: bool, format: Codec) -> io::Result<Self> {
        if sample_rate == 0
            || !matches!(format, Codec::Wav | Codec::Vorbis | Codec::Flac | Codec::Au)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid codec or sample rate",
            ));
        }
        Ok(Self {
            sample_rate,
            stereo,
            format,
            output: None,
        })
    }
}

/// Encode an audio file incrementally without staging the encoded file in
/// memory. A partially written destination is removed on every error.
pub fn encode_audio_file(
    path: &Path,
    sample_rate: u32,
    format: Codec,
    left: &[Sample],
    right: Option<&[Sample]>,
) -> io::Result<()> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = (|| {
        let mut encoder = SndFileEncoder::new(sample_rate, right.is_some(), format)?;
        encoder.setup_file_for_writing(file)?;
        let frame_count = left.len().min(right.map_or(left.len(), <[Sample]>::len));
        for start in (0..frame_count).step_by(MAX_STREAMING_FRAMES) {
            let end = (start + MAX_STREAMING_FRAMES).min(frame_count);
            encoder.write_samples_to_disk(
                &left[start..end],
                right.map(|samples| &samples[start..end]),
            )?;
        }
        encoder.prepare_file_for_closing()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

impl IFileEncoder for SndFileEncoder {
    fn setup_file_for_writing<W: Write + Seek + 'static>(&mut self, out: W) -> io::Result<()> {
        if self.output.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "encoder is already open",
            ));
        }
        let mut out: Box<dyn WriteSeek> = Box::new(out);
        self.output = Some(match self.format {
            Codec::Wav => EncoderOutput::Wav(
                hound::WavWriter::new(
                    out,
                    hound::WavSpec {
                        channels: if self.stereo { 2 } else { 1 },
                        sample_rate: self.sample_rate,
                        bits_per_sample: 32,
                        sample_format: hound::SampleFormat::Float,
                    },
                )
                .map_err(ioerr)?,
            ),
            Codec::Vorbis => EncoderOutput::Vorbis(Box::new(
                vorbis_rs::VorbisEncoderBuilder::new(
                    NonZeroU32::new(self.sample_rate).unwrap(),
                    NonZeroU8::new(if self.stereo { 2 } else { 1 }).unwrap(),
                    out,
                )
                .map_err(ioerr)?
                .build()
                .map_err(ioerr)?,
            )),
            Codec::Flac => EncoderOutput::Flac(Box::new(FlacEncoder::new(
                out,
                self.sample_rate,
                if self.stereo { 2 } else { 1 },
            )?)),
            Codec::Au => {
                write_au_header(&mut out, self.sample_rate, self.stereo)?;
                EncoderOutput::Au { out, data_len: 0 }
            }
            _ => unreachable!(),
        });
        Ok(())
    }

    fn write_samples_to_disk(
        &mut self,
        left: &[Sample],
        right: Option<&[Sample]>,
    ) -> io::Result<usize> {
        let right = if self.stereo {
            Some(right.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "stereo encoder needs a right channel",
                )
            })?)
        } else {
            None
        };
        let n = left.len().min(right.map_or(left.len(), <[Sample]>::len));
        match self
            .output
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "encoder is not open"))?
        {
            EncoderOutput::Wav(writer) => {
                for i in 0..n {
                    writer.write_sample(left[i]).map_err(ioerr)?;
                    if let Some(r) = right {
                        writer.write_sample(r[i]).map_err(ioerr)?;
                    }
                }
            }
            EncoderOutput::Vorbis(encoder) => {
                if let Some(r) = right {
                    encoder
                        .encode_audio_block([&left[..n], &r[..n]])
                        .map_err(ioerr)?
                } else {
                    encoder.encode_audio_block([&left[..n]]).map_err(ioerr)?
                }
            }
            EncoderOutput::Flac(encoder) => encoder.push(&left[..n], right.map(|r| &r[..n]))?,
            EncoderOutput::Au { out, data_len } => {
                for i in 0..n {
                    out.write_all(&float_to_i32(left[i]).to_be_bytes())?;
                    if let Some(r) = right {
                        out.write_all(&float_to_i32(r[i]).to_be_bytes())?;
                    }
                }
                let bytes = u32::try_from(
                    n.checked_mul(if self.stereo { 8 } else { 4 })
                        .ok_or_else(|| io::Error::other("AU data length overflow"))?,
                )
                .map_err(|_| io::Error::other("AU data length overflow"))?;
                *data_len = data_len
                    .checked_add(bytes)
                    .ok_or_else(|| io::Error::other("AU data length overflow"))?;
            }
        }
        Ok(n)
    }

    fn prepare_file_for_closing(&mut self) -> io::Result<()> {
        match self
            .output
            .take()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "encoder is not open"))?
        {
            EncoderOutput::Wav(writer) => writer.finalize().map_err(ioerr),
            EncoderOutput::Vorbis(encoder) => (*encoder).finish().map(|_| ()).map_err(ioerr),
            EncoderOutput::Flac(encoder) => (*encoder).finish(),
            EncoderOutput::Au { mut out, data_len } => {
                out.seek(SeekFrom::Start(8))?;
                out.write_all(&data_len.to_be_bytes())?;
                out.flush()
            }
        }
    }
}

pub type VorbisEncoder = SndFileEncoder;

enum DecoderInput {
    Wav(hound::WavReader<Box<dyn ReadSeek>>),
    Vorbis {
        decoder: Box<vorbis_rs::VorbisDecoder<Box<dyn ReadSeek>>>,
        pending: Vec<Vec<Sample>>,
        pos: usize,
    },
    Flac {
        reader: claxon::FlacReader<Box<dyn ReadSeek>>,
        pending: Vec<Vec<Sample>>,
        pos: usize,
    },
    Au {
        input: Box<dyn ReadSeek>,
        remaining: Option<u64>,
        channels: usize,
    },
}

pub struct SndFileDecoder {
    pub sample_rate: u32,
    pub stereo: bool,
    format: Codec,
    input: Option<DecoderInput>,
    left: Vec<Sample>,
    right: Vec<Sample>,
}

impl SndFileDecoder {
    pub fn new(sample_rate: u32, format: Codec) -> Self {
        Self {
            sample_rate,
            stereo: false,
            format,
            input: None,
            left: Vec::new(),
            right: Vec::new(),
        }
    }

    fn buffers(&mut self, n: usize) {
        self.left.clear();
        self.right.clear();
        self.left.reserve(n);
        if self.stereo {
            self.right.reserve(n);
        }
    }
}

impl IFileDecoder for SndFileDecoder {
    fn read_from_file<R: Read + Seek + 'static>(&mut self, input: R) -> io::Result<()> {
        self.stop();
        let mut input: Box<dyn ReadSeek> = Box::new(input);
        self.input = Some(match self.format {
            Codec::Wav => {
                let reader = hound::WavReader::new(input).map_err(ioerr)?;
                let spec = reader.spec();
                if spec.sample_rate != self.sample_rate
                    || !(spec.channels == 1 || spec.channels == 2)
                    || spec.bits_per_sample != 32
                    || spec.sample_format != hound::SampleFormat::Float
                {
                    return Err(invalid("unsupported WAV format"));
                }
                self.stereo = spec.channels == 2;
                DecoderInput::Wav(reader)
            }
            Codec::Vorbis => {
                let decoder = vorbis_rs::VorbisDecoder::new(input).map_err(ioerr)?;
                if decoder.sampling_frequency().get() != self.sample_rate
                    || !(decoder.channels().get() == 1 || decoder.channels().get() == 2)
                {
                    return Err(invalid("unsupported Vorbis format"));
                }
                self.stereo = decoder.channels().get() == 2;
                DecoderInput::Vorbis {
                    decoder: Box::new(decoder),
                    pending: vec![Vec::new(), Vec::new()],
                    pos: 0,
                }
            }
            Codec::Flac => {
                let reader = claxon::FlacReader::new(input).map_err(ioerr)?;
                let info = reader.streaminfo();
                if info.sample_rate != self.sample_rate
                    || !(info.channels == 1 || info.channels == 2)
                    || info.bits_per_sample != 24
                {
                    return Err(invalid("unsupported FLAC format"));
                }
                self.stereo = info.channels == 2;
                DecoderInput::Flac {
                    reader,
                    pending: vec![Vec::new(), Vec::new()],
                    pos: 0,
                }
            }
            Codec::Au => {
                // The historical C++ encoder accidentally selected WAV/float
                // for AU.  Accept those mislabeled library files on input,
                // while continuing to emit a standards-compliant .snd file.
                let mut magic = [0; 4];
                input.read_exact(&mut magic)?;
                input.seek(SeekFrom::Start(0))?;
                if &magic == b"RIFF" {
                    let reader = hound::WavReader::new(input).map_err(ioerr)?;
                    let spec = reader.spec();
                    if spec.sample_rate != self.sample_rate
                        || !(spec.channels == 1 || spec.channels == 2)
                        || spec.bits_per_sample != 32
                        || spec.sample_format != hound::SampleFormat::Float
                    {
                        return Err(invalid("unsupported historical AU/WAV format"));
                    }
                    self.stereo = spec.channels == 2;
                    return {
                        self.input = Some(DecoderInput::Wav(reader));
                        Ok(())
                    };
                }
                let (offset, size, channels) = read_au_header(&mut input, self.sample_rate)?;
                input.seek(SeekFrom::Start(offset))?;
                self.stereo = channels == 2;
                DecoderInput::Au {
                    input,
                    remaining: (size != u32::MAX).then_some(size as u64),
                    channels: channels as usize,
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported codec",
                ));
            }
        });
        Ok(())
    }

    fn read_samples(
        &mut self,
        it: &mut AudioBlockIterator<'_>,
        max_len: usize,
    ) -> io::Result<usize> {
        let capacity = max_len
            .min(MAX_STREAMING_FRAMES)
            .min(it.block.total_len().saturating_sub(it.position));
        self.buffers(capacity);
        let input = match self.input.as_mut() {
            Some(input) => input,
            // Preserve the legacy decoder contract: reads after `stop` are a
            // clean end-of-stream, which lets streamer shutdown drain safely.
            None => return Ok(0),
        };
        match input {
            DecoderInput::Wav(reader) => {
                let channels = if self.stereo { 2 } else { 1 };
                let mut samples = reader.samples::<f32>();
                for _ in 0..capacity {
                    let Some(left) = samples.next() else { break };
                    self.left.push(left.map_err(ioerr)?);
                    if channels == 2 {
                        self.right.push(
                            samples
                                .next()
                                .ok_or_else(|| invalid("truncated WAV frame"))?
                                .map_err(ioerr)?,
                        );
                    }
                }
            }
            DecoderInput::Flac {
                reader,
                pending,
                pos,
            } => {
                while self.left.len() < capacity {
                    if *pos == pending[0].len() {
                        let Some(block) = reader
                            .blocks()
                            .read_next_or_eof(Vec::new())
                            .map_err(ioerr)?
                        else {
                            break;
                        };
                        pending[0].clear();
                        pending[0].extend(
                            block
                                .channel(0)
                                .iter()
                                .map(|value| *value as f32 / 8_388_608.0),
                        );
                        pending[1].clear();
                        if self.stereo {
                            pending[1].extend(
                                block
                                    .channel(1)
                                    .iter()
                                    .map(|value| *value as f32 / 8_388_608.0),
                            );
                        }
                        *pos = 0;
                    }
                    let n = (capacity - self.left.len()).min(pending[0].len() - *pos);
                    self.left.extend_from_slice(&pending[0][*pos..*pos + n]);
                    if self.stereo {
                        self.right.extend_from_slice(&pending[1][*pos..*pos + n]);
                    }
                    *pos += n;
                }
            }
            DecoderInput::Vorbis {
                decoder,
                pending,
                pos,
            } => {
                while self.left.len() < capacity {
                    if *pos == pending[0].len() {
                        let Some(block) = decoder.decode_audio_block().map_err(ioerr)? else {
                            break;
                        };
                        let samples = block.samples();
                        pending[0].clear();
                        pending[0].extend_from_slice(samples[0]);
                        pending[1].clear();
                        if self.stereo {
                            pending[1].extend_from_slice(samples[1]);
                        }
                        *pos = 0;
                    }
                    let n = (capacity - self.left.len()).min(pending[0].len() - *pos);
                    self.left.extend_from_slice(&pending[0][*pos..*pos + n]);
                    if self.stereo {
                        self.right.extend_from_slice(&pending[1][*pos..*pos + n]);
                    }
                    *pos += n;
                }
            }
            DecoderInput::Au {
                input,
                remaining,
                channels,
            } => {
                let frame_bytes = *channels * 4;
                let mut frame = [0u8; 8];
                for _ in 0..capacity {
                    if remaining.is_some_and(|n| n < frame_bytes as u64) {
                        if remaining != &Some(0) {
                            return Err(invalid("truncated AU frame"));
                        }
                        break;
                    }
                    match input.read_exact(&mut frame[..frame_bytes]) {
                        Ok(()) => {}
                        Err(error)
                            if error.kind() == io::ErrorKind::UnexpectedEof
                                && remaining.is_none() =>
                        {
                            break;
                        }
                        Err(error) => return Err(error),
                    }
                    self.left.push(
                        i32::from_be_bytes(frame[..4].try_into().unwrap()) as f32 / 2_147_483_648.0,
                    );
                    if *channels == 2 {
                        self.right.push(
                            i32::from_be_bytes(frame[4..8].try_into().unwrap()) as f32
                                / 2_147_483_648.0,
                        );
                    }
                    if let Some(n) = remaining {
                        *n -= frame_bytes as u64;
                    }
                }
            }
        }
        let n = self.left.len();
        if self.stereo {
            it.put_fragment_stereo(&self.left, &self.right);
        } else {
            it.put_fragment(&self.left);
        }
        Ok(n)
    }

    fn stop(&mut self) {
        self.input = None;
        self.left.clear();
        self.right.clear();
    }
    fn stereo(&self) -> bool {
        self.stereo
    }
}

pub type VorbisDecoder = SndFileDecoder;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn ioerr<E: std::fmt::Display>(error: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
fn float_to_i24(sample: f32) -> i32 {
    (sample.clamp(-1.0, 1.0 - f32::EPSILON) * 8_388_608.0).round() as i32
}
fn float_to_i32(sample: f32) -> i32 {
    (sample.clamp(-1.0, 1.0 - f32::EPSILON) * 2_147_483_648.0).round() as i32
}

fn write_au_header(out: &mut dyn Write, sample_rate: u32, stereo: bool) -> io::Result<()> {
    out.write_all(b".snd")?;
    out.write_all(&24u32.to_be_bytes())?;
    out.write_all(&u32::MAX.to_be_bytes())?;
    out.write_all(&5u32.to_be_bytes())?;
    out.write_all(&sample_rate.to_be_bytes())?;
    out.write_all(&(if stereo { 2u32 } else { 1 }).to_be_bytes())
}

fn read_au_header(input: &mut dyn Read, rate: u32) -> io::Result<(u64, u32, u32)> {
    let mut header = [0u8; 24];
    input.read_exact(&mut header)?;
    if &header[..4] != b".snd" {
        return Err(invalid("invalid AU header"));
    }
    let word = |at| u32::from_be_bytes(header[at..at + 4].try_into().unwrap());
    let offset = word(4);
    let size = word(8);
    let encoding = word(12);
    let sample_rate = word(16);
    let channels = word(20);
    if offset < 24
        || encoding != 5
        || sample_rate != rate
        || !(channels == 1 || channels == 2)
        || (size != u32::MAX && size % (channels * 4) != 0)
    {
        return Err(invalid("unsupported AU format"));
    }
    Ok((offset as u64, size, channels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::AudioBlock;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    struct SharedWriter(Arc<Mutex<Cursor<Vec<u8>>>>);
    impl Write for SharedWriter {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().write(b)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Seek for SharedWriter {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.0.lock().unwrap().seek(p)
        }
    }

    fn round_trip(format: Codec, stereo: bool) {
        let left: Vec<_> = (0..10_137)
            .map(|i| (i as f32 * 0.013).sin() * 0.5)
            .collect();
        let right: Vec<_> = (0..10_137)
            .map(|i| (i as f32 * 0.009).cos() * 0.25)
            .collect();
        let bytes = Arc::new(Mutex::new(Cursor::new(Vec::new())));
        let mut enc = SndFileEncoder::new(44_100, stereo, format).unwrap();
        enc.setup_file_for_writing(SharedWriter(bytes.clone()))
            .unwrap();
        for at in (0..left.len()).step_by(317) {
            let end = (at + 317).min(left.len());
            enc.write_samples_to_disk(&left[at..end], stereo.then_some(&right[at..end]))
                .unwrap();
        }
        enc.prepare_file_for_closing().unwrap();
        let encoded = bytes.lock().unwrap().get_ref().clone();
        let mut dec = SndFileDecoder::new(44_100, format);
        dec.read_from_file(Cursor::new(encoded)).unwrap();
        let mut block = AudioBlock::new(left.len());
        if stereo {
            block.extra = Some(crate::block::ExtraChannel::new(left.len()));
        }
        let mut it = AudioBlockIterator::new(&mut block, 113);
        let mut calls = 0;
        while dec.read_samples(&mut it, 113).unwrap() != 0 {
            calls += 1;
        }
        assert!(calls > 2);
        let tolerance = if format == Codec::Vorbis { 0.15 } else { 2e-6 };
        for (i, expected) in left.iter().enumerate() {
            let actual = block.sample(i).unwrap();
            assert!(
                (actual - expected).abs() < tolerance,
                "sample {i}: {actual} != {expected}"
            );
        }
        if stereo {
            for (i, expected) in right.iter().enumerate() {
                assert!((block.extra.as_ref().unwrap().samples[i] - expected).abs() < tolerance);
            }
        }
    }

    #[test]
    fn wav_streaming_round_trip() {
        round_trip(Codec::Wav, true);
    }
    #[test]
    fn vorbis_streaming_round_trip() {
        round_trip(Codec::Vorbis, true);
    }
    #[test]
    fn flac_incremental_round_trip() {
        round_trip(Codec::Flac, true);
    }
    #[test]
    fn au_streaming_round_trip() {
        round_trip(Codec::Au, true);
    }
}
