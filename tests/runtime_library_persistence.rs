use freewheeling_plus::block::{AudioBlock, AudioBlockIterator, Codec};
use freewheeling_plus::core_persistence::{
    AudioLoopSource, LoopMeta, LoopSource, Saveable, Scene, SnapshotLoop, SnapshotMeta,
};
use freewheeling_plus::core_persistence_parse::SceneLoad;
use freewheeling_plus::core_persistence_runtime::{
    OsPersistenceFileSystem, PersistenceEvents, PersistenceRuntime,
};
use freewheeling_plus::file_codecs::{IFileDecoder, SndFileDecoder};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temporary_library() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "freewheeling-runtime-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[derive(Default)]
struct Events {
    loads: Vec<(String, i32, f32)>,
    scenes: Vec<SceneLoad>,
}

impl PersistenceEvents for Events {
    type Event = ();
    fn queue_save(&mut self, _: i32) {}
    fn queue_load(&mut self, filename: String, index: i32, volume: f32) {
        self.loads.push((filename, index, volume));
    }
    fn queue_scene_load(&mut self, scene: SceneLoad) {
        self.scenes.push(scene);
    }
    fn emit(&mut self, _: ()) {}
}

struct LoopFixture {
    hash: Option<[u8; 16]>,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Saveable for LoopFixture {
    fn save_hash(&self) -> Option<[u8; 16]> {
        self.hash
    }
    fn set_save_hash(&mut self, hash: [u8; 16]) {
        self.hash = Some(hash);
    }
}
impl LoopSource for LoopFixture {
    fn audio_bytes(&self) -> &[u8] {
        b"cpp-compatible-quantized-fixture"
    }
    fn object_name(&self) -> Option<&str> {
        Some("verse-one")
    }
    fn nbeats(&self) -> i64 {
        4
    }
    fn pulse_length(&self) -> u32 {
        12_000
    }
}
impl AudioLoopSource for LoopFixture {
    fn sample_rate(&self) -> u32 {
        48_000
    }
    fn left_samples(&self) -> &[f32] {
        &self.left
    }
    fn right_samples(&self) -> Option<&[f32]> {
        Some(&self.right)
    }
}

#[test]
fn encoded_loop_rename_reload_and_scene_workflow_uses_real_files() {
    let library = temporary_library();
    let mut runtime = PersistenceRuntime::new(OsPersistenceFileSystem, Events::default());
    let mut source = LoopFixture {
        hash: None,
        left: (0..40_000)
            .map(|i| (i as f32 * 0.003).sin() * 0.5)
            .collect(),
        right: (0..40_000)
            .map(|i| (i as f32 * 0.005).cos() * 0.25)
            .collect(),
    };
    let (audio, metadata) = runtime
        .save_loop_encoded(&mut source, &library, Codec::Flac)
        .unwrap();
    assert!(audio.starts_with(&library));
    assert_eq!(
        runtime.load_loop_metadata(&metadata).unwrap().nbeats,
        Some(4)
    );

    let mut decoder = SndFileDecoder::new(48_000, Codec::Flac);
    decoder
        .read_from_file(fs::File::open(&audio).unwrap())
        .unwrap();
    let mut decoded = AudioBlock::new(source.left.len());
    decoded.extra = Some(freewheeling_plus::block::ExtraChannel::new(
        source.left.len(),
    ));
    let mut iterator = AudioBlockIterator::new(&mut decoded, 257);
    while decoder.read_samples(&mut iterator, 100_000).unwrap() != 0 {}
    assert!((decoded.sample(12_345).unwrap() - source.left[12_345]).abs() < 2e-6);

    let stub = audio.with_extension("");
    let renamed = runtime
        .rename_saveable(
            &stub,
            library.to_string_lossy().len() + 5,
            Some("chorus-two"),
            &[".flac", ".xml"],
        )
        .unwrap();
    assert!(renamed.with_extension("flac").is_file());
    assert!(renamed.with_extension("xml").is_file());
    assert!(!audio.exists());

    let hash = freewheeling_plus::core_persistence::encode_hash(&source.hash.unwrap());
    let scene = Scene {
        loops: vec![LoopMeta {
            hash,
            loop_id: 7,
            volume: 0.75,
        }],
        snapshots: vec![SnapshotMeta {
            id: 2,
            name: "live & loud".into(),
            loops: vec![SnapshotLoop {
                loop_id: 7,
                status: 1,
                loop_volume: 0.8,
                trigger_volume: 0.6,
            }],
        }],
    };
    let scene_path = library.join("scene-fixture.xml");
    runtime.save_scene(&scene_path, &scene).unwrap();
    runtime
        .load_scene_from_library(&scene_path, 0, Some(&library))
        .unwrap();
    assert_eq!(runtime.events.loads.len(), 1);
    assert_eq!(runtime.events.loads[0].1, 7);
    assert_eq!(runtime.events.scenes[0].snapshots[0].name, "live & loud");
    fs::remove_dir_all(library).unwrap();
}
