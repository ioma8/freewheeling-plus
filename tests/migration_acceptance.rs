use freewheeling_plus::audioio::AudioBackend;
use freewheeling_plus::audioio_platform::AudioIoPlatform;
use freewheeling_plus::block::Codec;
use freewheeling_plus::core::{Core, CoreEvent, CoreServices, LoopSnapshot, Snapshot, StreamState};
use freewheeling_plus::core_persistence::{LoopMeta, Scene, SnapshotMeta, scene_xml};
use freewheeling_plus::core_persistence_parse::{parse_loop_metadata_xml, parse_scene};
use freewheeling_plus::block::{AudioBlock, AudioBlockIterator};
use freewheeling_plus::file_codecs::{
    IFileDecoder, IFileEncoder, SndFileDecoder, SndFileEncoder,
};
use freewheeling_plus::midiio::{MidiBackend, MidiMessage, MidiPortMessage};
use freewheeling_plus::midiio_platform::{MidiPort, PortRegistry, RegistryMidiBackend};
use freewheeling_plus::videoio::VideoMode;
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

struct Lifecycle {
    events: Vec<Option<CoreEvent>>,
    closes: Vec<&'static str>,
}
impl CoreServices for Lifecycle {
    fn setup(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn start_session(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn start_interfaces(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn poll_event(&mut self) -> Result<Option<CoreEvent>, String> {
        // FIFO, like the real adapter: `pop` would reverse the order the test
        // registered its events in.
        if self.events.is_empty() {
            Ok(None)
        } else {
            Ok(self.events.remove(0))
        }
    }
    fn set_streaming(&mut self, _: bool, _: u64) -> Result<(), String> {
        Ok(())
    }
    fn stream_state(&self) -> StreamState {
        StreamState::Stopped
    }
    fn stream_bytes(&self) -> u64 {
        0
    }
    fn close_video(&mut self) {
        self.closes.push("video")
    }
    fn close_sdl(&mut self) {
        self.closes.push("sdl")
    }
    fn close_midi(&mut self) {
        self.closes.push("midi")
    }
    fn close_audio(&mut self) {
        self.closes.push("audio")
    }
    fn shutdown(&mut self) {
        self.closes.push("shutdown")
    }
    fn rollback_setup(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
        vec![]
    }
    fn restore_snapshot(&mut self, _: &Snapshot) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn persistence_scene_and_loop_metadata_round_trip() {
    let scene = Scene {
        loops: vec![LoopMeta {
            hash: "AB".into(),
            loop_id: 3,
            volume: 0.5,
        }],
        snapshots: vec![SnapshotMeta {
            id: 2,
            name: "a & b".into(),
            loops: vec![],
        }],
    };
    assert_eq!(parse_scene(&scene_xml(&scene)).unwrap(), scene);
    let meta =
        parse_loop_metadata_xml("<loop version=\"1\" nbeats=\"4\" pulselen=\"12\"/>").unwrap();
    assert!(meta.smooth_end);
    assert_eq!(meta.nbeats, Some(4));
    assert_eq!(meta.pulse_length, Some(12));
}

#[test]
fn application_lifecycle_reaches_ordered_idempotent_shutdown() {
    let mut core = Core::new(Lifecycle {
        events: vec![Some(CoreEvent::ExitSession)],
        closes: vec![],
    });
    core.setup().unwrap();
    core.go().unwrap();
    assert!(!core.is_running());
    assert_eq!(
        core.services().closes,
        vec!["video", "sdl", "midi", "audio", "shutdown"]
    );
    core.shutdown();
    assert_eq!(core.services().closes.len(), 5);
}

/// A `Write + Seek` sink over a shared buffer.
///
/// `seek` really moves the cursor: `prepare_file_for_closing` seeks back to
/// patch the RIFF chunk sizes, and a no-op seek would append those bytes at the
/// end instead (producing a malformed file that still starts with `RIFF`).
#[derive(Clone, Default)]
struct SharedWriter(Arc<Mutex<Cursor<Vec<u8>>>>);
impl Write for SharedWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}
impl Seek for SharedWriter {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.lock().unwrap().seek(pos)
    }
}

#[test]
fn codec_and_platform_state_work_without_hardware() {
    let samples = [0.0_f32, 0.25, -0.25];
    let bytes = Arc::new(Mutex::new(Cursor::new(Vec::new())));
    let mut enc = SndFileEncoder::new(48_000, false, Codec::Wav).unwrap();
    enc.setup_file_for_writing(SharedWriter(bytes.clone()))
        .unwrap();
    enc.write_samples_to_disk(&samples, None).unwrap();
    enc.prepare_file_for_closing().unwrap();
    let encoded = bytes.lock().unwrap().get_ref().clone();
    assert!(encoded.starts_with(b"RIFF"));
    // Decode the bytes back: the magic alone would not catch a wrong sample
    // format, channel count or chunk length.
    let mut decoder = SndFileDecoder::new(48_000, Codec::Wav);
    decoder.read_from_file(Cursor::new(encoded)).unwrap();
    let mut block = AudioBlock::new(samples.len());
    let mut iterator = AudioBlockIterator::new(&mut block, 3);
    assert_eq!(
        decoder.read_samples(&mut iterator, samples.len()).unwrap(),
        samples.len()
    );
    assert!(
        block
            .samples
            .iter()
            .zip(samples.iter())
            .all(|(decoded, wanted)| (decoded - wanted).abs() < 1e-6),
        "decoded samples differ from the written samples: {:?}",
        block.samples
    );

    let registry = PortRegistry::new([MidiPort {
        name: "virtual".into(),
        input: true,
        output: true,
    }]);
    let mut midi = RegistryMidiBackend::new(registry.clone());
    midi.open(1, 1).unwrap();
    midi.send(MidiPortMessage {
        port: 0,
        message: MidiMessage::Start,
    })
    .unwrap();
    assert_eq!(registry.take_output().unwrap().message, MidiMessage::Start);
    midi.close();

    let mut audio = AudioIoPlatform::new(48_000, 4);
    audio.open("virtual").unwrap();
    audio.close();
    assert_eq!(VideoMode { fullscreen: true, windowed_size: (640, 480) }.windowed_size, (640, 480));
}
