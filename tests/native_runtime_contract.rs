mod support;

use freewheeling_plus::application_services::Components;
use freewheeling_plus::audioio::{AudioCallback, AudioProcessor, JackPosition};
use freewheeling_plus::core::{CoreEvent, LoopStatus, StreamState};
use freewheeling_plus::fluidsynth::FluidSynthBackend;
use freewheeling_plus::native_dsp_graph::{
    LoopMode, RuntimeAudioProcessor, RuntimeCommand, RuntimeControls, RuntimeSnapshot,
    RuntimeStatus, runtime_audio_processor_with_backend,
};
use freewheeling_plus::native_startup::{NativeStartupServices, StartupPhase};
use freewheeling_plus::production_app::ProductionApp;
use freewheeling_plus::realtime_guard::{
    CallbackCountingAllocator, RealtimeMetrics, callback_allocations, reset_violation_counters,
};
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use support::*;

#[global_allocator]
static ALLOCATOR: CallbackCountingAllocator = CallbackCountingAllocator;

fn temp_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "freewheeling-native-contract-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("resources")).unwrap();
    fs::create_dir_all(root.join("support")).unwrap();
    fs::write(root.join("resources/fweelin.xml"), b"<config/>").unwrap();
    root
}

fn boxed_dsp(
    calls: Arc<Mutex<Vec<SynthCall>>>,
    max_callback_frames: usize,
) -> (Box<RuntimeAudioProcessor<FakeFluid>>, Box<RuntimeControls>) {
    let (dsp, controls) =
        runtime_audio_processor_with_backend(FakeFluid { calls }, 48_000, 16, max_callback_frames);
    (Box::new(dsp), Box::new(controls))
}

fn next_snapshot(controls: &mut RuntimeControls) -> Box<RuntimeSnapshot> {
    loop {
        match controls.try_status().expect("snapshot status expected") {
            RuntimeStatus::Snapshot(snapshot) => return Box::new(snapshot),
            RuntimeStatus::LoopCompleted { slot: 0 } => {}
            other => panic!("unexpected status: {other:?}"),
        }
    }
}

#[test]
fn input_and_output_slides_are_callback_safe_and_metronome_uses_selected_pulse() {
    let (mut dsp, mut controls) = boxed_dsp(Arc::new(Mutex::new(Vec::new())), 16);
    controls
        .try_command(RuntimeCommand::SetInputMonitor(0.5))
        .unwrap();
    controls
        .try_command(RuntimeCommand::AdjustInputMonitor(0.25))
        .unwrap();
    controls
        .try_command(RuntimeCommand::SetMasterGain(0.5))
        .unwrap();
    controls
        .try_command(RuntimeCommand::AdjustMasterGain(0.25))
        .unwrap();
    assert_eq!(
        process(&mut dsp, &[1.0], &[1.0]),
        [vec![0.5625], vec![0.5625]]
    );

    controls
        .try_command(RuntimeCommand::SetPulse { frames: 4 })
        .unwrap();
    controls
        .try_command(RuntimeCommand::SetMetronome {
            enabled: true,
            gain: 0.2,
        })
        .unwrap();
    // C++ Pulse starts its metronome offset after the hit buffer, so turning
    // it on waits for the first actual pulse downbeat rather than clicking
    // immediately.
    let until_downbeat = process(&mut dsp, &[0.0; 4], &[0.0; 4]);
    assert_eq!(&until_downbeat[0][..3], &[0.0; 3]);
    assert!(until_downbeat[0][3].abs() > 0.000_01);
    assert_eq!(until_downbeat[0], until_downbeat[1]);
    let hit = process(&mut dsp, &[0.0], &[0.0]);
    assert!(hit[0][0].abs() > 0.000_01);
    assert_eq!(hit[0], hit[1]);
}

