use freewheeling_plus::block::{AudioBlock, AudioBlockIterator, Codec, ExtraChannel};
use freewheeling_plus::core_persistence::{
    LoopMeta, Scene, SnapshotLoop, SnapshotMeta, loop_metadata_xml, scene_xml,
};
use freewheeling_plus::core_persistence_parse::{parse_loop_metadata_xml, parse_scene};
use freewheeling_plus::file_codecs::{IFileDecoder, IFileEncoder, SndFileDecoder, SndFileEncoder};
use std::fs;
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

const RATE: u32 = 48_000;
const FRAMES: usize = 4096;

fn golden(directory: &str, file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/cpp-golden")
        .join(directory)
        .join(file)
}

fn reference() -> (Vec<f32>, Vec<f32>) {
    let left = (0..FRAMES)
        .map(|i| {
            (0.55_f64 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / RATE as f64).sin()) as f32
        })
        .collect();
    let right = (0..FRAMES)
        .map(|i| {
            (0.35_f64 * (2.0 * std::f64::consts::PI * 660.0 * i as f64 / RATE as f64).cos()) as f32
        })
        .collect();
    (left, right)
}

fn decode(format: Codec, bytes: Vec<u8>) -> AudioBlock {
    let mut decoder = SndFileDecoder::new(RATE, format);
    decoder.read_from_file(Cursor::new(bytes)).unwrap();
    assert!(decoder.stereo());
    let mut block = AudioBlock::new(FRAMES);
    block.extra = Some(ExtraChannel::new(FRAMES));
    let mut iterator = AudioBlockIterator::new(&mut block, 127);
    let mut frames = 0;
    loop {
        let read = decoder.read_samples(&mut iterator, 127).unwrap();
        if read == 0 {
            break;
        }
        frames += read;
    }
    assert_eq!(frames, FRAMES);
    block
}

fn assert_pcm(block: &AudioBlock, tolerance: f32) {
    let (left, right) = reference();
    let decoded_right = &block.extra.as_ref().unwrap().samples;
    for (channel, actual, expected) in [
        ("left", &block.samples, &left),
        ("right", decoded_right, &right),
    ] {
        let (index, error) = actual
            .iter()
            .zip(expected)
            .enumerate()
            .map(|(i, (a, e))| (i, (a - e).abs()))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert!(
            error <= tolerance,
            "{channel} sample {index} differs by {error}, tolerance {tolerance}"
        );
    }
}

#[test]
fn rust_decodes_genuine_cpp_codec_outputs() {
    for (name, format, tolerance) in [
        ("reference.wav", Codec::Wav, f32::EPSILON),
        ("reference.flac", Codec::Flac, 1.2e-7),
        ("reference.ogg", Codec::Vorbis, 0.15),
    ] {
        let block = decode(format, fs::read(golden("codec", name)).unwrap());
        assert_pcm(&block, tolerance);
    }
}

