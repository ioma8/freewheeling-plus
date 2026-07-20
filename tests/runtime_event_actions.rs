// Exercise the production module through its public crate boundary.  Including
// its source directly gave this integration test a synthetic crate root and
// could diverge as module dependencies evolved.
use freewheeling_plus::config::FloConfig;
use freewheeling_plus::event::{
    EndRecordEvent, EventType, GoSubEvent, KeyInputEvent, LoopClickedEvent,
};
use freewheeling_plus::native_dsp_graph::{LoopMode, MAX_RUNTIME_LOOPS, RuntimeCommand};
use freewheeling_plus::runtime_event_actions::{
    ApplicationAction, CodecSelection, DispatchError, DispatchOutput, RuntimeEventDispatcher,
};
use std::path::PathBuf;

fn authoritative_config() -> FloConfig {
    let mut config = FloConfig::new();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/fweelin.xml");
    config
        .load_authoritative(&path)
        .expect("load actual data/fweelin.xml");
    config
}

#[test]
fn actual_loop_click_binding_tracks_record_mute_trigger_and_overdub_state() {
    let mut config = authoritative_config();
    let registry = config.binding_registry.clone();
    let dispatcher = RuntimeEventDispatcher::<8>::new();
    let input = LoopClickedEvent::new(true, 1, 7, true);
    let mut loops = [LoopMode::Empty; MAX_RUNTIME_LOOPS];
    for variable in [
        "VAR_keyheld_shift",
        "VAR_keyheld_space",
        "VAR_xferloop",
        "VAR_overdubmode",
    ] {
        config.get_variable_mut(variable).unwrap().set_char(0);
    }

    let batch = dispatcher
        .dispatch(&mut config, &registry, &input, &loops)
        .unwrap();
    assert!(!batch.is_empty());
    assert_eq!(batch.len(), 1);
    assert!(!batch.echo_input());
    assert_eq!(
        batch.iter().collect::<Vec<_>>(),
        vec![&DispatchOutput::Runtime(RuntimeCommand::Record { slot: 7 })]
    );

    loops[7] = LoopMode::Playing;
    let batch = dispatcher
        .dispatch(&mut config, &registry, &input, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Runtime(RuntimeCommand::Mute {
            slot: 7,
            muted: true
        }))
    );

    loops[7] = LoopMode::Muted;
    let batch = dispatcher
        .dispatch(&mut config, &registry, &input, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Runtime(RuntimeCommand::Trigger {
            slot: 7,
            gain: 1.0
        }))
    );

    config
        .get_variable_mut("VAR_overdubmode")
        .unwrap()
        .set_char(1);
    config.set_float_variable("VAR_overdubfeedback", 0.625);
    loops[7] = LoopMode::Playing;
    let batch = dispatcher
        .dispatch(&mut config, &registry, &input, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Runtime(RuntimeCommand::Overdub {
            slot: 7,
            feedback: 0.625,
            gain: 1.0,
        }))
    );
}

#[test]
fn actual_fullscreen_chain_applies_variable_before_application_action() {
    let mut config = authoritative_config();
    let registry = config.binding_registry.clone();
    let loops = [LoopMode::Empty; MAX_RUNTIME_LOOPS];
    let pause = freewheeling_plus::sdlio::get_sdl_key("pause");
    let input = KeyInputEvent::new(true, pause, 0);

    let batch = RuntimeEventDispatcher::<8>::new()
        .dispatch(&mut config, &registry, &input, &loops)
        .unwrap();
    assert_eq!(config.get_int("VAR_videofullscreen"), Some(1));
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Application(
            ApplicationAction::SetFullscreen(true)
        ))
    );
}

#[test]
fn bounded_batch_reports_overflow_instead_of_dropping_continued_actions() {
    let mut config = authoritative_config();
    let registry = config.binding_registry.clone();
    let loops = [LoopMode::Empty; MAX_RUNTIME_LOOPS];
    let pause = freewheeling_plus::sdlio::get_sdl_key("pause");
    let input = KeyInputEvent::new(true, pause, 0);
    let error = RuntimeEventDispatcher::<0>::new()
        .dispatch(&mut config, &registry, &input, &loops)
        .unwrap_err();
    assert_eq!(error, DispatchError::OutputFull { capacity: 0 });
}

#[test]
fn actual_save_loop_and_scene_bindings_carry_configured_codec_and_save_mode() {
    let mut config = FloConfig::new();
    config.set_int_variable("SYSTEM_loopid_lastrecord_0", 12);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/fweelin.xml");
    config.load_authoritative(&path).unwrap();
    config
        .get_variable_mut("VAR_keyheld_shift")
        .unwrap()
        .set_char(0);
    let registry = config.binding_registry.clone();
    let loops = [LoopMode::Empty; MAX_RUNTIME_LOOPS];
    let dispatcher = RuntimeEventDispatcher::<8>::new();

    let save_loop = KeyInputEvent::new(true, freewheeling_plus::sdlio::get_sdl_key("f8"), 0);
    let batch = dispatcher
        .dispatch(&mut config, &registry, &save_loop, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Application(ApplicationAction::SaveLoop {
            loop_id: 12,
            codec: CodecSelection::ConfiguredLoopOutput,
        }))
    );

    let save_scene = KeyInputEvent::new(true, freewheeling_plus::sdlio::get_sdl_key("f7"), 0);
    let batch = dispatcher
        .dispatch(&mut config, &registry, &save_scene, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Application(ApplicationAction::SaveScene {
            force_new: false,
        }))
    );
    config
        .get_variable_mut("VAR_keyheld_shift")
        .unwrap()
        .set_char(1);
    let batch = dispatcher
        .dispatch(&mut config, &registry, &save_scene, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Application(ApplicationAction::SaveScene {
            force_new: true,
        }))
    );
}