fn process<B: FluidSynthBackend>(
    processor: &mut RuntimeAudioProcessor<B>,
    left: &[f32],
    right: &[f32],
) -> [Vec<f32>; 2] {
    let mut out_l = vec![0.0; left.len()];
    let mut out_r = vec![0.0; right.len()];
    let mut callback = AudioCallback {
        inputs: [left, right],
        outputs: [&mut out_l, &mut out_r],
        nframes: left.len() as u32,
        position: JackPosition::default(),
        transport_rolling: false,
    };
    processor.process(&mut callback);
    [out_l, out_r]
}

fn assert_samples_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        // C++ AutoLimitProcessor increments its release envelope once per
        // rendered sample. The production graph intentionally preserves that
        // tiny (< 0.01%) change rather than treating the limiter as inert.
        assert!(
            (actual - expected).abs() < 0.000_02,
            "actual {actual} differs from expected {expected}"
        );
    }
}

#[test]
fn production_app_runs_every_real_phase_handles_quit_and_rolls_back_in_reverse() {
    let root = temp_root("lifecycle");
    let startup_log = Rc::new(RefCell::new(Vec::new()));
    let component_state = Rc::new(RefCell::new(ComponentState::default()));
    let startup = NativeStartupServices::new(
        paths(root.clone()),
        FakeStartup {
            log: startup_log.clone(),
            fail_at: None,
        },
    );
    let native = FakeNative::new(
        component_state.clone(),
        [CoreEvent::ExitSession],
        root.join("stream.bin"),
    );
    let mut app = ProductionApp::new(FakeConfig::default(), startup, native, 2, 8);
    app.run().unwrap();

    let log = startup_log.borrow();
    let expected_starts: Vec<_> = PHASES.iter().map(|p| format!("start:{p}")).collect();
    assert_eq!(&log[..PHASES.len()], expected_starts);
    // C++ `FweelinStartupGuard::Release()` commits a successful startup and
    // clears its rollback stack. Component shutdown below owns teardown.
    assert_eq!(log.len(), PHASES.len());
    assert_eq!(
        component_state.borrow().log,
        [
            "session:start",
            "interfaces:start",
            "close:video",
            "close:input",
            "close:midi",
            "close:audio",
            "close:graph"
        ]
    );
}

#[test]
fn failed_native_phase_rolls_back_only_completed_phases() {
    let root = temp_root("rollback");
    let log = Rc::new(RefCell::new(Vec::new()));
    let startup = NativeStartupServices::new(
        paths(root.clone()),
        FakeStartup {
            log: log.clone(),
            fail_at: Some(StartupPhase::SynthAndBuffers),
        },
    );
    let native = FakeNative::new(
        Rc::new(RefCell::new(ComponentState::default())),
        [],
        root.join("stream.bin"),
    );
    let mut app = ProductionApp::new(FakeConfig::default(), startup, native, 0, 0);
    let error = app.run().unwrap_err();
    assert!(
        error.contains("SynthAndBuffers: injected native failure"),
        "{error}"
    );
    let entries = log.borrow();
    let completed = &PHASES[..11];
    let expected: Vec<_> = completed
        .iter()
        .rev()
        .map(|p| format!("rollback:{p}"))
        .collect();
    assert_eq!(&entries[12..], expected);
}

