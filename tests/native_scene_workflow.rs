use freewheeling_plus::audioio::{AudioCallback, AudioProcessor, JackPosition};
use freewheeling_plus::config::FloConfig;
use freewheeling_plus::core_persistence::{LoopMeta, Scene, SnapshotLoop, SnapshotMeta};
use freewheeling_plus::core_persistence_parse::SceneLoad;
use freewheeling_plus::core_persistence_runtime::{
    OsPersistenceFileSystem, PersistenceEvents, PersistenceRuntime,
};
use freewheeling_plus::fluidsynth::FluidSynthBackend;
use freewheeling_plus::native_dsp_graph::{
    LoopMode, RuntimeStatus, runtime_audio_processor_with_backend,
};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_root(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "freewheeling-native-scene-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[derive(Default)]
struct Events {
    saves: Vec<i32>,
    loads: Vec<(String, i32, f32)>,
    scenes: Vec<SceneLoad>,
}

impl PersistenceEvents for Events {
    type Event = ();

    fn queue_save(&mut self, index: i32) {
        self.saves.push(index);
    }
    fn queue_load(&mut self, filename: String, index: i32, volume: f32) {
        self.loads.push((filename, index, volume));
    }
    fn queue_scene_load(&mut self, scene: SceneLoad) {
        self.scenes.push(scene);
    }
    fn emit(&mut self, _: ()) {}
}

fn scene() -> Scene {
    Scene {
        loops: vec![
            LoopMeta {
                hash: "00112233445566778899AABBCCDDEEFF".into(),
                loop_id: 2,
                volume: 0.75,
            },
            LoopMeta {
                hash: "FFEEDDCCBBAA99887766554433221100".into(),
                loop_id: 5,
                volume: 0.5,
            },
        ],
        snapshots: vec![SnapshotMeta {
            id: 4,
            name: "bridge & return".into(),
            loops: vec![
                SnapshotLoop {
                    loop_id: 2,
                    status: 1,
                    loop_volume: 0.8,
                    trigger_volume: 0.6,
                },
                SnapshotLoop {
                    loop_id: 5,
                    status: 0,
                    loop_volume: 0.4,
                    trigger_volume: 0.9,
                },
            ],
        }],
    }
}

#[test]
fn native_scene_save_backup_import_queue_and_autosave_contract() {
    let root = temp_root("persistence");
    let library = root.join("library");
    fs::create_dir_all(&library).unwrap();
    let path = library.join("scene-live.xml");
    let mut runtime = PersistenceRuntime::new(OsPersistenceFileSystem, Events::default());

    runtime.save_scene(&path, &scene()).unwrap();
    let old = fs::read(&path).unwrap();
    let backup = FloConfig::next_backup_path(&path);
    FloConfig::copy_file_contents(&path, &backup).unwrap();
    runtime.queue_save(4);
    runtime.queue_save(4);
    runtime
        .load_scene_from_library(&path, 99, Some(&library))
        .unwrap();

    assert_eq!(fs::read(&backup).unwrap(), old);
    assert_eq!(runtime.events.saves, vec![4, 4]);
    assert_eq!(
        runtime.events.loads,
        vec![
            (
                library
                    .join("loop-00112233445566778899AABBCCDDEEFF")
                    .display()
                    .to_string(),
                2,
                0.75
            ),
            (
                library
                    .join("loop-FFEEDDCCBBAA99887766554433221100")
                    .display()
                    .to_string(),
                5,
                0.5
            ),
        ]
    );
    assert_eq!(
        runtime.events.scenes,
        vec![
            freewheeling_plus::core_persistence_parse::parse_scene_xml(
                &fs::read_to_string(&path).unwrap(),
                99
            )
            .unwrap()
        ]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_scene_save_is_create_new_and_preserves_existing_state() {
    let root = temp_root("save");
    let path = root.join("scene.xml");
    let runtime = PersistenceRuntime::new(OsPersistenceFileSystem, Events::default());
    runtime.save_scene(&path, &scene()).unwrap();
    let first = fs::read(&path).unwrap();
    let error = runtime.save_scene(&path, &scene()).unwrap_err();
    assert!(error.contains("could not save scene"));
    assert_eq!(fs::read(&path).unwrap(), first);
    fs::remove_dir_all(root).unwrap();
}

#[derive(Default)]
struct SilentSynth;
impl FluidSynthBackend for SilentSynth {
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
    }
    fn controller(&mut self, _: u8, _: u8, _: u8) {}
    fn pitch_bend(&mut self, _: u8, _: i32) {}
    fn note_on(&mut self, _: u8, _: i32, _: u8) {}
    fn note_off(&mut self, _: u8, _: i32) {}
    fn program_select(&mut self, _: u8, _: i32, _: i32, _: i32) {}
    fn set_tuning(&mut self, _: f64) {}
    fn shutdown(&mut self) {}
}

#[test]
fn imported_scene_loop_transfer_reaches_dsp_and_publishes_state() {
    let (mut processor, mut controls) =
        runtime_audio_processor_with_backend(SilentSynth, 48_000, 4, 2);
    let handle = controls.try_acquire_transfer().unwrap();
    let left = [0.1, -0.2, 0.3, -0.4];
    let right = [-0.4, 0.3, -0.2, 0.1];
    controls.write_transfer(handle, &left, &right).unwrap();
    controls
        .try_import_loop(2, handle, 123, LoopMode::Playing, 0.75)
        .unwrap();
    let input = [0.0; 4];
    let mut output_left = [0.0; 4];
    let mut output_right = [0.0; 4];
    processor.process(&mut AudioCallback {
        inputs: [&input, &input],
        outputs: [&mut output_left, &mut output_right],
        nframes: 4,
        position: JackPosition::default(),
        transport_rolling: false,
    });
    assert_eq!(
        controls.try_status(),
        Some(RuntimeStatus::LoopImported { slot: 2, handle })
    );
    controls.release_transfer(handle).unwrap();
}