#[test]
fn actual_browser_binding_specializes_loop_import_scene_load_and_rename() {
    let mut config = authoritative_config();
    let mut select_registry = config.binding_registry.clone();
    for bucket in &mut select_registry
        .tables
        .get_mut(&EventType::InputKey)
        .unwrap()
        .buckets
    {
        bucket.retain(|binding| binding.output_event == Some(EventType::BrowserSelectItem));
    }
    let loops = [LoopMode::Empty; MAX_RUNTIME_LOOPS];
    let dispatcher = RuntimeEventDispatcher::<8>::new();
    let enter = KeyInputEvent::new(true, freewheeling_plus::sdlio::get_sdl_key("return"), 0);
    config
        .get_variable_mut("VAR_keyheld_ctrl")
        .unwrap()
        .set_char(0);

    config.set_int_variable(
        "VAR_cur_browser",
        config.get_int("DISPLAY_browser_loop").unwrap(),
    );
    let batch = dispatcher
        .dispatch(&mut config, &select_registry, &enter, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Application(
            ApplicationAction::ImportSelectedLoop {
                browser: config.get_int("DISPLAY_browser_loop").unwrap(),
                codec: CodecSelection::DetectFromSelectedFile,
            }
        ))
    );

    config.set_int_variable(
        "VAR_cur_browser",
        config.get_int("DISPLAY_browser_scene").unwrap(),
    );
    let batch = dispatcher
        .dispatch(&mut config, &select_registry, &enter, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Application(
            ApplicationAction::LoadSelectedScene {
                browser: config.get_int("DISPLAY_browser_scene").unwrap(),
            }
        ))
    );

    config
        .get_variable_mut("VAR_keyheld_ctrl")
        .unwrap()
        .set_char(1);
    let mut rename_registry = config.binding_registry.clone();
    for bucket in &mut rename_registry
        .tables
        .get_mut(&EventType::InputKey)
        .unwrap()
        .buckets
    {
        bucket.retain(|binding| binding.output_event == Some(EventType::BrowserRenameItem));
    }
    let batch = dispatcher
        .dispatch(&mut config, &rename_registry, &enter, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Application(
            ApplicationAction::RenameBrowserItem {
                browser: config.get_int("VAR_cur_browser").unwrap(),
            }
        ))
    );
}

#[test]
fn actual_snapshot_subroutine_preserves_action_then_variable_update_order() {
    let mut config = authoritative_config();
    config.set_int_variable("VAR_snapid_last1", 3);
    config.set_int_variable("VAR_snapid_last2", 2);
    let registry = config.binding_registry.clone();
    let loops = [LoopMode::Empty; MAX_RUNTIME_LOOPS];
    let trigger = GoSubEvent::new(101, 9.0, 0.0, 0.0);

    let batch = RuntimeEventDispatcher::<8>::new()
        .dispatch(&mut config, &registry, &trigger, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Application(
            ApplicationAction::TriggerSnapshot { snapshot: 9 }
        ))
    );
    assert_eq!(config.get_int("VAR_snapid_last2"), Some(3));
    assert_eq!(config.get_int("VAR_snapid_last1"), Some(9));
}

#[test]
fn actual_loop_controls_emit_gain_actions_from_shipped_bindings() {
    let mut config = authoritative_config();
    let mut registry = config.binding_registry.clone();
    registry
        .tables
        .get_mut(&EventType::LoopClicked)
        .unwrap()
        .buckets
        .iter_mut()
        .for_each(|bucket| {
            bucket.retain(|binding| {
                matches!(
                    binding.output_event,
                    Some(EventType::SlideLoopAmp)
                        | Some(EventType::AdjustLoopAmp)
                        | Some(EventType::SetTriggerVolume)
                )
            })
        });
    let loops = [LoopMode::Empty; MAX_RUNTIME_LOOPS];
    let input = LoopClickedEvent::new(true, 1, 4, true);
    config
        .get_variable_mut("VAR_keyheld_up")
        .unwrap()
        .set_char(1);
    let batch = RuntimeEventDispatcher::<8>::new()
        .dispatch(&mut config, &registry, &input, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Application(
            ApplicationAction::SlideLoopGain {
                loop_id: 4,
                amount: config.get_float("VAR_slide_speed").unwrap() / 2.0,
            }
        ))
    );

    config
        .get_variable_mut("VAR_keyheld_up")
        .unwrap()
        .set_char(0);
    let input = LoopClickedEvent::new(true, 4, 4, true);
    let batch = RuntimeEventDispatcher::<8>::new()
        .dispatch(&mut config, &registry, &input, &loops)
        .unwrap();
    assert_eq!(
        batch.iter().next(),
        Some(&DispatchOutput::Application(
            ApplicationAction::AdjustLoopGain {
                loop_id: 4,
                factor: 1.0 / config.get_float("VAR_loopamp_adj").unwrap(),
            }
        ))
    );
}

#[test]
fn end_record_is_translated_to_the_existing_runtime_stop_command() {
    let mut config = authoritative_config();
    let registry = config.binding_registry.clone();
    let loops = [LoopMode::Recording; MAX_RUNTIME_LOOPS];
    let batch = RuntimeEventDispatcher::<2>::new()
        .dispatch(&mut config, &registry, &EndRecordEvent::new(false), &loops)
        .unwrap();
    assert_eq!(
        batch.iter().collect::<Vec<_>>(),
        vec![&DispatchOutput::Runtime(RuntimeCommand::StopRecord)]
    );
}
