use freewheeling_plus::datatypes::{Range, UserVariable};
use freewheeling_plus::event::{
    ALSAMixerControlSetEvent, AdjustMidiTransposeEvent, BrowserItemBrowsedEvent,
    BrowserMoveToItemAbsoluteEvent, BrowserMoveToItemEvent, BrowserRenameItemEvent,
    BrowserSelectItemEvent, CreateSnapshotEvent, DeletePulseEvent, EraseAllLoopsEvent,
    EraseLoopEvent, EraseSelectedLoopsEvent, Event, EventParameter, EventType, ExitSessionEvent,
    FluidSynthEnableEvent, GoSubEvent, JoystickButtonInputEvent, KeyInputEvent,
    LogFaderVolToLinearEvent, LoopClickedEvent, MIDIChannelPressureInputEvent,
    MIDIControllerInputEvent, MIDIKeyInputEvent, MIDIPitchBendInputEvent,
    MIDIProgramChangeInputEvent, MouseButtonInputEvent, MouseMotionInputEvent, MoveLoopEvent,
    ParamSetGetAbsoluteParamIdxEvent, ParamSetGetParamEvent, ParamSetSetParamEvent,
    PatchBrowserMoveToBankByIndexEvent, PatchBrowserMoveToBankEvent, PulseSyncEvent,
    RenameLoopEvent, RenameSnapshotEvent, SaveCurrentSceneEvent, SaveLoopEvent, SaveNewSceneEvent,
    SelectPulseEvent, SetAutoLoopSavingEvent, SetDefaultLoopPlacementEvent, SetInVolumeEvent,
    SetLoadLoopIdEvent, SetMasterInVolumeEvent, SetMidiEchoChannelEvent, SetMidiEchoPortEvent,
    SetMidiSyncEvent, SetMidiTuningEvent, SetSyncSpeedEvent, SetSyncTypeEvent,
    SetTriggerVolumeEvent, ShowDebugInfoEvent, SlideInVolumeEvent, SlideMasterInVolumeEvent,
    StartInterfaceEvent, StartSessionEvent, SwapSnapshotsEvent, SwitchMetronomeEvent,
    TapPulseEvent, ToggleDiskOutputEvent, ToggleInputRecordEvent, TransmitPlayingLoopsToDAWEvent,
    TriggerSnapshotEvent, VideoFullScreenEvent, VideoShowDisplayEvent, VideoShowHelpEvent,
    VideoShowLoopEvent, VideoShowParamSetBankEvent, VideoShowParamSetPageEvent,
    VideoShowSnapshotPageEvent, VideoSwitchInterfaceEvent,
};

#[test]
fn test_midi_events_report_expected_types() {
    let cc = MIDIControllerInputEvent::new(1, 7, 100);
    let key = MIDIKeyInputEvent::new(2, 64, 90, true);
    let program = MIDIProgramChangeInputEvent::new(1, 6, 42, false);
    let pressure = MIDIChannelPressureInputEvent::new(1, 6, 99, true);
    let bend = MIDIPitchBendInputEvent::new(3, 1234);

    assert_eq!(cc.get_type(), EventType::InputMIDIController);
    assert_eq!(key.get_type(), EventType::InputMIDIKey);
    assert_eq!(program.get_type(), EventType::InputMIDIProgramChange);
    assert_eq!(pressure.get_type(), EventType::InputMIDIChannelPressure);
    assert_eq!(bend.get_type(), EventType::InputMIDIPitchBend);
}

#[test]
fn test_event_clone_box_preserves_payload() {
    let ev: Box<dyn Event> = Box::new(FluidSynthEnableEvent::new(true));
    let cloned = ev.clone_box();
    let typed = cloned
        .as_any()
        .downcast_ref::<FluidSynthEnableEvent>()
        .unwrap();
    assert!(typed.enable);
    assert_eq!(typed.get_type(), EventType::FluidSynthEnable);
}

#[test]
fn test_browser_navigation_events_carry_indices() {
    let move_rel = BrowserMoveToItemEvent::new(9, -1, 4);
    let move_abs = BrowserMoveToItemAbsoluteEvent::new(9, 12);
    let bank_rel = PatchBrowserMoveToBankEvent::new(1);
    let bank_abs = PatchBrowserMoveToBankByIndexEvent::new(3);

    assert_eq!(move_rel.browserid, 9);
    assert_eq!(move_rel.adjust, -1);
    assert_eq!(move_rel.jump_adjust, 4);
    assert_eq!(move_abs.browserid, 9);
    assert_eq!(move_abs.index, 12);
    assert_eq!(bank_rel.direction, 1);
    assert_eq!(bank_abs.index, 3);
}

