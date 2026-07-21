use freewheeling_plus::datatypes::{Range, UserVariable};
use freewheeling_plus::event::{Event, EventParameter, EventType, INTERFACEID};

#[test]
fn test_midi_events_report_expected_types() {
    let cc = Event::MIDIControllerInput {
        outport: 1,
        channel: 0,
        ctrl: 7,
        val: 100,
        echo: false,
    };
    let key = Event::MIDIKeyInput {
        outport: 2,
        channel: 0,
        notenum: 64,
        vel: 90,
        down: true,
        echo: false,
    };
    let program = Event::MIDIProgramChangeInput {
        outport: 1,
        channel: 6,
        val: 42,
        echo: false,
    };
    let pressure = Event::MIDIChannelPressureInput {
        outport: 1,
        channel: 6,
        val: 99,
        echo: true,
    };
    let bend = Event::MIDIPitchBendInput {
        outport: 3,
        channel: 0,
        val: 1234,
        echo: false,
    };

    assert_eq!(cc.get_type(), EventType::InputMIDIController);
    assert_eq!(key.get_type(), EventType::InputMIDIKey);
    assert_eq!(program.get_type(), EventType::InputMIDIProgramChange);
    assert_eq!(pressure.get_type(), EventType::InputMIDIChannelPressure);
    assert_eq!(bend.get_type(), EventType::InputMIDIPitchBend);
}

#[test]
fn test_event_clone_preserves_payload() {
    let ev = Event::FluidSynthEnable { enable: true };
    let cloned = ev.clone();
    assert_eq!(cloned, ev);
    assert_eq!(cloned.get_type(), EventType::FluidSynthEnable);
}

#[test]
fn test_browser_navigation_events_carry_indices() {
    let move_rel = Event::BrowserMoveToItem {
        browserid: 9,
        adjust: -1,
        jump_adjust: 4,
    };
    let move_abs = Event::BrowserMoveToItemAbsolute {
        browserid: 9,
        index: 12,
    };
    let bank_rel = Event::PatchBrowserMoveToBank { direction: 1 };
    let bank_abs = Event::PatchBrowserMoveToBankByIndex { index: 3 };

    let (browserid, adjust, jump_adjust) = match &move_rel {
        Event::BrowserMoveToItem {
            browserid,
            adjust,
            jump_adjust,
        } => (*browserid, *adjust, *jump_adjust),
        _ => unreachable!(),
    };
    assert_eq!(browserid, 9);
    assert_eq!(adjust, -1);
    assert_eq!(jump_adjust, 4);

    let (browserid2, index2) = match &move_abs {
        Event::BrowserMoveToItemAbsolute { browserid, index } => (*browserid, *index),
        _ => unreachable!(),
    };
    assert_eq!(browserid2, 9);
    assert_eq!(index2, 12);

    let direction = match &bank_rel {
        Event::PatchBrowserMoveToBank { direction } => *direction,
        _ => unreachable!(),
    };
    assert_eq!(direction, 1);

    let index3 = match &bank_abs {
        Event::PatchBrowserMoveToBankByIndex { index } => *index,
        _ => unreachable!(),
    };
    assert_eq!(index3, 3);
}

#[test]
fn test_browser_selection_events_preserve_browser_id() {
    let select = Event::BrowserSelectItem { browserid: 2 };
    let rename = Event::BrowserRenameItem { browserid: 3 };
    let browsed = Event::BrowserItemBrowsed { browserid: 4 };

    let b1 = match &select {
        Event::BrowserSelectItem { browserid } => *browserid,
        _ => unreachable!(),
    };
    assert_eq!(b1, 2);

    let b2 = match &rename {
        Event::BrowserRenameItem { browserid } => *browserid,
        _ => unreachable!(),
    };
    assert_eq!(b2, 3);

    let b3 = match &browsed {
        Event::BrowserItemBrowsed { browserid } => *browserid,
        _ => unreachable!(),
    };
    assert_eq!(b3, 4);
    assert_eq!(browsed.get_type(), EventType::BrowserItemBrowsed);
}