#[test]
fn dsp_workflow_records_overdubs_triggers_mutes_erases_and_routes_fluid_commands() {
    let synth_calls = Arc::new(Mutex::new(Vec::new()));
    let (mut dsp, mut controls) = boxed_dsp(synth_calls.clone(), 16);
    controls
        .try_command(RuntimeCommand::Record { slot: 0 })
        .unwrap();
    // A fresh C++ RecordProcessor writes PCM but clears its own output.
    // Input monitoring, when enabled, is a separate processor path.
    assert_samples_close(&process(&mut dsp, &[0.2, 0.4], &[0.1, 0.3])[0], &[0.0, 0.0]);
    controls.try_command(RuntimeCommand::StopRecord).unwrap();
    controls
        .try_command(RuntimeCommand::Trigger { slot: 0, gain: 0.5 })
        .unwrap();
    assert_samples_close(&process(&mut dsp, &[0.0; 2], &[0.0; 2])[0], &[0.1, 0.2]);
    controls
        .try_command(RuntimeCommand::Overdub {
            slot: 0,
            feedback: 0.5,
            gain: 1.0,
        })
        .unwrap();
    // RecordProcessor renders the old fragment before it writes the
    // feedback-plus-input replacement.
    assert_samples_close(&process(&mut dsp, &[0.1, 0.1], &[0.0; 2])[0], &[0.2, 0.4]);
    controls.try_command(RuntimeCommand::StopRecord).unwrap();
    controls
        .try_command(RuntimeCommand::Mute {
            slot: 0,
            muted: true,
        })
        .unwrap();
    assert_eq!(process(&mut dsp, &[0.0; 2], &[0.0; 2])[0], [0.0; 2]);
    for command in [
        RuntimeCommand::SynthNote {
            note: 60,
            velocity: 100,
        },
        RuntimeCommand::SynthController {
            channel: 2,
            control: 74,
            value: 90,
        },
        RuntimeCommand::SynthPitchBend {
            channel: 3,
            value: 12_000,
        },
        RuntimeCommand::SynthPatch {
            channel: 4,
            soundfont_id: 7,
            bank: 8,
            program: 9,
        },
        RuntimeCommand::SynthTuning { cents: -7.5 },
        RuntimeCommand::SynthOff,
    ] {
        controls.try_command(command).unwrap();
    }
    process(&mut dsp, &[], &[]);
    controls
        .try_command(RuntimeCommand::Erase { slot: 0 })
        .unwrap();
    controls
        .try_command(RuntimeCommand::RequestSnapshot)
        .unwrap();
    process(&mut dsp, &[], &[]);
    let snapshot = next_snapshot(&mut controls);
    assert_eq!(snapshot.loops[0].mode, LoopMode::Empty);
    controls.try_command(RuntimeCommand::Shutdown).unwrap();
    process(&mut dsp, &[1.0], &[1.0]);
    assert_eq!(controls.try_status(), Some(RuntimeStatus::ShutdownComplete));
    let calls = synth_calls.lock().unwrap();
    assert!(calls.contains(&SynthCall::Note(0, 60, 100)));
    // C++ FluidSynthProcessor routes inbound controller and bend events to
    // FloConfig's one configured synth channel, not the incoming MIDI
    // channel; bend is centred at the FluidSynth API boundary.
    assert!(calls.contains(&SynthCall::Controller(0, 74, 90)));
    assert!(calls.contains(&SynthCall::Bend(0, 12_000 + 8_192)));
    assert!(calls.contains(&SynthCall::Patch(4, 7, 8, 9)));
    assert!(calls.contains(&SynthCall::Tuning(-7.5)));
    assert!(calls.contains(&SynthCall::Controller(0, 123, 0)));
    assert!(calls.contains(&SynthCall::Shutdown));
}

#[test]
fn loop_gain_and_move_are_callback_safe_and_preserve_recording_identity() {
    let (mut dsp, mut controls) = boxed_dsp(Arc::new(Mutex::new(Vec::new())), 16);
    controls
        .try_command(RuntimeCommand::Record { slot: 0 })
        .unwrap();
    process(&mut dsp, &[0.25, 0.5], &[0.0, 0.0]);
    controls
        .try_command(RuntimeCommand::MoveLoop { from: 0, to: 1 })
        .unwrap();
    process(&mut dsp, &[0.75], &[0.0]);
    controls
        .try_command(RuntimeCommand::SetLoopGain { slot: 1, gain: 0.5 })
        .unwrap();
    controls
        .try_command(RuntimeCommand::AdjustLoopGain {
            slot: 1,
            factor: 2.0,
        })
        .unwrap();
    controls
        .try_command(RuntimeCommand::RequestSnapshot)
        .unwrap();
    process(&mut dsp, &[], &[]);

    let snapshot = next_snapshot(&mut controls);
    assert_eq!(snapshot.recording_slot, 1);
    assert_eq!(snapshot.loops[0].mode, LoopMode::Empty);
    assert_eq!(snapshot.loops[1].mode, LoopMode::Recording);
    assert_eq!(snapshot.loops[1].position, 0);
    assert_eq!(snapshot.loops[1].gain, 1.0);
}