#[test]
fn test_browser_selection_events_preserve_browser_id() {
    let select = BrowserSelectItemEvent::new(2);
    let rename = BrowserRenameItemEvent::new(3);
    let browsed = BrowserItemBrowsedEvent::new(4);

    assert_eq!(select.browserid, 2);
    assert_eq!(rename.browserid, 3);
    assert_eq!(browsed.browserid, 4);
    assert_eq!(browsed.get_type(), EventType::BrowserItemBrowsed);
}

#[test]
fn test_set_midi_tuning_event_payload() {
    let tuning = SetMidiTuningEvent::new(12.5);
    assert_eq!(tuning.get_type(), EventType::SetMidiTuning);
    assert!((tuning.tuning - 12.5).abs() < 0.0001);
}

#[test]
fn test_session_events_have_expected_types_and_payloads() {
    let start = StartSessionEvent::new();
    let iface = StartInterfaceEvent::new(7);
    let exit = ExitSessionEvent::new();

    assert_eq!(start.get_type(), EventType::StartSession);
    assert_eq!(iface.get_type(), EventType::StartInterface);
    assert_eq!(iface.interfaceid, 7);
    assert_eq!(exit.get_type(), EventType::ExitSession);
}

#[test]
fn test_volume_and_input_control_events_preserve_payloads() {
    let slide_master = SlideMasterInVolumeEvent::new(0.25);
    let slide_in = SlideInVolumeEvent::new(2, -0.5);
    let set_master = SetMasterInVolumeEvent::new(0.8, 0.7);
    let set_in = SetInVolumeEvent::new(3, 0.4, 0.2);
    let toggle = ToggleInputRecordEvent::new(1);

    assert!((slide_master.slide - 0.25).abs() < 0.0001);
    assert_eq!(slide_in.input, 2);
    assert!((slide_in.slide + 0.5).abs() < 0.0001);
    assert!((set_master.vol - 0.8).abs() < 0.0001);
    assert!((set_master.fadervol - 0.7).abs() < 0.0001);
    assert_eq!(set_in.input, 3);
    assert_eq!(toggle.input, 1);
}

#[test]
fn test_midi_echo_and_trigger_events_preserve_payloads() {
    let echo_port = SetMidiEchoPortEvent::new(2);
    let echo_channel = SetMidiEchoChannelEvent::new(-1);
    let transpose = AdjustMidiTransposeEvent::new(12);
    let trigger = SetTriggerVolumeEvent::new(5, 0.9);

    assert_eq!(echo_port.echoport, 2);
    assert_eq!(echo_channel.echochannel, -1);
    assert_eq!(transpose.adjust, 12);
    assert_eq!(trigger.index, 5);
    assert!((trigger.vol - 0.9).abs() < 0.0001);
}

#[test]
fn test_loop_management_events_preserve_payloads() {
    let move_loop = MoveLoopEvent::new(5, 8);
    let erase_loop = EraseLoopEvent::new(11);
    let erase_all = EraseAllLoopsEvent::new();
    let save_loop = SaveLoopEvent::new(7);
    let placement = SetDefaultLoopPlacementEvent::new(Range::new(20, 29));

    assert_eq!(move_loop.oldloopid, 5);
    assert_eq!(move_loop.newloopid, 8);
    assert_eq!(erase_loop.index, 11);
    assert_eq!(erase_all.get_type(), EventType::EraseAllLoops);
    assert_eq!(save_loop.index, 7);
    assert_eq!(placement.looprange, Range::new(20, 29));
}

#[test]
fn test_pulse_and_sync_events_preserve_payloads() {
    let select = SelectPulseEvent::new(1);
    let delete = DeletePulseEvent::new(2);
    let tap = TapPulseEvent::new(3, true);
    let metro = SwitchMetronomeEvent::new(4, false);
    let sync_type = SetSyncTypeEvent::new(true);
    let sync_speed = SetSyncSpeedEvent::new(24);
    let midi_sync = SetMidiSyncEvent::new(1);
    let pulse_sync = PulseSyncEvent::new();

    assert_eq!(select.pulse, 1);
    assert_eq!(delete.pulse, 2);
    assert_eq!(tap.pulse, 3);
    assert!(tap.newlen);
    assert_eq!(metro.pulse, 4);
    assert!(!metro.metronome);
    assert!(sync_type.stype);
    assert_eq!(sync_speed.sspd, 24);
    assert_eq!(midi_sync.midisync, 1);
    assert_eq!(pulse_sync.get_type(), EventType::PulseSync);
}