#[test]
fn test_set_midi_tuning_event_payload() {
    let tuning = Event::SetMidiTuning { tuning: 12.5 };
    assert_eq!(tuning.get_type(), EventType::SetMidiTuning);
    if let Event::SetMidiTuning { tuning: val } = &tuning {
        assert!((val - 12.5).abs() < 0.0001);
    }
}

#[test]
fn test_session_events_have_expected_types_and_payloads() {
    let start = Event::StartSession;
    let iface = Event::StartInterface { interfaceid: 7 };
    let exit = Event::ExitSession;

    assert_eq!(start.get_type(), EventType::StartSession);
    assert_eq!(iface.get_type(), EventType::StartInterface);
    let ifid = match &iface {
        Event::StartInterface { interfaceid } => *interfaceid,
        _ => unreachable!(),
    };
    assert_eq!(ifid, 7);
    assert_eq!(exit.get_type(), EventType::ExitSession);
}

#[test]
fn test_volume_and_input_control_events_preserve_payloads() {
    let slide_master = Event::SlideMasterInVolume { slide: 0.25 };
    let slide_in = Event::SlideInVolume {
        input: 2,
        slide: -0.5,
    };
    let set_master = Event::SetMasterInVolume {
        vol: 0.8,
        fadervol: 0.7,
    };
    let set_in = Event::SetInVolume {
        input: 3,
        vol: 0.4,
        fadervol: 0.2,
    };
    let toggle = Event::ToggleInputRecord { input: 1 };

    let sm_slide = match &slide_master {
        Event::SlideMasterInVolume { slide } => *slide,
        _ => unreachable!(),
    };
    assert!((sm_slide - 0.25).abs() < 0.0001);

    let (si_input, si_slide) = match &slide_in {
        Event::SlideInVolume { input, slide } => (*input, *slide),
        _ => unreachable!(),
    };
    assert_eq!(si_input, 2);
    assert!((si_slide + 0.5).abs() < 0.0001);

    let (sm_vol, sm_fv) = match &set_master {
        Event::SetMasterInVolume { vol, fadervol } => (*vol, *fadervol),
        _ => unreachable!(),
    };
    assert!((sm_vol - 0.8).abs() < 0.0001);
    assert!((sm_fv - 0.7).abs() < 0.0001);

    let si_input2 = match &set_in {
        Event::SetInVolume { input, .. } => *input,
        _ => unreachable!(),
    };
    assert_eq!(si_input2, 3);

    let toggle_input = match &toggle {
        Event::ToggleInputRecord { input } => *input,
        _ => unreachable!(),
    };
    assert_eq!(toggle_input, 1);
}

#[test]
fn test_midi_echo_and_trigger_events_preserve_payloads() {
    let echo_port = Event::SetMidiEchoPort { echoport: 2 };
    let echo_channel = Event::SetMidiEchoChannel { echochannel: -1 };
    let transpose = Event::AdjustMidiTranspose { adjust: 12 };
    let trigger = Event::SetTriggerVolume {
        index: 5,
        vol: 0.9,
    };

    let ep = match &echo_port {
        Event::SetMidiEchoPort { echoport } => *echoport,
        _ => unreachable!(),
    };
    assert_eq!(ep, 2);

    let ec = match &echo_channel {
        Event::SetMidiEchoChannel { echochannel } => *echochannel,
        _ => unreachable!(),
    };
    assert_eq!(ec, -1);

    let ta = match &transpose {
        Event::AdjustMidiTranspose { adjust } => *adjust,
        _ => unreachable!(),
    };
    assert_eq!(ta, 12);

    let (ti, tv) = match &trigger {
        Event::SetTriggerVolume { index, vol } => (*index, *vol),
        _ => unreachable!(),
    };
    assert_eq!(ti, 5);
    assert!((tv - 0.9).abs() < 0.0001);
}

