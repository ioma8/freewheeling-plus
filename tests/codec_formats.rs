use freewheeling_plus::block::{AudioBlock, AudioBlockIterator, Codec};
use freewheeling_plus::file_codecs::{IFileDecoder, IFileEncoder, SndFileDecoder, SndFileEncoder};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

fn encode(format: Codec, stereo: bool, left: &[f32], right: Option<&[f32]>) -> Vec<u8> {
    let bytes = Arc::new(Mutex::new(Cursor::new(Vec::new())));
    let mut encoder = SndFileEncoder::new(48_000, stereo, format).unwrap();
    encoder
        .setup_file_for_writing(SharedWriter(bytes.clone()))
        .unwrap();
    encoder.write_samples_to_disk(left, right).unwrap();
    encoder.prepare_file_for_closing().unwrap();
    bytes.lock().unwrap().get_ref().clone()
}

fn round_trip(format: Codec, stereo: bool) {
    let left: Vec<f32> = (0..257).map(|i| (i as f32 * 0.031).sin() * 0.7).collect();
    let right: Vec<f32> = (0..257).map(|i| (i as f32 * 0.017).cos() * 0.4).collect();
    let bytes = encode(format, stereo, &left, stereo.then_some(right.as_slice()));
    let mut decoder = SndFileDecoder::new(48_000, format);
    decoder.read_from_file(Cursor::new(bytes)).unwrap();
    let mut block = AudioBlock::new(left.len());
    let mut it = AudioBlockIterator::new(&mut block, 19);
    while decoder.read_samples(&mut it, 19).unwrap() != 0 {}
    assert_eq!(decoder.stereo(), stereo);
    assert!(
        block
            .samples
            .iter()
            .zip(left.iter())
            .all(|(a, b)| (a - b).abs() < 1e-6)
    );
    if stereo {
        assert!(
            block
                .extra
                .as_ref()
                .unwrap()
                .samples
                .iter()
                .zip(right.iter())
                .all(|(a, b)| (a - b).abs() < 1e-6)
        );
    }
}

#[test]
fn flac_mono_and_stereo_round_trip() {
    round_trip(Codec::Flac, false);
    round_trip(Codec::Flac, true);
}

#[test]
fn au_mono_and_stereo_round_trip() {
    round_trip(Codec::Au, false);
    round_trip(Codec::Au, true);
}

#[test]
fn codec_signatures_are_real_formats() {
    let samples = [0.0, 0.25, -0.25];
    assert_eq!(&encode(Codec::Flac, false, &samples, None)[..4], b"fLaC");
    assert_eq!(&encode(Codec::Au, false, &samples, None)[..4], b".snd");
}

struct SharedWriter(Arc<Mutex<Cursor<Vec<u8>>>>);
impl std::io::Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl std::io::Seek for SharedWriter {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.lock().unwrap().seek(pos)
    }
}