#[test]
fn set_trigger_gain_scales_current_playback_without_retriggering() {
    let (mut dsp, mut controls) = boxed_dsp(Arc::new(Mutex::new(Vec::new())), 16);
    controls
        .try_command(RuntimeCommand::Record { slot: 0 })
        .unwrap();
    process(&mut dsp, &[0.25, 0.5, 0.75], &[0.0; 3]);
    controls.try_command(RuntimeCommand::StopRecord).unwrap();
    controls
        .try_command(RuntimeCommand::Trigger { slot: 0, gain: 1.0 })
        .unwrap();
    assert_samples_close(&process(&mut dsp, &[0.0], &[0.0])[0], &[0.25]);

    controls
        .try_command(RuntimeCommand::SetTriggerGain { slot: 0, gain: 0.5 })
        .unwrap();
    assert_samples_close(&process(&mut dsp, &[0.0], &[0.0])[0], &[0.25]);

    controls
        .try_command(RuntimeCommand::RequestSnapshot)
        .unwrap();
    process(&mut dsp, &[], &[]);
    let snapshot = next_snapshot(&mut controls);
    assert_eq!(snapshot.loops[0].mode, LoopMode::Playing);
    assert_eq!(snapshot.loops[0].position, 2);
    assert_eq!(snapshot.loops[0].trigger_gain, 0.5);
}

#[test]
fn changing_selected_trigger_volume_preserves_the_active_playback_cursor() {
    let (mut dsp, mut controls) = boxed_dsp(Arc::new(Mutex::new(Vec::new())), 16);
    controls
        .try_command(RuntimeCommand::Record { slot: 0 })
        .unwrap();
    process(&mut dsp, &[0.1, 0.2, 0.3, 0.4], &[0.0; 4]);
    controls.try_command(RuntimeCommand::StopRecord).unwrap();
    controls
        .try_command(RuntimeCommand::Trigger { slot: 0, gain: 1.0 })
        .unwrap();
    assert_samples_close(&process(&mut dsp, &[0.0], &[0.0])[0], &[0.1]);

    // The selected-volume action is a gain update, not a second Trigger.
    controls
        .try_command(RuntimeCommand::SetTriggerGain { slot: 0, gain: 0.5 })
        .unwrap();
    assert_samples_close(&process(&mut dsp, &[0.0], &[0.0])[0], &[0.2 * 0.5]);

    controls
        .try_command(RuntimeCommand::RequestSnapshot)
        .unwrap();
    process(&mut dsp, &[], &[]);
    let snapshot = next_snapshot(&mut controls);
    assert_eq!(snapshot.loops[0].mode, LoopMode::Playing);
    assert_eq!(snapshot.loops[0].position, 2);
    assert_eq!(snapshot.loops[0].trigger_gain, 0.5);
}

#[test]
fn export_rejects_mutation_of_source_or_move_destination() {
    let (mut dsp, mut controls) = boxed_dsp(Arc::new(Mutex::new(Vec::new())), 1);
    controls
        .try_command(RuntimeCommand::Record { slot: 0 })
        .unwrap();
    process(&mut dsp, &[0.25, 0.5], &[0.0, 0.0]);
    controls.try_command(RuntimeCommand::StopRecord).unwrap();
    process(&mut dsp, &[], &[]);
    let _replacement = controls.try_request_loop_export(0).unwrap();
    controls
        .try_command(RuntimeCommand::SetLoopGain {
            slot: 0,
            gain: 0.25,
        })
        .unwrap();
    controls
        .try_command(RuntimeCommand::MoveLoop { from: 0, to: 1 })
        .unwrap();
    process(&mut dsp, &[], &[]);
    let mut rejected = [false; 2];
    for _ in 0..3 {
        match controls.try_status() {
            Some(RuntimeStatus::CommandRejected(RuntimeCommand::SetLoopGain {
                slot: 0, ..
            })) => rejected[0] = true,
            Some(RuntimeStatus::CommandRejected(RuntimeCommand::MoveLoop { from: 0, to: 1 })) => {
                rejected[1] = true
            }
            Some(_) | None => {}
        }
    }
    assert_eq!(rejected, [true, true]);
}