#[test]
fn test_loop_management_events_preserve_payloads() {
    let move_loop = Event::MoveLoop {
        oldloopid: 5,
        newloopid: 8,
    };
    let erase_loop = Event::EraseLoop { index: 11 };
    let erase_all = Event::EraseAllLoops;
    let save_loop = Event::SaveLoop { index: 7 };
    let placement = Event::SetDefaultLoopPlacement {
        looprange: Range::new(20, 29),
    };

    let (oldid, newid) = match &move_loop {
        Event::MoveLoop {
            oldloopid,
            newloopid,
        } => (*oldloopid, *newloopid),
        _ => unreachable!(),
    };
    assert_eq!(oldid, 5);
    assert_eq!(newid, 8);

    let el = match &erase_loop {
        Event::EraseLoop { index } => *index,
        _ => unreachable!(),
    };
    assert_eq!(el, 11);

    assert_eq!(erase_all.get_type(), EventType::EraseAllLoops);

    let sl = match &save_loop {
        Event::SaveLoop { index } => *index,
        _ => unreachable!(),
    };
    assert_eq!(sl, 7);

    let lr = match &placement {
        Event::SetDefaultLoopPlacement { looprange } => looprange.clone(),
        _ => unreachable!(),
    };
    assert_eq!(lr, Range::new(20, 29));
}

#[test]
fn test_pulse_and_sync_events_preserve_payloads() {
    let select = Event::SelectPulse { pulse: 1 };
    let delete = Event::DeletePulse { pulse: 2 };
    let tap = Event::TapPulse {
        pulse: 3,
        newlen: true,
    };
    let metro = Event::SwitchMetronome {
        pulse: 4,
        metronome: false,
    };
    let sync_type = Event::SetSyncType { stype: true };
    let sync_speed = Event::SetSyncSpeed { sspd: 24 };
    let midi_sync = Event::SetMidiSync { midisync: 1 };
    let pulse_sync = Event::PulseSync;

    let sp = match &select {
        Event::SelectPulse { pulse } => *pulse,
        _ => unreachable!(),
    };
    assert_eq!(sp, 1);

    let dp = match &delete {
        Event::DeletePulse { pulse } => *pulse,
        _ => unreachable!(),
    };
    assert_eq!(dp, 2);

    let (tp, tn) = match &tap {
        Event::TapPulse { pulse, newlen } => (*pulse, *newlen),
        _ => unreachable!(),
    };
    assert_eq!(tp, 3);
    assert!(tn);

    let (mp, mm) = match &metro {
        Event::SwitchMetronome { pulse, metronome } => (*pulse, *metronome),
        _ => unreachable!(),
    };
    assert_eq!(mp, 4);
    assert!(!mm);

    let st = match &sync_type {
        Event::SetSyncType { stype } => *stype,
        _ => unreachable!(),
    };
    assert!(st);

    let ss = match &sync_speed {
        Event::SetSyncSpeed { sspd } => *sspd,
        _ => unreachable!(),
    };
    assert_eq!(ss, 24);

    let ms = match &midi_sync {
        Event::SetMidiSync { midisync } => *midisync,
        _ => unreachable!(),
    };
    assert_eq!(ms, 1);

    assert_eq!(pulse_sync.get_type(), EventType::PulseSync);
}

