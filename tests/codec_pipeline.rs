use freewheeling_plus::block::{AudioBlock, AudioBlockIterator, Codec};
use freewheeling_plus::block_managers::{BlockReadManager, BlockWriteManager};
use freewheeling_plus::file_codecs::{IFileDecoder, IFileEncoder, SndFileDecoder, SndFileEncoder};
use freewheeling_plus::mem::Preallocated;
use std::io::{self, Cursor, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Clone, Default)]
struct Bytes(Arc<Mutex<Cursor<Vec<u8>>>>);

impl Write for Bytes {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(data)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Bytes {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.lock().unwrap().seek(position)
    }
}

fn samples(n: usize, phase: f32) -> Vec<f32> {
    (0..n)
        .map(|i| ((i as f32 * 0.037) + phase).sin() * 0.6)
        .collect()
}

fn encode(format: Codec, stereo: bool, left: &[f32], right: Option<&[f32]>) -> Vec<u8> {
    let bytes = Arc::new(Mutex::new(Cursor::new(Vec::new())));
    let mut enc = SndFileEncoder::new(44_100, stereo, format).unwrap();
    enc.setup_file_for_writing(Bytes(bytes.clone())).unwrap();
    enc.write_samples_to_disk(left, right).unwrap();
    enc.prepare_file_for_closing().unwrap();
    bytes.lock().unwrap().get_ref().clone()
}

fn decode_into_chain(
    format: Codec,
    bytes: Vec<u8>,
    expected: &[f32],
) -> (AudioBlock, SndFileDecoder) {
    let block = Arc::new(RwLock::new(AudioBlock::new(expected.len())));
    let mut read = BlockReadManager::new(block.clone());
    let mut decoder = SndFileDecoder::new(44_100, format);
    decoder.read_from_file(Cursor::new(bytes)).unwrap();
    let mut out = AudioBlock::new(expected.len());
    let mut it = AudioBlockIterator::new(&mut out, 7);
    assert_eq!(
        decoder.read_samples(&mut it, expected.len()).unwrap(),
        expected.len()
    );
    read.start(out.samples.clone());
    assert!(!read.manage());
    let mut write = BlockWriteManager::new(block);
    assert!(!write.manage());
    let tolerance = if format == Codec::Vorbis { 0.15 } else { 1e-6 };
    assert_eq!(write.output.len(), expected.len());
    assert!(
        write
            .output
            .iter()
            .zip(expected)
            .all(|(actual, wanted)| (actual - wanted).abs() <= tolerance)
    );
    (out, decoder)
}

#[test]
fn wav_and_vorbis_mono_pipeline_uses_managed_blocks() {
    for format in [Codec::Wav, Codec::Vorbis, Codec::Au] {
        let input = samples(2048, 0.0);
        let bytes = encode(format, false, &input, None);
        assert!(!bytes.is_empty());
        let (mut block, mut decoder) = decode_into_chain(format, bytes, &input);
        assert!(!decoder.stereo());
        decoder.stop();
        let mut it = AudioBlockIterator::new(&mut block, 8);
        assert_eq!(decoder.read_samples(&mut it, input.len()).unwrap(), 0);
    }
}

#[test]
fn wav_and_vorbis_stereo_pipeline_preserves_channel_shape() {
    for format in [Codec::Wav, Codec::Vorbis] {
        let left = samples(2048, 0.0);
        let right = samples(2048, 1.1);
        let bytes = encode(format, true, &left, Some(&right));
        let block = Arc::new(RwLock::new(AudioBlock::new(left.len())));
        block.write().unwrap().extra =
            Some(freewheeling_plus::block::ExtraChannel::new(left.len()));
        let mut decoder = SndFileDecoder::new(44_100, format);
        decoder.read_from_file(Cursor::new(bytes)).unwrap();
        let mut out = AudioBlock::new(left.len());
        out.extra = Some(freewheeling_plus::block::ExtraChannel::new(left.len()));
        let mut it = AudioBlockIterator::new(&mut out, 17);
        assert_eq!(
            decoder.read_samples(&mut it, left.len()).unwrap(),
            left.len()
        );
        assert!(decoder.stereo());
        let tolerance = if format == Codec::Vorbis { 0.15 } else { 1e-6 };
        assert!(
            out.extra
                .as_ref()
                .unwrap()
                .samples
                .iter()
                .zip(&right)
                .all(|(actual, wanted)| (actual - wanted).abs() <= tolerance)
        );
    }
}

#[test]
fn encoders_write_before_finalize() {
    for format in [Codec::Wav, Codec::Vorbis] {
        let bytes = Arc::new(Mutex::new(Cursor::new(Vec::new())));
        let mut enc = SndFileEncoder::new(44_100, false, format).unwrap();
        enc.setup_file_for_writing(Bytes(bytes.clone())).unwrap();
        enc.write_samples_to_disk(&samples(4096, 0.0), None)
            .unwrap();
        assert!(
            !bytes.lock().unwrap().get_ref().is_empty(),
            "{format:?} buffered until finalize"
        );
        enc.prepare_file_for_closing().unwrap();
    }
}

#[test]
fn finalize_stop_errors_and_recycling_are_deterministic() {
    let mut enc = SndFileEncoder::new(44_100, true, Codec::Wav).unwrap();
    assert_eq!(
        enc.write_samples_to_disk(&[1.0], Some(&[1.0]))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotConnected
    );
    let bytes = Arc::new(Mutex::new(Cursor::new(Vec::new())));
    enc.setup_file_for_writing(Bytes(bytes)).unwrap();
    assert_eq!(
        enc.write_samples_to_disk(&[1.0], None).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    enc.write_samples_to_disk(&[1.0], Some(&[0.5])).unwrap();
    enc.prepare_file_for_closing().unwrap();
    assert_eq!(
        enc.prepare_file_for_closing().unwrap_err().kind(),
        io::ErrorKind::NotConnected
    );

    let block = Arc::new(RwLock::new(AudioBlock::new(4)));
    let mut manager = BlockReadManager::new(block.clone());
    manager.start(vec![1.0, 2.0]);
    manager.manage();
    manager.recycle();
    assert!(manager.base.block.is_none());
    assert!(manager.input.is_empty());
    assert!(manager.done);
}