#[test]
fn test_event_parameter_metadata_for_core_events() {
    let midi_key = MIDIKeyInputEvent::new(2, 64, 90, true);
    // The C++ event table includes output routing ahead of the MIDI payload.
    assert_eq!(midi_key.get_num_params(), 6);
    assert_eq!(
        midi_key.get_param(0),
        Some(EventParameter::new(
            "outport",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );
    assert_eq!(
        midi_key.get_param(1),
        Some(EventParameter::new(
            "keydown",
            freewheeling_plus::datatypes::CoreDataType::Char
        ))
    );
    assert_eq!(
        midi_key.get_param(2),
        Some(EventParameter::with_max_index(
            "midichannel",
            freewheeling_plus::datatypes::CoreDataType::Int,
            16
        ))
    );
    assert_eq!(midi_key.get_param(99), None);

    let ctrl = MIDIControllerInputEvent::new(1, 7, 100);
    assert_eq!(
        ctrl.get_param(2),
        Some(EventParameter::with_max_index(
            "controlnum",
            freewheeling_plus::datatypes::CoreDataType::Int,
            127
        ))
    );

    let program = MIDIProgramChangeInputEvent::new(1, 6, 42, false);
    assert_eq!(
        program.get_param(2),
        Some(EventParameter::new(
            "programval",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let pressure = MIDIChannelPressureInputEvent::new(1, 6, 99, true);
    assert_eq!(
        pressure.get_param(2),
        Some(EventParameter::new(
            "pressureval",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let bend = MIDIPitchBendInputEvent::new(3, 1234);
    assert_eq!(
        bend.get_param(2),
        Some(EventParameter::new(
            "pitchval",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );
}

#[test]
fn test_display_control_events_preserve_payloads() {
    let switch_iface = VideoSwitchInterfaceEvent::new(2);
    let show_display = VideoShowDisplayEvent::new(2, 5, true);
    let help = VideoShowHelpEvent::new(3);
    let fullscreen = VideoFullScreenEvent::new(true);
    let debug = ShowDebugInfoEvent::new(false);
    let show_loop = VideoShowLoopEvent::new(1, 9, Range::new(10, 13));
    let snapshot = VideoShowSnapshotPageEvent::new(1, 4, -1);
    let bank = VideoShowParamSetBankEvent::new(1, 7, 1);
    let page = VideoShowParamSetPageEvent::new(1, 7, -1);

    assert_eq!(switch_iface.interfaceid, 2);
    assert_eq!(show_display.displayid, 5);
    assert!(show_display.show);
    assert_eq!(help.page, 3);
    assert!(fullscreen.fullscreen);
    assert!(!debug.show);
    assert_eq!(show_loop.layoutid, 9);
    assert_eq!(show_loop.loopid, Range::new(10, 13));
    assert_eq!(snapshot.page, -1);
    assert_eq!(bank.bank, 1);
    assert_eq!(page.page, -1);
}

#[test]
fn test_gosub_paramset_and_mixer_events_preserve_payloads() {
    let go_sub = GoSubEvent::new(100, 1.5, 2.5, 3.5);
    let loop_clicked = LoopClickedEvent::new(true, 2, 7, false);
    let abs_idx = ParamSetGetAbsoluteParamIdxEvent::new(1, 5, 3, Some("VAR_idx".to_string()));
    let get_param = ParamSetGetParamEvent::new(1, 5, 4, Some("VAR_value".to_string()));
    let set_param = ParamSetSetParamEvent::new(1, 5, 4, 0.75);
    let mut fadervol = UserVariable::new();
    fadervol.set_float(0.5);
    let fader =
        LogFaderVolToLinearEvent::new(Some("VAR_out".to_string()), fadervol.clone(), 16384.0);
    let mixer = ALSAMixerControlSetEvent::new(0, 5, 1, 2, 3, 4);

    assert_eq!(go_sub.sub, 100);
    assert_eq!(go_sub.param2, 2.5);
    assert_eq!(loop_clicked.loopid, 7);
    assert!(!loop_clicked.in_layout);
    assert_eq!(abs_idx.absidx_name.as_deref(), Some("VAR_idx"));
    assert_eq!(get_param.var_name.as_deref(), Some("VAR_value"));
    assert_eq!(set_param.value, 0.75);
    assert_eq!(fader.var_name.as_deref(), Some("VAR_out"));
    assert_eq!(fader.fadervol.as_f32(), fadervol.as_f32());
    assert_eq!(mixer.numid, 5);
    assert_eq!(mixer.val4, 4);
}

#[test]
fn test_snapshot_scene_and_runtime_events_preserve_payloads() {
    let rename_loop = RenameLoopEvent::new(9, true);
    let erase_selected = EraseSelectedLoopsEvent::new(3);
    let toggle_disk = ToggleDiskOutputEvent::new();
    let autosave = SetAutoLoopSavingEvent::new(true);
    let save_new = SaveNewSceneEvent::new();
    let save_current = SaveCurrentSceneEvent::new();
    let load_loop = SetLoadLoopIdEvent::new(14);
    let create = CreateSnapshotEvent::new(2);
    let swap = SwapSnapshotsEvent::new(2, 5);
    let rename_snap = RenameSnapshotEvent::new(4);
    let trigger = TriggerSnapshotEvent::new(7);
    let transmit = TransmitPlayingLoopsToDAWEvent::new();

    assert_eq!(rename_loop.loopid, 9);
    assert!(rename_loop.in_layout);
    assert_eq!(erase_selected.setid, 3);
    assert!(autosave.save);
    assert_eq!(load_loop.index, 14);
    assert_eq!(create.snapid, 2);
    assert_eq!(swap.snapid2, 5);
    assert_eq!(rename_snap.snapid, 4);
    assert_eq!(trigger.snapid, 7);
    assert_eq!(toggle_disk.get_type(), EventType::ToggleDiskOutput);
    assert_eq!(save_new.get_type(), EventType::SaveNewScene);
    assert_eq!(save_current.get_type(), EventType::SaveCurrentScene);
    assert_eq!(transmit.get_type(), EventType::TransmitPlayingLoopsToDAW);
}

#[test]
fn test_event_parameter_metadata_for_browser_and_volume_events() {
    let move_rel = BrowserMoveToItemEvent::new(9, -1, 4);
    assert_eq!(move_rel.get_num_params(), 3);
    assert_eq!(
        move_rel.get_param(2),
        Some(EventParameter::new(
            "jumpadjust",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let move_abs = BrowserMoveToItemAbsoluteEvent::new(9, 12);
    assert_eq!(
        move_abs.get_param(1),
        Some(EventParameter::new(
            "idx",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let iface = StartInterfaceEvent::new(7);
    assert_eq!(
        iface.get_param(0),
        Some(EventParameter::new(
            freewheeling_plus::event::INTERFACEID,
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let slide_in = SlideInVolumeEvent::new(2, -0.5);
    assert_eq!(
        slide_in.get_param(1),
        Some(EventParameter::new(
            "slide",
            freewheeling_plus::datatypes::CoreDataType::Float
        ))
    );

    let set_in = SetInVolumeEvent::new(3, 0.4, 0.2);
    assert_eq!(
        set_in.get_param(2),
        Some(EventParameter::new(
            "fadervol",
            freewheeling_plus::datatypes::CoreDataType::Float
        ))
    );
}

#[test]
fn test_event_parameter_metadata_for_input_events() {
    let key = KeyInputEvent::new(true, 32, 65);
    assert_eq!(key.get_num_params(), 3);
    assert_eq!(
        key.get_param(1),
        Some(EventParameter::with_max_index(
            "key",
            freewheeling_plus::datatypes::CoreDataType::Int,
            512
        ))
    );

    let joy = JoystickButtonInputEvent::new(true, 4, 1);
    assert_eq!(
        joy.get_param(2),
        Some(EventParameter::new(
            "joystick",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let mouse_button = MouseButtonInputEvent::new(false, 2, 100, 200);
    assert_eq!(
        mouse_button.get_param(3),
        Some(EventParameter::new(
            "y",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let mouse_motion = MouseMotionInputEvent::new(11, 22);
    assert_eq!(mouse_motion.get_num_params(), 2);
    assert_eq!(
        mouse_motion.get_param(0),
        Some(EventParameter::new(
            "x",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );
}