#[test]
fn historical_cpp_sndfile_harness_reloads_rust_outputs() {
    if !Command::new("pkg-config")
        .args(["--exists", "sndfile"])
        .status()
        .is_ok_and(|status| status.success())
    {
        eprintln!("skipping C++ reload parity: libsndfile development files unavailable");
        return;
    }
    let compiler = if Command::new("c++").arg("--version").output().is_ok() {
        "c++"
    } else {
        eprintln!("skipping C++ reload parity: C++ compiler unavailable");
        return;
    };
    let directory =
        std::env::temp_dir().join(format!("freewheeling-cpp-reload-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir(&directory).unwrap();
    let source = directory.join("reload.cc");
    fs::write(
        &source,
        r#"#include <sndfile.h>
#include <cmath>
#include <cstdio>
int main(int argc, char **argv) {
  if (argc != 4) return 2;
  for (int file = 1; file < argc; ++file) {
    SF_INFO info{};
    SNDFILE *in = sf_open(argv[file], SFM_READ, &info);
    if (!in || info.samplerate != 48000 || info.channels != 2 || info.frames != 4096) return 10 + file;
    float frame[2];
    for (int i = 0; i < 4096; ++i) {
      if (sf_readf_float(in, frame, 1) != 1) return 20 + file;
      const float left = 0.55f * std::sin(2.0 * M_PI * 440.0 * i / 48000.0);
      const float right = 0.35f * std::cos(2.0 * M_PI * 660.0 * i / 48000.0);
      if (std::fabs(frame[0] - left) > 2e-6f || std::fabs(frame[1] - right) > 2e-6f) return 30 + file;
    }
    sf_close(in);
  }
}
"#,
    )
    .unwrap();
    let flags = Command::new("pkg-config")
        .args(["--cflags", "--libs", "sndfile"])
        .output()
        .unwrap();
    assert!(flags.status.success());
    let harness = directory.join("reload");
    let mut compile = Command::new(compiler);
    compile
        .arg("-std=c++20")
        .arg(&source)
        .arg("-o")
        .arg(&harness);
    compile.args(String::from_utf8(flags.stdout).unwrap().split_whitespace());
    let output = compile.output().unwrap();
    assert!(
        output.status.success(),
        "C++ reload harness failed to compile: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let (left, right) = reference();
    let mut paths = Vec::new();
    for (name, format) in [
        ("rust.wav", Codec::Wav),
        ("rust.flac", Codec::Flac),
        ("rust.au", Codec::Au),
    ] {
        let path = directory.join(name);
        let mut encoder = SndFileEncoder::new(RATE, true, format).unwrap();
        encoder
            .setup_file_for_writing(fs::File::create(&path).unwrap())
            .unwrap();
        encoder.write_samples_to_disk(&left, Some(&right)).unwrap();
        encoder.prepare_file_for_closing().unwrap();
        paths.push(path);
    }
    let output = Command::new(&harness).args(&paths).output().unwrap();
    let _ = fs::remove_dir_all(&directory);
    assert!(
        output.status.success(),
        "historical C++ reload harness rejected Rust output (status {:?})",
        output.status.code()
    );
}

#[test]
fn historical_au_routing_is_accepted_only_on_input() {
    let historical = fs::read(golden("codec", "reference.wav")).unwrap();
    assert_pcm(&decode(Codec::Au, historical), f32::EPSILON);

    let (left, right) = reference();
    let output = SharedBytes::default();
    let mut encoder = SndFileEncoder::new(RATE, true, Codec::Au).unwrap();
    encoder.setup_file_for_writing(output.clone()).unwrap();
    encoder.write_samples_to_disk(&left, Some(&right)).unwrap();
    encoder.prepare_file_for_closing().unwrap();
    let bytes = output.bytes();
    assert_eq!(&bytes[..4], b".snd");
    assert_pcm(&decode(Codec::Au, bytes), 1.2e-7);
}

#[test]
fn rust_loads_cpp_persistence_semantics_and_round_trips_them() {
    let scene_text = fs::read_to_string(golden("persistence", "scene.xml")).unwrap();
    let scene = parse_scene(&scene_text).unwrap();
    let expected = Scene {
        loops: vec![LoopMeta {
            hash: "00112233445566778899aabbccddeeff".into(),
            loop_id: 3,
            volume: 0.625,
        }],
        snapshots: vec![SnapshotMeta {
            id: 2,
            name: "Golden & snapshot".into(),
            loops: vec![SnapshotLoop {
                loop_id: 3,
                status: 1,
                loop_volume: 0.625,
                trigger_volume: 0.75,
            }],
        }],
    };
    assert_eq!(scene, expected);
    assert_eq!(parse_scene(&scene_xml(&scene)).unwrap(), expected);

    let metadata_text = fs::read_to_string(golden("persistence", "loop.xml")).unwrap();
    let metadata = parse_loop_metadata_xml(&metadata_text).unwrap();
    assert!(metadata.smooth_end);
    assert_eq!(metadata.nbeats, Some(4));
    assert_eq!(metadata.pulse_length, Some(24_000));
    assert_eq!(
        parse_loop_metadata_xml(&loop_metadata_xml(4, 24_000)).unwrap(),
        metadata
    );
}

#[derive(Clone, Default)]
struct SharedBytes(Arc<Mutex<Cursor<Vec<u8>>>>);

impl SharedBytes {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().get_ref().clone()
    }
}

impl Write for SharedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for SharedBytes {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.0.lock().unwrap().seek(position)
    }
}