#[test]
fn test_event_parameter_metadata_for_core_events() {
    let midi_key = Event::MIDIKeyInput {
        outport: 2,
        channel: 0,
        notenum: 64,
        vel: 90,
        down: true,
        echo: false,
    };
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

    let ctrl = Event::MIDIControllerInput {
        outport: 1,
        channel: 0,
        ctrl: 7,
        val: 100,
        echo: false,
    };
    assert_eq!(
        ctrl.get_param(2),
        Some(EventParameter::with_max_index(
            "controlnum",
            freewheeling_plus::datatypes::CoreDataType::Int,
            127
        ))
    );

    let program = Event::MIDIProgramChangeInput {
        outport: 1,
        channel: 6,
        val: 42,
        echo: false,
    };
    assert_eq!(
        program.get_param(2),
        Some(EventParameter::new(
            "programval",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let pressure = Event::MIDIChannelPressureInput {
        outport: 1,
        channel: 6,
        val: 99,
        echo: true,
    };
    assert_eq!(
        pressure.get_param(2),
        Some(EventParameter::new(
            "pressureval",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let bend = Event::MIDIPitchBendInput {
        outport: 3,
        channel: 0,
        val: 1234,
        echo: false,
    };
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
    let switch_iface = Event::VideoSwitchInterface { interfaceid: 2 };
    let show_display = Event::VideoShowDisplay {
        interfaceid: 2,
        displayid: 5,
        show: true,
    };
    let help = Event::VideoShowHelp { page: 3 };
    let fullscreen = Event::VideoFullScreen { fullscreen: true };
    let debug = Event::ShowDebugInfo { show: false };
    let show_loop = Event::VideoShowLoop {
        interfaceid: 1,
        layoutid: 9,
        loopid: Range::new(10, 13),
    };
    let snapshot = Event::VideoShowSnapshotPage {
        interfaceid: 1,
        displayid: 4,
        page: -1,
    };
    let bank = Event::VideoShowParamSetBank {
        interfaceid: 1,
        displayid: 7,
        bank: 1,
    };
    let page = Event::VideoShowParamSetPage {
        interfaceid: 1,
        displayid: 7,
        page: -1,
    };

    let si = match &switch_iface {
        Event::VideoSwitchInterface { interfaceid } => *interfaceid,
        _ => unreachable!(),
    };
    assert_eq!(si, 2);

    let (did, showing) = match &show_display {
        Event::VideoShowDisplay {
            displayid, show, ..
        } => (*displayid, *show),
        _ => unreachable!(),
    };
    assert_eq!(did, 5);
    assert!(showing);

    let hp = match &help {
        Event::VideoShowHelp { page } => *page,
        _ => unreachable!(),
    };
    assert_eq!(hp, 3);

    let fs = match &fullscreen {
        Event::VideoFullScreen { fullscreen } => *fullscreen,
        _ => unreachable!(),
    };
    assert!(fs);

    let ds = match &debug {
        Event::ShowDebugInfo { show } => *show,
        _ => unreachable!(),
    };
    assert!(!ds);

    let (lid, lr) = match &show_loop {
        Event::VideoShowLoop {
            layoutid, loopid, ..
        } => (*layoutid, loopid.clone()),
        _ => unreachable!(),
    };
    assert_eq!(lid, 9);
    assert_eq!(lr, Range::new(10, 13));

    let sp = match &snapshot {
        Event::VideoShowSnapshotPage { page, .. } => *page,
        _ => unreachable!(),
    };
    assert_eq!(sp, -1);

    let bk = match &bank {
        Event::VideoShowParamSetBank { bank, .. } => *bank,
        _ => unreachable!(),
    };
    assert_eq!(bk, 1);

    let pp = match &page {
        Event::VideoShowParamSetPage { page, .. } => *page,
        _ => unreachable!(),
    };
    assert_eq!(pp, -1);
}

#[test]
fn test_gosub_paramset_and_mixer_events_preserve_payloads() {
    let go_sub = Event::GoSub {
        sub: 100,
        param1: 1.5,
        param2: 2.5,
        param3: 3.5,
    };
    let loop_clicked = Event::LoopClicked {
        down: true,
        button: 2,
        loopid: 7,
        in_layout: false,
    };
    let abs_idx = Event::ParamSetGetAbsoluteParamIdx {
        interfaceid: 1,
        displayid: 5,
        paramidx: 3,
        absidx_name: Some("VAR_idx".to_string()),
    };
    let get_param = Event::ParamSetGetParam {
        interfaceid: 1,
        displayid: 5,
        paramidx: 4,
        var_name: Some("VAR_value".to_string()),
    };
    let set_param = Event::ParamSetSetParam {
        interfaceid: 1,
        displayid: 5,
        paramidx: 4,
        value: 0.75,
    };
    let mut fadervol = UserVariable::new();
    fadervol.set_float(0.5);
    let fader = Event::LogFaderVolToLinear {
        var_name: Some("VAR_out".to_string()),
        fadervol: fadervol.clone(),
        scale: 16384.0,
    };
    let mixer = Event::ALSAMixerControlSet {
        hwid: 0,
        numid: 5,
        val1: 1,
        val2: 2,
        val3: 3,
        val4: 4,
    };

    let (gs_sub, gs_param2) = match &go_sub {
        Event::GoSub {
            sub, param2, ..
        } => (*sub, *param2),
        _ => unreachable!(),
    };
    assert_eq!(gs_sub, 100);
    assert_eq!(gs_param2, 2.5);

    let (lc_loopid, lc_in_layout) = match &loop_clicked {
        Event::LoopClicked {
            loopid,
            in_layout,
            ..
        } => (*loopid, *in_layout),
        _ => unreachable!(),
    };
    assert_eq!(lc_loopid, 7);
    assert!(!lc_in_layout);

    let ai_name = match &abs_idx {
        Event::ParamSetGetAbsoluteParamIdx {
            absidx_name, ..
        } => absidx_name.as_deref(),
        _ => unreachable!(),
    };
    assert_eq!(ai_name, Some("VAR_idx"));

    let gp_name = match &get_param {
        Event::ParamSetGetParam { var_name, .. } => var_name.as_deref(),
        _ => unreachable!(),
    };
    assert_eq!(gp_name, Some("VAR_value"));

    let sp_val = match &set_param {
        Event::ParamSetSetParam { value, .. } => *value,
        _ => unreachable!(),
    };
    assert_eq!(sp_val, 0.75);

    let fv_name = match &fader {
        Event::LogFaderVolToLinear { var_name, .. } => var_name.as_deref(),
        _ => unreachable!(),
    };
    assert_eq!(fv_name, Some("VAR_out"));

    let fv_val = match &fader {
        Event::LogFaderVolToLinear { fadervol, .. } => fadervol.as_f32(),
        _ => unreachable!(),
    };
    assert_eq!(fv_val, fadervol.as_f32());

    let (mx_numid, mx_val4) = match &mixer {
        Event::ALSAMixerControlSet {
            numid, val4, ..
        } => (*numid, *val4),
        _ => unreachable!(),
    };
    assert_eq!(mx_numid, 5);
    assert_eq!(mx_val4, 4);
}

#[test]
fn test_snapshot_scene_and_runtime_events_preserve_payloads() {
    let rename_loop = Event::RenameLoop {
        loopid: 9,
        in_layout: true,
    };
    let erase_selected = Event::EraseSelectedLoops { setid: 3 };
    let toggle_disk = Event::ToggleDiskOutput;
    let autosave = Event::SetAutoLoopSaving { save: true };
    let save_new = Event::SaveNewScene;
    let save_current = Event::SaveCurrentScene;
    let load_loop = Event::SetLoadLoopId { index: 14 };
    let create = Event::CreateSnapshot { snapid: 2 };
    let swap = Event::SwapSnapshots {
        snapid1: 2,
        snapid2: 5,
    };
    let rename_snap = Event::RenameSnapshot { snapid: 4 };
    let trigger = Event::TriggerSnapshot { snapid: 7 };
    let transmit = Event::TransmitPlayingLoopsToDAW;

    let (rl_id, rl_layout) = match &rename_loop {
        Event::RenameLoop {
            loopid,
            in_layout,
        } => (*loopid, *in_layout),
        _ => unreachable!(),
    };
    assert_eq!(rl_id, 9);
    assert!(rl_layout);

    let es_setid = match &erase_selected {
        Event::EraseSelectedLoops { setid } => *setid,
        _ => unreachable!(),
    };
    assert_eq!(es_setid, 3);

    let autosave_val = match &autosave {
        Event::SetAutoLoopSaving { save } => *save,
        _ => unreachable!(),
    };
    assert!(autosave_val);

    let ll_idx = match &load_loop {
        Event::SetLoadLoopId { index } => *index,
        _ => unreachable!(),
    };
    assert_eq!(ll_idx, 14);

    let cs_id = match &create {
        Event::CreateSnapshot { snapid } => *snapid,
        _ => unreachable!(),
    };
    assert_eq!(cs_id, 2);

    let sw_id2 = match &swap {
        Event::SwapSnapshots { snapid2, .. } => *snapid2,
        _ => unreachable!(),
    };
    assert_eq!(sw_id2, 5);

    let rs_id = match &rename_snap {
        Event::RenameSnapshot { snapid } => *snapid,
        _ => unreachable!(),
    };
    assert_eq!(rs_id, 4);

    let ts_id = match &trigger {
        Event::TriggerSnapshot { snapid } => *snapid,
        _ => unreachable!(),
    };
    assert_eq!(ts_id, 7);

    assert_eq!(toggle_disk.get_type(), EventType::ToggleDiskOutput);
    assert_eq!(save_new.get_type(), EventType::SaveNewScene);
    assert_eq!(save_current.get_type(), EventType::SaveCurrentScene);
    assert_eq!(transmit.get_type(), EventType::TransmitPlayingLoopsToDAW);
}

#[test]
fn test_event_parameter_metadata_for_browser_and_volume_events() {
    let move_rel = Event::BrowserMoveToItem {
        browserid: 9,
        adjust: -1,
        jump_adjust: 4,
    };
    assert_eq!(move_rel.get_num_params(), 3);
    assert_eq!(
        move_rel.get_param(2),
        Some(EventParameter::new(
            "jumpadjust",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let move_abs = Event::BrowserMoveToItemAbsolute {
        browserid: 9,
        index: 12,
    };
    assert_eq!(
        move_abs.get_param(1),
        Some(EventParameter::new(
            "idx",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let iface = Event::StartInterface { interfaceid: 7 };
    assert_eq!(
        iface.get_param(0),
        Some(EventParameter::new(
            INTERFACEID,
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let slide_in = Event::SlideInVolume {
        input: 2,
        slide: -0.5,
    };
    assert_eq!(
        slide_in.get_param(1),
        Some(EventParameter::new(
            "slide",
            freewheeling_plus::datatypes::CoreDataType::Float
        ))
    );

    let set_in = Event::SetInVolume {
        input: 3,
        vol: 0.4,
        fadervol: 0.2,
    };
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
    let key = Event::KeyInput {
        down: true,
        keysym: 32,
        unicode: 65,
    };
    assert_eq!(key.get_num_params(), 3);
    assert_eq!(
        key.get_param(1),
        Some(EventParameter::with_max_index(
            "key",
            freewheeling_plus::datatypes::CoreDataType::Int,
            512
        ))
    );

    let joy = Event::JoystickButtonInput {
        down: true,
        button: 4,
        joystick: 1,
    };
    assert_eq!(
        joy.get_param(2),
        Some(EventParameter::new(
            "joystick",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let mouse_button = Event::MouseButtonInput {
        down: false,
        button: 2,
        x: 100,
        y: 200,
    };
    assert_eq!(
        mouse_button.get_param(3),
        Some(EventParameter::new(
            "y",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );

    let mouse_motion = Event::MouseMotionInput { x: 11, y: 22 };
    assert_eq!(mouse_motion.get_num_params(), 2);
    assert_eq!(
        mouse_motion.get_param(0),
        Some(EventParameter::new(
            "x",
            freewheeling_plus::datatypes::CoreDataType::Int
        ))
    );
}