#[test]
fn gain_and_move_commands_allocate_nothing_in_the_callback() {
    let (mut dsp, mut controls) = boxed_dsp(Arc::new(Mutex::new(Vec::new())), 4);
    controls
        .try_command(RuntimeCommand::Record { slot: 0 })
        .unwrap();
    process(&mut dsp, &[0.25], &[0.0]);
    controls
        .try_command(RuntimeCommand::SetLoopGain { slot: 0, gain: 0.5 })
        .unwrap();
    controls
        .try_command(RuntimeCommand::SetTriggerGain {
            slot: 0,
            gain: 0.75,
        })
        .unwrap();
    controls
        .try_command(RuntimeCommand::AdjustLoopGain {
            slot: 0,
            factor: 2.0,
        })
        .unwrap();
    controls
        .try_command(RuntimeCommand::MoveLoop { from: 0, to: 1 })
        .unwrap();
    let realtime = RealtimeMetrics::new(48_000, 4).unwrap();
    let input_left = [0.25, 0.5];
    let input_right = [0.0, 0.0];
    let mut output_left = [0.0; 2];
    let mut output_right = [0.0; 2];
    let mut callback = AudioCallback {
        inputs: [&input_left, &input_right],
        outputs: [&mut output_left, &mut output_right],
        nframes: 2,
        position: JackPosition::default(),
        transport_rolling: false,
    };
    reset_violation_counters();
    {
        let _guard = realtime.enter_callback();
        dsp.process(&mut callback);
    }
    assert_eq!(callback_allocations(), 0);
}

#[test]
fn fake_native_stream_round_trip_device_restart_snapshot_and_clean_shutdown() {
    let root = temp_root("recovery");
    let state = Rc::new(RefCell::new(ComponentState {
        loops: vec![playing_loop()],
        ..ComponentState::default()
    }));
    let startup = NativeStartupServices::new(
        paths(root.clone()),
        FakeStartup {
            log: Rc::new(RefCell::new(Vec::new())),
            fail_at: None,
        },
    );
    let native = FakeNative::new(state.clone(), [], root.join("stream.bin"));
    let mut app = ProductionApp::new(FakeConfig::default(), startup, native, 0, 0);
    app.app_mut().setup().unwrap();
    app.app_mut().components_mut().start_session().unwrap();
    app.app_mut().components_mut().start_interfaces().unwrap();
    app.app_mut().toggle_disk_output().unwrap();
    assert_eq!(app.app().stream_stats().0, StreamState::Writing);
    assert_eq!(
        app.app().components().adapter().reload_stream(),
        b"FWEELIN-FAKE-STREAM:0\n"
    );
    app.app_mut().toggle_disk_output().unwrap();
    app.app_mut().create_snapshot(3, "live scene");
    state.borrow_mut().loops[0].status = LoopStatus::Off;
    app.app_mut().trigger_snapshot(3).unwrap();
    assert_eq!(
        state.borrow().restored.as_ref().unwrap().loops[0].status,
        LoopStatus::Playing
    );
    app.app_mut()
        .components_mut()
        .adapter_mut()
        .lose_device_and_restart();
    assert!(!state.borrow().device_lost);
    app.app_mut().shutdown();
    let log = &state.borrow().log;
    let recovery = [
        "device:lost",
        "audio:quiesce",
        "audio:close",
        "audio:open",
        "audio:activate",
    ];
    assert!(log.windows(recovery.len()).any(|window| window == recovery));
    assert!(log.ends_with(&[
        "close:video".into(),
        "close:input".into(),
        "close:midi".into(),
        "close:audio".into(),
        "close:graph".into()
    ]));
}
