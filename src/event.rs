/*
   Event system – complete port of fweelin_event.h/cc.
*/

use crate::datatypes::{CoreDataType, Range, UserVariable};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

// ============================================================
// Constants
// ============================================================

pub const FWEELIN_OUTNAME_LEN: usize = 1024;
pub const MAX_MIDI_CHANNELS: usize = 16;
pub const MAX_MIDI_CONTROLLERS: usize = 127;
pub const MAX_MIDI_NOTES: usize = 127;
pub const MAX_MIDI_PORTS: usize = 4;
pub const INTERFACEID: &str = "interfaceid";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventParameter {
    pub name: &'static str,
    pub dtype: CoreDataType,
    pub max_index: i32,
}

/// Entry in the event type table.
///
/// The C++ implementation stores non-owning pointers to the allocator and
/// prototype.  The Rust port has no corresponding allocator/prototype
/// objects, so these are retained as opaque pointers while preserving the
/// original nullable, non-owning representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventTypeTable {
    pub name: Option<&'static str>,
    pub pretype: *mut c_void,
    pub proto: *mut c_void,
    pub paramidx: i32,
    pub slowdelivery: i8,
}

impl EventTypeTable {
    pub const fn new(
        name: Option<&'static str>,
        pretype: *mut c_void,
        proto: *mut c_void,
        paramidx: i32,
        slowdelivery: i8,
    ) -> Self {
        Self {
            name,
            pretype,
            proto,
            paramidx,
            slowdelivery,
        }
    }

    pub const fn with_name(name: &'static str) -> Self {
        Self::new(
            Some(name),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            -1,
            0,
        )
    }
}

impl Default for EventTypeTable {
    fn default() -> Self {
        Self::new(None, std::ptr::null_mut(), std::ptr::null_mut(), -1, 0)
    }
}

#[cfg(test)]
mod event_type_table_tests {
    use super::EventTypeTable;

    #[test]
    fn defaults_match_cpp_constructor() {
        let table = EventTypeTable::default();
        assert_eq!(table.name, None);
        assert!(table.pretype.is_null());
        assert!(table.proto.is_null());
        assert_eq!(table.paramidx, -1);
        assert_eq!(table.slowdelivery, 0);
    }
}

impl EventParameter {
    pub const fn new(name: &'static str, dtype: CoreDataType) -> Self {
        Self {
            name,
            dtype,
            max_index: -1,
        }
    }

    pub const fn with_max_index(name: &'static str, dtype: CoreDataType, max_index: i32) -> Self {
        Self {
            name,
            dtype,
            max_index,
        }
    }
}

const KEY_INPUT_PARAMS: [EventParameter; 3] = [
    EventParameter::new("keydown", CoreDataType::Char),
    EventParameter::with_max_index("key", CoreDataType::Int, 512),
    EventParameter::new("unicode", CoreDataType::Int),
];

const JOYSTICK_BUTTON_INPUT_PARAMS: [EventParameter; 3] = [
    EventParameter::new("down", CoreDataType::Char),
    EventParameter::new("button", CoreDataType::Int),
    EventParameter::new("joystick", CoreDataType::Int),
];

const MOUSE_BUTTON_INPUT_PARAMS: [EventParameter; 4] = [
    EventParameter::new("down", CoreDataType::Char),
    EventParameter::new("button", CoreDataType::Int),
    EventParameter::new("x", CoreDataType::Int),
    EventParameter::new("y", CoreDataType::Int),
];

const MOUSE_MOTION_INPUT_PARAMS: [EventParameter; 2] = [
    EventParameter::new("x", CoreDataType::Int),
    EventParameter::new("y", CoreDataType::Int),
];

const MIDI_CONTROLLER_INPUT_PARAMS: [EventParameter; 5] = [
    EventParameter::new("outport", CoreDataType::Int),
    EventParameter::with_max_index("midichannel", CoreDataType::Int, MAX_MIDI_CHANNELS as i32),
    EventParameter::with_max_index("controlnum", CoreDataType::Int, MAX_MIDI_CONTROLLERS as i32),
    EventParameter::new("controlval", CoreDataType::Int),
    EventParameter::new("routethroughpatch", CoreDataType::Char),
];

const MIDI_CHANNEL_PRESSURE_INPUT_PARAMS: [EventParameter; 4] = [
    EventParameter::new("outport", CoreDataType::Int),
    EventParameter::with_max_index("midichannel", CoreDataType::Int, MAX_MIDI_CHANNELS as i32),
    EventParameter::new("pressureval", CoreDataType::Int),
    EventParameter::new("routethroughpatch", CoreDataType::Char),
];

const MIDI_PROGRAM_CHANGE_INPUT_PARAMS: [EventParameter; 4] = [
    EventParameter::new("outport", CoreDataType::Int),
    EventParameter::with_max_index("midichannel", CoreDataType::Int, MAX_MIDI_CHANNELS as i32),
    EventParameter::new("programval", CoreDataType::Int),
    EventParameter::new("routethroughpatch", CoreDataType::Char),
];

const MIDI_PITCH_BEND_INPUT_PARAMS: [EventParameter; 4] = [
    EventParameter::new("outport", CoreDataType::Int),
    EventParameter::with_max_index("midichannel", CoreDataType::Int, MAX_MIDI_CHANNELS as i32),
    EventParameter::new("pitchval", CoreDataType::Int),
    EventParameter::new("routethroughpatch", CoreDataType::Char),
];

const MIDI_KEY_INPUT_PARAMS: [EventParameter; 6] = [
    EventParameter::new("outport", CoreDataType::Int),
    EventParameter::new("keydown", CoreDataType::Char),
    EventParameter::with_max_index("midichannel", CoreDataType::Int, MAX_MIDI_CHANNELS as i32),
    EventParameter::with_max_index("notenum", CoreDataType::Int, MAX_MIDI_NOTES as i32),
    EventParameter::new("velocity", CoreDataType::Int),
    EventParameter::new("routethroughpatch", CoreDataType::Char),
];

const MIDI_POLYPHONIC_PRESSURE_INPUT_PARAMS: [EventParameter; 3] = [
    EventParameter::with_max_index("midichannel", CoreDataType::Int, MAX_MIDI_CHANNELS as i32),
    EventParameter::with_max_index("notenum", CoreDataType::Int, MAX_MIDI_NOTES as i32),
    EventParameter::new("pressureval", CoreDataType::Int),
];

const MIDI_VALUE_INPUT_PARAMS: [EventParameter; 1] =
    [EventParameter::new("value", CoreDataType::Int)];

const MIDI_CLOCK_INPUT_PARAMS: [EventParameter; 1] =
    [EventParameter::new("outport", CoreDataType::Int)];
const MIDI_START_STOP_INPUT_PARAMS: [EventParameter; 2] = [
    EventParameter::new("outport", CoreDataType::Int),
    EventParameter::new("start", CoreDataType::Char),
];

const BROWSER_MOVE_PARAMS: [EventParameter; 3] = [
    EventParameter::new("browserid", CoreDataType::Int),
    EventParameter::new("adjust", CoreDataType::Int),
    EventParameter::new("jumpadjust", CoreDataType::Int),
];

const BROWSER_MOVE_ABSOLUTE_PARAMS: [EventParameter; 2] = [
    EventParameter::new("browserid", CoreDataType::Int),
    EventParameter::new("idx", CoreDataType::Int),
];

const START_INTERFACE_PARAMS: [EventParameter; 1] =
    [EventParameter::new(INTERFACEID, CoreDataType::Int)];

const SLIDE_IN_VOLUME_PARAMS: [EventParameter; 2] = [
    EventParameter::new("input", CoreDataType::Int),
    EventParameter::new("slide", CoreDataType::Float),
];

const SLIDE_MASTER_VOLUME_PARAMS: [EventParameter; 1] =
    [EventParameter::new("slide", CoreDataType::Float)];

const SET_IN_VOLUME_PARAMS: [EventParameter; 3] = [
    EventParameter::new("input", CoreDataType::Int),
    EventParameter::new("vol", CoreDataType::Float),
    EventParameter::new("fadervol", CoreDataType::Float),
];

const TRIGGER_LOOP_PARAMS: [EventParameter; 2] = [
    EventParameter::new("loopid", CoreDataType::Int),
    EventParameter::new("vol", CoreDataType::Float),
];

const SET_MASTER_VOLUME_PARAMS: [EventParameter; 2] = [
    EventParameter::new("vol", CoreDataType::Float),
    EventParameter::new("fadervol", CoreDataType::Float),
];

const TOGGLE_INPUT_RECORD_PARAMS: [EventParameter; 1] =
    [EventParameter::new("input", CoreDataType::Int)];

const SET_MIDI_ECHO_PORT_PARAMS: [EventParameter; 1] =
    [EventParameter::new("echoport", CoreDataType::Int)];

const SET_MIDI_ECHO_CHANNEL_PARAMS: [EventParameter; 1] =
    [EventParameter::new("echochannel", CoreDataType::Int)];

const ADJUST_MIDI_TRANSPOSE_PARAMS: [EventParameter; 1] =
    [EventParameter::new("adjust", CoreDataType::Int)];

const SET_TRIGGER_VOLUME_PARAMS: [EventParameter; 2] = [
    EventParameter::new("loopid", CoreDataType::Int),
    EventParameter::new("vol", CoreDataType::Float),
];

const SLIDE_LOOP_AMP_PARAMS: [EventParameter; 2] = [
    EventParameter::new("loopid", CoreDataType::Int),
    EventParameter::new("slide", CoreDataType::Float),
];

const SET_LOOP_AMP_PARAMS: [EventParameter; 2] = [
    EventParameter::new("loopid", CoreDataType::Int),
    EventParameter::new("amp", CoreDataType::Float),
];

const ADJUST_LOOP_AMP_PARAMS: [EventParameter; 2] = [
    EventParameter::new("loopid", CoreDataType::Int),
    EventParameter::new("ampfactor", CoreDataType::Float),
];

const TOGGLE_SELECT_LOOP_PARAMS: [EventParameter; 2] = [
    EventParameter::new("setid", CoreDataType::Int),
    EventParameter::new("loopid", CoreDataType::Int),
];

const SELECT_ONLY_PLAYING_PARAMS: [EventParameter; 2] = [
    EventParameter::new("setid", CoreDataType::Int),
    EventParameter::new("playing", CoreDataType::Char),
];

const SELECT_ALL_LOOPS_PARAMS: [EventParameter; 2] = [
    EventParameter::new("setid", CoreDataType::Int),
    EventParameter::new("select", CoreDataType::Char),
];

const SETID_PARAMS: [EventParameter; 1] = [EventParameter::new("setid", CoreDataType::Int)];

const TRIGGER_SELECTED_LOOPS_PARAMS: [EventParameter; 3] = [
    EventParameter::new("setid", CoreDataType::Int),
    EventParameter::new("vol", CoreDataType::Float),
    EventParameter::new("toggleloops", CoreDataType::Char),
];

const SET_SELECTED_LOOPS_TRIGGER_VOLUME_PARAMS: [EventParameter; 2] = [
    EventParameter::new("setid", CoreDataType::Int),
    EventParameter::new("vol", CoreDataType::Float),
];

const ADJUST_SELECTED_LOOPS_AMP_PARAMS: [EventParameter; 2] = [
    EventParameter::new("setid", CoreDataType::Int),
    EventParameter::new("ampfactor", CoreDataType::Float),
];

const MOVE_LOOP_PARAMS: [EventParameter; 2] = [
    EventParameter::new("oldloopid", CoreDataType::Int),
    EventParameter::new("newloopid", CoreDataType::Int),
];

const LOOP_ID_PARAMS: [EventParameter; 1] = [EventParameter::new("loopid", CoreDataType::Int)];

const LOOP_RANGE_PARAMS: [EventParameter; 1] =
    [EventParameter::new("looprange", CoreDataType::Range)];

const PULSE_PARAMS: [EventParameter; 1] = [EventParameter::new("pulse", CoreDataType::Int)];

const TAP_PULSE_PARAMS: [EventParameter; 2] = [
    EventParameter::new("pulse", CoreDataType::Int),
    EventParameter::new("newlen", CoreDataType::Char),
];

const SWITCH_METRONOME_PARAMS: [EventParameter; 2] = [
    EventParameter::new("pulse", CoreDataType::Int),
    EventParameter::new("metronome", CoreDataType::Char),
];

const SET_SYNC_TYPE_PARAMS: [EventParameter; 1] =
    [EventParameter::new("stype", CoreDataType::Char)];

const SET_SYNC_SPEED_PARAMS: [EventParameter; 1] = [EventParameter::new("sspd", CoreDataType::Int)];

const SET_MIDI_SYNC_PARAMS: [EventParameter; 1] =
    [EventParameter::new("midisync", CoreDataType::Int)];

const SET_VARIABLE_PARAMS: [EventParameter; 4] = [
    EventParameter::new("var", CoreDataType::VariableRef),
    EventParameter::new("value", CoreDataType::Variable),
    EventParameter::new("maxjumpcheck", CoreDataType::Char),
    EventParameter::new("maxjump", CoreDataType::Variable),
];

const TOGGLE_VARIABLE_PARAMS: [EventParameter; 3] = [
    EventParameter::new("var", CoreDataType::VariableRef),
    EventParameter::new("maxvalue", CoreDataType::Int),
    EventParameter::new("minvalue", CoreDataType::Int),
];

const SPLIT_VARIABLE_PARAMS: [EventParameter; 3] = [
    EventParameter::new("var", CoreDataType::Variable),
    EventParameter::new("msb", CoreDataType::VariableRef),
    EventParameter::new("lsb", CoreDataType::VariableRef),
];

const ENABLE_BOOL_PARAMS: [EventParameter; 1] = [EventParameter::new("enable", CoreDataType::Char)];

const MIDI_TUNING_PARAMS: [EventParameter; 1] =
    [EventParameter::new("tuning", CoreDataType::Float)];

const BROWSER_SELECT_PARAMS: [EventParameter; 1] =
    [EventParameter::new("browserid", CoreDataType::Int)];

const PATCH_BANK_MOVE_PARAMS: [EventParameter; 1] =
    [EventParameter::new("direction", CoreDataType::Int)];

const PATCH_BANK_INDEX_PARAMS: [EventParameter; 1] =
    [EventParameter::new("idx", CoreDataType::Int)];

const VIDEO_SHOW_LOOP_PARAMS: [EventParameter; 3] = [
    EventParameter::new(INTERFACEID, CoreDataType::Int),
    EventParameter::new("layoutid", CoreDataType::Int),
    EventParameter::new("loopid", CoreDataType::Range),
];

const VIDEO_DISPLAY_SHOW_PARAMS: [EventParameter; 3] = [
    EventParameter::new(INTERFACEID, CoreDataType::Int),
    EventParameter::new("displayid", CoreDataType::Int),
    EventParameter::new("show", CoreDataType::Char),
];

const VIDEO_LAYOUT_SHOW_PARAMS: [EventParameter; 4] = [
    EventParameter::new(INTERFACEID, CoreDataType::Int),
    EventParameter::new("layoutid", CoreDataType::Int),
    EventParameter::new("show", CoreDataType::Char),
    EventParameter::new("hideothers", CoreDataType::Char),
];

const VIDEO_PAGE_PARAMS: [EventParameter; 3] = [
    EventParameter::new(INTERFACEID, CoreDataType::Int),
    EventParameter::new("displayid", CoreDataType::Int),
    EventParameter::new("page", CoreDataType::Int),
];

const VIDEO_BANK_PARAMS: [EventParameter; 3] = [
    EventParameter::new(INTERFACEID, CoreDataType::Int),
    EventParameter::new("displayid", CoreDataType::Int),
    EventParameter::new("bank", CoreDataType::Int),
];

const VIDEO_HELP_PARAMS: [EventParameter; 1] = [EventParameter::new("page", CoreDataType::Int)];

const VIDEO_FULLSCREEN_PARAMS: [EventParameter; 1] =
    [EventParameter::new("fullscreen", CoreDataType::Char)];

const SHOW_DEBUG_INFO_PARAMS: [EventParameter; 1] =
    [EventParameter::new("show", CoreDataType::Char)];

const GO_SUB_PARAMS: [EventParameter; 4] = [
    EventParameter::with_max_index("sub", CoreDataType::Int, 127),
    EventParameter::new("param1", CoreDataType::Float),
    EventParameter::new("param2", CoreDataType::Float),
    EventParameter::new("param3", CoreDataType::Float),
];

const LOOP_CLICKED_PARAMS: [EventParameter; 4] = [
    EventParameter::new("down", CoreDataType::Char),
    EventParameter::new("button", CoreDataType::Int),
    EventParameter::new("loopid", CoreDataType::Int),
    EventParameter::new("in", CoreDataType::Char),
];

const PARAMSET_ABS_PARAM_IDX_PARAMS: [EventParameter; 4] = [
    EventParameter::new(INTERFACEID, CoreDataType::Int),
    EventParameter::new("displayid", CoreDataType::Int),
    EventParameter::new("paramidx", CoreDataType::Int),
    EventParameter::new("absidx", CoreDataType::VariableRef),
];

const PARAMSET_GET_PARAM_PARAMS: [EventParameter; 4] = [
    EventParameter::new(INTERFACEID, CoreDataType::Int),
    EventParameter::new("displayid", CoreDataType::Int),
    EventParameter::new("paramidx", CoreDataType::Int),
    EventParameter::new("var", CoreDataType::VariableRef),
];

const PARAMSET_SET_PARAM_PARAMS: [EventParameter; 4] = [
    EventParameter::new(INTERFACEID, CoreDataType::Int),
    EventParameter::new("displayid", CoreDataType::Int),
    EventParameter::new("paramidx", CoreDataType::Int),
    EventParameter::new("value", CoreDataType::Float),
];

const LOG_FADER_TO_LINEAR_PARAMS: [EventParameter; 3] = [
    EventParameter::new("var", CoreDataType::VariableRef),
    EventParameter::new("fadervol", CoreDataType::Variable),
    EventParameter::new("scale", CoreDataType::Float),
];

const ALSA_MIXER_CONTROL_SET_PARAMS: [EventParameter; 6] = [
    EventParameter::new("hwid", CoreDataType::Int),
    EventParameter::new("numid", CoreDataType::Int),
    EventParameter::new("val1", CoreDataType::Int),
    EventParameter::new("val2", CoreDataType::Int),
    EventParameter::new("val3", CoreDataType::Int),
    EventParameter::new("val4", CoreDataType::Int),
];

const RENAME_LOOP_PARAMS: [EventParameter; 2] = [
    EventParameter::new("loopid", CoreDataType::Int),
    EventParameter::new("in", CoreDataType::Char),
];

const SNAPSHOT_ID_PARAMS: [EventParameter; 1] = [EventParameter::new("snapid", CoreDataType::Int)];

const SNAPSHOT_SWAP_PARAMS: [EventParameter; 2] = [
    EventParameter::new("snapid1", CoreDataType::Int),
    EventParameter::new("snapid2", CoreDataType::Int),
];

const SAVE_BOOL_PARAMS: [EventParameter; 1] = [EventParameter::new("save", CoreDataType::Char)];

// ============================================================
// EventType
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    None,
    InputKey,
    InputJoystickButton,
    InputMIDIKey,
    InputMIDIController,
    InputMIDIProgramChange,
    InputMIDIChannelPressure,
    InputMIDIPitchBend,
    InputMIDIPolyphonicPressure,
    InputMIDISystemExclusive,
    InputMIDITimeCodeQuarterFrame,
    InputMIDISongPosition,
    InputMIDISongSelect,
    InputMIDITuneRequest,
    InputMIDIActiveSensing,
    InputMIDIReset,
    StartSession,
    StartInterface,
    GoSub,
    LoopClicked,
    BrowserItemBrowsed,
    LastBindable,
    InputMIDIClock,
    InputMIDIStartStop,
    InputMouseButton,
    InputMouseMotion,
    ALSAMixerControlSet,
    EndRecord,
    LoopList,
    SceneMarker,
    PulseSync,
    TriggerSet,
    AddProcessor,
    DelProcessor,
    CleanupProcessor,
    SetVariable,
    ToggleVariable,
    SplitVariableMSBLSB,
    ParamSetGetAbsoluteParamIdx,
    ParamSetGetParam,
    ParamSetSetParam,
    LogFaderVolToLinear,
    VideoShowParamSetBank,
    VideoShowParamSetPage,
    VideoShowSnapshotPage,
    VideoShowLoop,
    VideoShowLayout,
    VideoSwitchInterface,
    VideoShowDisplay,
    VideoShowHelp,
    VideoFullScreen,
    ShowDebugInfo,
    ExitSession,
    SlideMasterInVolume,
    SlideMasterOutVolume,
    SlideInVolume,
    SetMasterInVolume,
    SetMasterOutVolume,
    SetInVolume,
    ToggleInputRecord,
    SetMidiEchoPort,
    SetMidiEchoChannel,
    AdjustMidiTranspose,
    FluidSynthEnable,
    SetMidiTuning,
    DeletePulse,
    SelectPulse,
    TapPulse,
    SwitchMetronome,
    SetSyncType,
    SetSyncSpeed,
    SetMidiSync,
    ToggleSelectLoop,
    SelectOnlyPlayingLoops,
    SelectAllLoops,
    TriggerSelectedLoops,
    SetSelectedLoopsTriggerVolume,
    AdjustSelectedLoopsAmp,
    InvertSelection,
    CreateSnapshot,
    RenameSnapshot,
    TriggerSnapshot,
    SwapSnapshots,
    SetTriggerVolume,
    SlideLoopAmp,
    SetLoopAmp,
    AdjustLoopAmp,
    TriggerLoop,
    MoveLoop,
    RenameLoop,
    EraseLoop,
    EraseAllLoops,
    EraseSelectedLoops,
    SlideLoopAmpStopAll,
    ToggleDiskOutput,
    SetAutoLoopSaving,
    SaveLoop,
    SaveNewScene,
    SaveCurrentScene,
    SetLoadLoopId,
    SetDefaultLoopPlacement,
    BrowserMoveToItem,
    BrowserMoveToItemAbsolute,
    BrowserSelectItem,
    BrowserRenameItem,
    PatchBrowserMoveToBank,
    PatchBrowserMoveToBankByIndex,
    TransmitPlayingLoopsToDAW,
    Last,
}

#[derive(Clone)]
struct EventTypeMeta {
    name: &'static str,
    slow_delivery: bool,
}

impl EventType {
    fn meta(self) -> EventTypeMeta {
        use EventType::*;
        let (name, slow) = match self {
            InputKey => ("key", false),
            InputJoystickButton => ("joybutton", false),
            InputMouseButton => ("mousebutton", false),
            InputMouseMotion => ("mousemotion", false),
            InputMIDIKey => ("midikey", false),
            InputMIDIController => ("midicontroller", false),
            InputMIDIProgramChange => ("midiprogramchange", false),
            InputMIDIChannelPressure => ("midichannelpressure", false),
            InputMIDIPitchBend => ("midipitchbend", false),
            InputMIDIPolyphonicPressure => ("midipolyphonicpressure", false),
            InputMIDISystemExclusive => ("midisysex", false),
            InputMIDITimeCodeQuarterFrame => ("midimtcquarterframe", false),
            InputMIDISongPosition => ("midisongposition", false),
            InputMIDISongSelect => ("midisongselect", false),
            InputMIDITuneRequest => ("miditunerequest", false),
            InputMIDIActiveSensing => ("midiactivesensing", false),
            InputMIDIReset => ("midireset", false),
            InputMIDIClock => ("midiclock", false),
            InputMIDIStartStop => ("midistartstop", false),
            EndRecord => ("end-record", false),
            LoopList => ("loop-list", false),
            SceneMarker => ("scene-marker", false),
            TriggerSet => ("trigger-set", false),
            ALSAMixerControlSet => ("alsa-mixer-control-set", true),
            AddProcessor => ("add-processor", false),
            DelProcessor => ("del-processor", false),
            CleanupProcessor => ("cleanup-processor", false),
            LoopClicked => ("loop-clicked", false),
            GoSub => ("go-sub", false),
            StartSession => ("start-freewheeling", false),
            StartInterface => ("start-interface", false),
            ExitSession => ("exit-freewheeling", true),
            SlideMasterInVolume => ("slide-master-in-volume", false),
            SlideMasterOutVolume => ("slide-master-out-volume", false),
            SlideInVolume => ("slide-in-volume", false),
            SetMasterInVolume => ("set-master-in-volume", false),
            SetMasterOutVolume => ("set-master-out-volume", false),
            SetInVolume => ("set-in-volume", false),
            ToggleInputRecord => ("toggle-input-record", false),
            SetMidiEchoPort => ("set-midi-echo-port", false),
            SetMidiEchoChannel => ("set-midi-echo-channel", false),
            AdjustMidiTranspose => ("adjust-midi-transpose", false),
            FluidSynthEnable => ("fluidsynth-enable", false),
            SetMidiTuning => ("set-midi-tuning", false),
            SetTriggerVolume => ("set-trigger-volume", false),
            SlideLoopAmp => ("slide-loop-amplifier", false),
            SetLoopAmp => ("set-loop-amplifier", false),
            AdjustLoopAmp => ("adjust-loop-amplifier", false),
            TriggerLoop => ("trigger-loop", false),
            MoveLoop => ("move-loop", true),
            RenameLoop => ("rename-loop", true),
            EraseLoop => ("erase-loop", true),
            EraseAllLoops => ("erase-all-loops", true),
            EraseSelectedLoops => ("erase-selected-loops", true),
            SlideLoopAmpStopAll => ("slide-loop-amplifier-stop-all", false),
            DeletePulse => ("delete-pulse", true),
            SelectPulse => ("select-pulse", true),
            TapPulse => ("tap-pulse", false),
            SwitchMetronome => ("switch-metronome", false),
            SetSyncType => ("set-sync-type", false),
            SetSyncSpeed => ("set-sync-speed", false),
            SetMidiSync => ("set-midi-sync", false),
            ToggleSelectLoop => ("toggle-select-loop", true),
            SelectOnlyPlayingLoops => ("select-only-playing-loops", true),
            SelectAllLoops => ("select-all-loops", true),
            TriggerSelectedLoops => ("trigger-selected-loops", true),
            SetSelectedLoopsTriggerVolume => ("set-selected-loops-trigger-volume", false),
            AdjustSelectedLoopsAmp => ("adjust-selected-loops-amp", false),
            InvertSelection => ("invert-selection", true),
            CreateSnapshot => ("create-snapshot", true),
            RenameSnapshot => ("rename-snapshot", true),
            TriggerSnapshot => ("trigger-snapshot", true),
            SwapSnapshots => ("swap-snapshots", true),
            BrowserMoveToItem => ("browser-move-to-item", true),
            BrowserMoveToItemAbsolute => ("browser-move-to-item-absolute", true),
            BrowserSelectItem => ("browser-select-item", true),
            BrowserRenameItem => ("browser-rename-item", true),
            BrowserItemBrowsed => ("browser-item-browsed", false),
            PatchBrowserMoveToBank => ("patchbrowser-move-to-bank", true),
            PatchBrowserMoveToBankByIndex => ("patchbrowser-move-to-bank-by-index", true),
            TransmitPlayingLoopsToDAW => ("transmit-playing-loops-to-daw", true),
            SetVariable => ("set-variable", false),
            ToggleVariable => ("toggle-variable", false),
            SplitVariableMSBLSB => ("split-variable-msb-lsb", false),
            ParamSetGetAbsoluteParamIdx => ("paramset-get-absolute-param-index", false),
            ParamSetGetParam => ("paramset-get-param", false),
            ParamSetSetParam => ("paramset-set-param", false),
            LogFaderVolToLinear => ("log-fader-to-linear", false),
            VideoShowParamSetBank => ("video-show-paramset-bank", false),
            VideoShowParamSetPage => ("video-show-paramset-page", false),
            VideoShowSnapshotPage => ("video-show-snapshot-page", true),
            VideoShowLoop => ("video-show-loop", true),
            VideoShowLayout => ("video-show-layout", true),
            VideoSwitchInterface => ("video-switch-interface", true),
            VideoShowDisplay => ("video-show-display", true),
            VideoShowHelp => ("video-show-help", true),
            VideoFullScreen => ("video-full-screen", true),
            ShowDebugInfo => ("show-debug-info", true),
            ToggleDiskOutput => ("toggle-disk-output", true),
            SetAutoLoopSaving => ("set-auto-loop-saving", false),
            SaveLoop => ("save-loop", true),
            SaveNewScene => ("save-new-scene", true),
            SaveCurrentScene => ("save-current-scene", true),
            SetLoadLoopId => ("set-load-loop-id", false),
            SetDefaultLoopPlacement => ("set-default-loop-placement", false),
            _ => ("", false),
        };
        EventTypeMeta {
            name,
            slow_delivery: slow,
        }
    }

    pub fn name(self) -> &'static str {
        self.meta().name
    }
    pub fn is_slow(self) -> bool {
        self.meta().slow_delivery
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "key" => Some(EventType::InputKey),
            "joybutton" => Some(EventType::InputJoystickButton),
            "mousebutton" => Some(EventType::InputMouseButton),
            "mousemotion" => Some(EventType::InputMouseMotion),
            "midikey" => Some(EventType::InputMIDIKey),
            "midicontroller" => Some(EventType::InputMIDIController),
            "midiprogramchange" => Some(EventType::InputMIDIProgramChange),
            "midichannelpressure" => Some(EventType::InputMIDIChannelPressure),
            "midipitchbend" => Some(EventType::InputMIDIPitchBend),
            "midipolyphonicpressure" => Some(EventType::InputMIDIPolyphonicPressure),
            "midisysex" => Some(EventType::InputMIDISystemExclusive),
            "midimtcquarterframe" => Some(EventType::InputMIDITimeCodeQuarterFrame),
            "midisongposition" => Some(EventType::InputMIDISongPosition),
            "midisongselect" => Some(EventType::InputMIDISongSelect),
            "miditunerequest" => Some(EventType::InputMIDITuneRequest),
            "midiactivesensing" => Some(EventType::InputMIDIActiveSensing),
            "midireset" => Some(EventType::InputMIDIReset),
            "midiclock" => Some(EventType::InputMIDIClock),
            "midistartstop" => Some(EventType::InputMIDIStartStop),
            "end-record" => Some(EventType::EndRecord),
            "loop-list" => Some(EventType::LoopList),
            "scene-marker" => Some(EventType::SceneMarker),
            "trigger-set" => Some(EventType::TriggerSet),
            "go-sub" => Some(EventType::GoSub),
            "loop-clicked" => Some(EventType::LoopClicked),
            "add-processor" => Some(EventType::AddProcessor),
            "del-processor" => Some(EventType::DelProcessor),
            "cleanup-processor" => Some(EventType::CleanupProcessor),
            "alsa-mixer-control-set" => Some(EventType::ALSAMixerControlSet),
            "browser-move-to-item" => Some(EventType::BrowserMoveToItem),
            "browser-move-to-item-absolute" => Some(EventType::BrowserMoveToItemAbsolute),
            "browser-select-item" => Some(EventType::BrowserSelectItem),
            "browser-rename-item" => Some(EventType::BrowserRenameItem),
            "browser-item-browsed" => Some(EventType::BrowserItemBrowsed),
            "patchbrowser-move-to-bank" => Some(EventType::PatchBrowserMoveToBank),
            "patchbrowser-move-to-bank-by-index" => Some(EventType::PatchBrowserMoveToBankByIndex),
            "start-freewheeling" => Some(EventType::StartSession),
            "start-interface" => Some(EventType::StartInterface),
            "exit-freewheeling" => Some(EventType::ExitSession),
            "fluidsynth-enable" => Some(EventType::FluidSynthEnable),
            "set-midi-tuning" => Some(EventType::SetMidiTuning),
            "video-show-loop" => Some(EventType::VideoShowLoop),
            "video-show-layout" => Some(EventType::VideoShowLayout),
            "video-show-snapshot-page" => Some(EventType::VideoShowSnapshotPage),
            "video-show-paramset-bank" => Some(EventType::VideoShowParamSetBank),
            "video-show-paramset-page" => Some(EventType::VideoShowParamSetPage),
            "video-switch-interface" => Some(EventType::VideoSwitchInterface),
            "video-show-display" => Some(EventType::VideoShowDisplay),
            "video-show-help" => Some(EventType::VideoShowHelp),
            "video-full-screen" => Some(EventType::VideoFullScreen),
            "show-debug-info" => Some(EventType::ShowDebugInfo),
            "slide-master-in-volume" => Some(EventType::SlideMasterInVolume),
            "slide-master-out-volume" => Some(EventType::SlideMasterOutVolume),
            "slide-in-volume" => Some(EventType::SlideInVolume),
            "set-master-in-volume" => Some(EventType::SetMasterInVolume),
            "set-master-out-volume" => Some(EventType::SetMasterOutVolume),
            "set-in-volume" => Some(EventType::SetInVolume),
            "toggle-input-record" => Some(EventType::ToggleInputRecord),
            "set-midi-echo-port" => Some(EventType::SetMidiEchoPort),
            "set-midi-echo-channel" => Some(EventType::SetMidiEchoChannel),
            "adjust-midi-transpose" => Some(EventType::AdjustMidiTranspose),
            "paramset-get-absolute-param-index" => Some(EventType::ParamSetGetAbsoluteParamIdx),
            "paramset-get-param" => Some(EventType::ParamSetGetParam),
            "paramset-set-param" => Some(EventType::ParamSetSetParam),
            "log-fader-to-linear" => Some(EventType::LogFaderVolToLinear),
            "set-trigger-volume" => Some(EventType::SetTriggerVolume),
            "slide-loop-amplifier" => Some(EventType::SlideLoopAmp),
            "set-loop-amplifier" => Some(EventType::SetLoopAmp),
            "adjust-loop-amplifier" => Some(EventType::AdjustLoopAmp),
            "rename-loop" => Some(EventType::RenameLoop),
            "erase-selected-loops" => Some(EventType::EraseSelectedLoops),
            "toggle-disk-output" => Some(EventType::ToggleDiskOutput),
            "set-auto-loop-saving" => Some(EventType::SetAutoLoopSaving),
            "save-new-scene" => Some(EventType::SaveNewScene),
            "save-current-scene" => Some(EventType::SaveCurrentScene),
            "set-load-loop-id" => Some(EventType::SetLoadLoopId),
            "create-snapshot" => Some(EventType::CreateSnapshot),
            "swap-snapshots" => Some(EventType::SwapSnapshots),
            "rename-snapshot" => Some(EventType::RenameSnapshot),
            "trigger-snapshot" => Some(EventType::TriggerSnapshot),
            "transmit-playing-loops-to-daw" => Some(EventType::TransmitPlayingLoopsToDAW),
            "toggle-select-loop" => Some(EventType::ToggleSelectLoop),
            "select-only-playing-loops" => Some(EventType::SelectOnlyPlayingLoops),
            "select-all-loops" => Some(EventType::SelectAllLoops),
            "trigger-selected-loops" => Some(EventType::TriggerSelectedLoops),
            "set-selected-loops-trigger-volume" => Some(EventType::SetSelectedLoopsTriggerVolume),
            "adjust-selected-loops-amp" => Some(EventType::AdjustSelectedLoopsAmp),
            "invert-selection" => Some(EventType::InvertSelection),
            "trigger-loop" => Some(EventType::TriggerLoop),
            "move-loop" => Some(EventType::MoveLoop),
            "erase-loop" => Some(EventType::EraseLoop),
            "erase-all-loops" => Some(EventType::EraseAllLoops),
            "save-loop" => Some(EventType::SaveLoop),
            "set-default-loop-placement" => Some(EventType::SetDefaultLoopPlacement),
            "select-pulse" => Some(EventType::SelectPulse),
            "delete-pulse" => Some(EventType::DeletePulse),
            "tap-pulse" => Some(EventType::TapPulse),
            "switch-metronome" => Some(EventType::SwitchMetronome),
            "set-sync-type" => Some(EventType::SetSyncType),
            "set-sync-speed" => Some(EventType::SetSyncSpeed),
            "set-midi-sync" => Some(EventType::SetMidiSync),
            "pulse-sync" => Some(EventType::PulseSync),
            "slide-loop-amplifier-stop-all" => Some(EventType::SlideLoopAmpStopAll),
            "set-variable" => Some(EventType::SetVariable),
            "toggle-variable" => Some(EventType::ToggleVariable),
            "split-variable-msb-lsb" => Some(EventType::SplitVariableMSBLSB),
            _ => Option::None,
        }
    }

    pub fn parameters(self) -> &'static [EventParameter] {
        match self {
            EventType::InputKey => &KEY_INPUT_PARAMS,
            EventType::InputJoystickButton => &JOYSTICK_BUTTON_INPUT_PARAMS,
            EventType::InputMouseButton => &MOUSE_BUTTON_INPUT_PARAMS,
            EventType::InputMouseMotion => &MOUSE_MOTION_INPUT_PARAMS,
            EventType::InputMIDIController => &MIDI_CONTROLLER_INPUT_PARAMS,
            EventType::InputMIDIProgramChange => &MIDI_PROGRAM_CHANGE_INPUT_PARAMS,
            EventType::InputMIDIChannelPressure => &MIDI_CHANNEL_PRESSURE_INPUT_PARAMS,
            EventType::InputMIDIPitchBend => &MIDI_PITCH_BEND_INPUT_PARAMS,
            EventType::InputMIDIPolyphonicPressure => &MIDI_POLYPHONIC_PRESSURE_INPUT_PARAMS,
            EventType::InputMIDITimeCodeQuarterFrame
            | EventType::InputMIDISongPosition
            | EventType::InputMIDISongSelect => &MIDI_VALUE_INPUT_PARAMS,
            EventType::InputMIDIKey => &MIDI_KEY_INPUT_PARAMS,
            EventType::GoSub => &GO_SUB_PARAMS,
            EventType::LoopClicked => &LOOP_CLICKED_PARAMS,
            EventType::ALSAMixerControlSet => &ALSA_MIXER_CONTROL_SET_PARAMS,
            EventType::ParamSetGetAbsoluteParamIdx => &PARAMSET_ABS_PARAM_IDX_PARAMS,
            EventType::ParamSetGetParam => &PARAMSET_GET_PARAM_PARAMS,
            EventType::ParamSetSetParam => &PARAMSET_SET_PARAM_PARAMS,
            EventType::LogFaderVolToLinear => &LOG_FADER_TO_LINEAR_PARAMS,
            EventType::FluidSynthEnable => &ENABLE_BOOL_PARAMS,
            EventType::SetMidiTuning => &MIDI_TUNING_PARAMS,
            EventType::SlideMasterInVolume => &SLIDE_MASTER_VOLUME_PARAMS,
            EventType::SlideMasterOutVolume => &SLIDE_MASTER_VOLUME_PARAMS,
            EventType::BrowserMoveToItem => &BROWSER_MOVE_PARAMS,
            EventType::BrowserMoveToItemAbsolute => &BROWSER_MOVE_ABSOLUTE_PARAMS,
            EventType::BrowserSelectItem => &BROWSER_SELECT_PARAMS,
            EventType::BrowserRenameItem => &BROWSER_SELECT_PARAMS,
            EventType::BrowserItemBrowsed => &BROWSER_SELECT_PARAMS,
            EventType::PatchBrowserMoveToBank => &PATCH_BANK_MOVE_PARAMS,
            EventType::PatchBrowserMoveToBankByIndex => &PATCH_BANK_INDEX_PARAMS,
            EventType::StartInterface => &START_INTERFACE_PARAMS,
            EventType::VideoShowLoop => &VIDEO_SHOW_LOOP_PARAMS,
            EventType::VideoShowLayout => &VIDEO_LAYOUT_SHOW_PARAMS,
            EventType::VideoShowSnapshotPage => &VIDEO_PAGE_PARAMS,
            EventType::VideoShowParamSetBank => &VIDEO_BANK_PARAMS,
            EventType::VideoShowParamSetPage => &VIDEO_PAGE_PARAMS,
            EventType::VideoSwitchInterface => &START_INTERFACE_PARAMS,
            EventType::VideoShowDisplay => &VIDEO_DISPLAY_SHOW_PARAMS,
            EventType::VideoShowHelp => &VIDEO_HELP_PARAMS,
            EventType::VideoFullScreen => &VIDEO_FULLSCREEN_PARAMS,
            EventType::ShowDebugInfo => &SHOW_DEBUG_INFO_PARAMS,
            EventType::SlideInVolume => &SLIDE_IN_VOLUME_PARAMS,
            EventType::SetMasterInVolume => &SET_MASTER_VOLUME_PARAMS,
            EventType::SetMasterOutVolume => &SET_MASTER_VOLUME_PARAMS,
            EventType::SetInVolume => &SET_IN_VOLUME_PARAMS,
            EventType::ToggleInputRecord => &TOGGLE_INPUT_RECORD_PARAMS,
            EventType::SetMidiEchoPort => &SET_MIDI_ECHO_PORT_PARAMS,
            EventType::SetMidiEchoChannel => &SET_MIDI_ECHO_CHANNEL_PARAMS,
            EventType::AdjustMidiTranspose => &ADJUST_MIDI_TRANSPOSE_PARAMS,
            EventType::SetTriggerVolume => &SET_TRIGGER_VOLUME_PARAMS,
            EventType::SlideLoopAmp => &SLIDE_LOOP_AMP_PARAMS,
            EventType::SetLoopAmp => &SET_LOOP_AMP_PARAMS,
            EventType::AdjustLoopAmp => &ADJUST_LOOP_AMP_PARAMS,
            EventType::RenameLoop => &RENAME_LOOP_PARAMS,
            EventType::ToggleSelectLoop => &TOGGLE_SELECT_LOOP_PARAMS,
            EventType::SelectOnlyPlayingLoops => &SELECT_ONLY_PLAYING_PARAMS,
            EventType::SelectAllLoops => &SELECT_ALL_LOOPS_PARAMS,
            EventType::TriggerSelectedLoops => &TRIGGER_SELECTED_LOOPS_PARAMS,
            EventType::SetSelectedLoopsTriggerVolume => &SET_SELECTED_LOOPS_TRIGGER_VOLUME_PARAMS,
            EventType::AdjustSelectedLoopsAmp => &ADJUST_SELECTED_LOOPS_AMP_PARAMS,
            EventType::InvertSelection => &SETID_PARAMS,
            EventType::CreateSnapshot => &SNAPSHOT_ID_PARAMS,
            EventType::RenameSnapshot => &SNAPSHOT_ID_PARAMS,
            EventType::TriggerSnapshot => &SNAPSHOT_ID_PARAMS,
            EventType::SwapSnapshots => &SNAPSHOT_SWAP_PARAMS,
            EventType::TriggerLoop => &TRIGGER_LOOP_PARAMS,
            EventType::MoveLoop => &MOVE_LOOP_PARAMS,
            EventType::EraseLoop => &LOOP_ID_PARAMS,
            EventType::EraseSelectedLoops => &SETID_PARAMS,
            EventType::ToggleDiskOutput => &[],
            EventType::SetAutoLoopSaving => &SAVE_BOOL_PARAMS,
            EventType::SaveLoop => &LOOP_ID_PARAMS,
            EventType::SaveNewScene => &[],
            EventType::SaveCurrentScene => &[],
            EventType::SetLoadLoopId => &LOOP_ID_PARAMS,
            EventType::SetDefaultLoopPlacement => &LOOP_RANGE_PARAMS,
            EventType::SelectPulse => &PULSE_PARAMS,
            EventType::DeletePulse => &PULSE_PARAMS,
            EventType::TapPulse => &TAP_PULSE_PARAMS,
            EventType::SwitchMetronome => &SWITCH_METRONOME_PARAMS,
            EventType::SetSyncType => &SET_SYNC_TYPE_PARAMS,
            EventType::SetSyncSpeed => &SET_SYNC_SPEED_PARAMS,
            EventType::SetMidiSync => &SET_MIDI_SYNC_PARAMS,
            EventType::PulseSync => &[],
            EventType::SlideLoopAmpStopAll => &[],
            EventType::TransmitPlayingLoopsToDAW => &[],
            EventType::SetVariable => &SET_VARIABLE_PARAMS,
            EventType::ToggleVariable => &TOGGLE_VARIABLE_PARAMS,
            EventType::SplitVariableMSBLSB => &SPLIT_VARIABLE_PARAMS,
            EventType::InputMIDIClock => &MIDI_CLOCK_INPUT_PARAMS,
            EventType::InputMIDIStartStop => &MIDI_START_STOP_INPUT_PARAMS,
            EventType::InputMIDITuneRequest => &[],
            EventType::InputMIDIActiveSensing => &[],
            EventType::InputMIDIReset => &[],
            _ => &[],
        }
    }
}

// ============================================================
// Traits
// ============================================================

pub trait EventProducer: Send {
    fn producer_name(&self) -> &str {
        "unknown"
    }
}
impl EventProducer for () {}

pub trait EventListener: Send {
    fn receive_event(&mut self, ev: &Event, from: &dyn EventProducer);
}

// ============================================================
// Event enum — single type for all events
// ============================================================

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    // Unit variants (no data beyond the tag)
    None,
    StartSession,
    ExitSession,
    SlideLoopAmpStopAll,
    EraseAllLoops,
    ToggleDiskOutput,
    SaveNewScene,
    SaveCurrentScene,
    TransmitPlayingLoopsToDAW,
    PulseSync,
    MIDITuneRequestInput,
    MIDIActiveSensingInput,
    MIDIResetInput,

    // Struct variants with fields
    EndRecord {
        keeprecord: bool,
    },
    GoSub {
        sub: i32,
        param1: f32,
        param2: f32,
        param3: f32,
    },
    KeyInput {
        down: bool,
        keysym: i32,
        unicode: i32,
        presslen: u32,
    },
    LoopClicked {
        down: bool,
        button: i32,
        loopid: i32,
        in_layout: bool,
        presslen: u32,
    },
    JoystickButtonInput {
        down: bool,
        button: i32,
        joystick: i32,
        presslen: u32,
    },
    MouseButtonInput {
        down: bool,
        button: i32,
        x: i32,
        y: i32,
        presslen: u32,
    },
    MouseMotionInput {
        x: i32,
        y: i32,
    },
    TriggerLoop {
        index: i32,
        vol: f32,
        engage: i32,
        shot: bool,
        overdub: bool,
        overdub_feedback_var: Option<String>,
    },
    MIDIControllerInput {
        outport: i32,
        channel: u8,
        ctrl: u8,
        val: u8,
        echo: bool,
    },
    MIDIChannelPressureInput {
        outport: i32,
        channel: u8,
        val: u8,
        echo: bool,
    },
    MIDIProgramChangeInput {
        outport: i32,
        channel: u8,
        val: u8,
        echo: bool,
    },
    MIDIPitchBendInput {
        outport: i32,
        channel: u8,
        val: i32,
        echo: bool,
    },
    MIDIPolyphonicPressureInput {
        channel: u8,
        notenum: u8,
        val: u8,
    },
    MIDISystemExclusiveInput {
        bytes: Vec<u8>,
    },
    MIDITimeCodeQuarterFrameInput {
        value: u16,
    },
    MIDISongPositionInput {
        value: u16,
    },
    MIDISongSelectInput {
        value: u16,
    },
    MIDIKeyInput {
        outport: i32,
        channel: u8,
        notenum: u8,
        vel: u8,
        down: bool,
        echo: bool,
    },
    MIDIClockInput {
        outport: i32,
    },
    MIDIStartStopInput {
        outport: i32,
        start: bool,
    },
    FluidSynthEnable {
        enable: bool,
    },
    SetMidiTuning {
        tuning: f32,
    },
    BrowserMoveToItem {
        browserid: i32,
        adjust: i32,
        jump_adjust: i32,
    },
    BrowserMoveToItemAbsolute {
        browserid: i32,
        index: i32,
    },
    BrowserSelectItem {
        browserid: i32,
    },
    BrowserRenameItem {
        browserid: i32,
    },
    BrowserItemBrowsed {
        browserid: i32,
    },
    PatchBrowserMoveToBank {
        direction: i32,
    },
    PatchBrowserMoveToBankByIndex {
        index: i32,
    },
    StartInterface {
        interfaceid: i32,
    },
    VideoShowLayout {
        interfaceid: i32,
        layoutid: i32,
        show: bool,
        hideothers: bool,
    },
    VideoSwitchInterface {
        interfaceid: i32,
    },
    VideoShowDisplay {
        interfaceid: i32,
        displayid: i32,
        show: bool,
    },
    VideoShowHelp {
        page: i32,
    },
    VideoFullScreen {
        fullscreen: bool,
    },
    ShowDebugInfo {
        show: bool,
    },
    VideoShowLoop {
        interfaceid: i32,
        layoutid: i32,
        loopid: Range,
    },
    VideoShowSnapshotPage {
        interfaceid: i32,
        displayid: i32,
        page: i32,
    },
    VideoShowParamSetBank {
        interfaceid: i32,
        displayid: i32,
        bank: i32,
    },
    VideoShowParamSetPage {
        interfaceid: i32,
        displayid: i32,
        page: i32,
    },
    SlideMasterInVolume {
        slide: f32,
    },
    SlideMasterOutVolume {
        slide: f32,
    },
    SlideInVolume {
        input: i32,
        slide: f32,
    },
    SetMasterInVolume {
        vol: f32,
        fadervol: f32,
    },
    SetMasterOutVolume {
        vol: f32,
        fadervol: f32,
    },
    SetInVolume {
        input: i32,
        vol: f32,
        fadervol: f32,
    },
    ToggleInputRecord {
        input: i32,
    },
    SetMidiEchoPort {
        echoport: i32,
    },
    SetMidiEchoChannel {
        echochannel: i32,
    },
    AdjustMidiTranspose {
        adjust: i32,
    },
    SetTriggerVolume {
        index: i32,
        vol: f32,
    },
    SlideLoopAmp {
        index: i32,
        slide: f32,
    },
    SetLoopAmp {
        index: i32,
        amp: f32,
    },
    AdjustLoopAmp {
        index: i32,
        ampfactor: f32,
    },
    ToggleSelectLoop {
        setid: i32,
        loopid: i32,
    },
    SelectOnlyPlayingLoops {
        setid: i32,
        playing: bool,
    },
    SelectAllLoops {
        setid: i32,
        select: bool,
    },
    InvertSelection {
        setid: i32,
    },
    TriggerSelectedLoops {
        setid: i32,
        vol: f32,
        toggleloops: bool,
    },
    SetSelectedLoopsTriggerVolume {
        setid: i32,
        vol: f32,
    },
    AdjustSelectedLoopsAmp {
        setid: i32,
        ampfactor: f32,
    },
    MoveLoop {
        oldloopid: i32,
        newloopid: i32,
    },
    EraseLoop {
        index: i32,
    },
    SaveLoop {
        index: i32,
    },
    RenameLoop {
        loopid: i32,
        in_layout: bool,
    },
    EraseSelectedLoops {
        setid: i32,
    },
    SetAutoLoopSaving {
        save: bool,
    },
    SetLoadLoopId {
        index: i32,
    },
    SetDefaultLoopPlacement {
        looprange: Range,
    },
    SelectPulse {
        pulse: i32,
    },
    DeletePulse {
        pulse: i32,
    },
    TapPulse {
        pulse: i32,
        newlen: bool,
    },
    SwitchMetronome {
        pulse: i32,
        metronome: bool,
    },
    SetSyncType {
        stype: bool,
    },
    SetSyncSpeed {
        sspd: i32,
    },
    SetMidiSync {
        midisync: i32,
    },
    SetVariable {
        var_name: Option<String>,
        value: UserVariable,
        maxjumpcheck: bool,
        maxjump: UserVariable,
    },
    ToggleVariable {
        var_name: Option<String>,
        maxvalue: i32,
        minvalue: i32,
    },
    SplitVariableMSBLSB {
        var: UserVariable,
        msb_name: Option<String>,
        lsb_name: Option<String>,
    },
    ParamSetGetAbsoluteParamIdx {
        interfaceid: i32,
        displayid: i32,
        paramidx: i32,
        absidx_name: Option<String>,
    },
    ParamSetGetParam {
        interfaceid: i32,
        displayid: i32,
        paramidx: i32,
        var_name: Option<String>,
    },
    ParamSetSetParam {
        interfaceid: i32,
        displayid: i32,
        paramidx: i32,
        value: f32,
    },
    LogFaderVolToLinear {
        var_name: Option<String>,
        fadervol: UserVariable,
        scale: f32,
    },
    ALSAMixerControlSet {
        hwid: i32,
        numid: i32,
        val1: i32,
        val2: i32,
        val3: i32,
        val4: i32,
    },
    CreateSnapshot {
        snapid: i32,
    },
    SwapSnapshots {
        snapid1: i32,
        snapid2: i32,
    },
    RenameSnapshot {
        snapid: i32,
    },
    TriggerSnapshot {
        snapid: i32,
    },
}

impl Event {
    pub fn get_type(&self) -> EventType {
        match self {
            // Unit variants
            Event::None => EventType::None,
            Event::StartSession => EventType::StartSession,
            Event::ExitSession => EventType::ExitSession,
            Event::SlideLoopAmpStopAll => EventType::SlideLoopAmpStopAll,
            Event::EraseAllLoops => EventType::EraseAllLoops,
            Event::ToggleDiskOutput => EventType::ToggleDiskOutput,
            Event::SaveNewScene => EventType::SaveNewScene,
            Event::SaveCurrentScene => EventType::SaveCurrentScene,
            Event::TransmitPlayingLoopsToDAW => EventType::TransmitPlayingLoopsToDAW,
            Event::PulseSync => EventType::PulseSync,
            Event::MIDITuneRequestInput => EventType::InputMIDITuneRequest,
            Event::MIDIActiveSensingInput => EventType::InputMIDIActiveSensing,
            Event::MIDIResetInput => EventType::InputMIDIReset,

            // Struct variants
            Event::EndRecord { .. } => EventType::EndRecord,
            Event::GoSub { .. } => EventType::GoSub,
            Event::KeyInput { .. } => EventType::InputKey,
            Event::LoopClicked { .. } => EventType::LoopClicked,
            Event::JoystickButtonInput { .. } => EventType::InputJoystickButton,
            Event::MouseButtonInput { .. } => EventType::InputMouseButton,
            Event::MouseMotionInput { .. } => EventType::InputMouseMotion,
            Event::TriggerLoop { .. } => EventType::TriggerLoop,
            Event::MIDIControllerInput { .. } => EventType::InputMIDIController,
            Event::MIDIChannelPressureInput { .. } => EventType::InputMIDIChannelPressure,
            Event::MIDIProgramChangeInput { .. } => EventType::InputMIDIProgramChange,
            Event::MIDIPitchBendInput { .. } => EventType::InputMIDIPitchBend,
            Event::MIDIPolyphonicPressureInput { .. } => EventType::InputMIDIPolyphonicPressure,
            Event::MIDISystemExclusiveInput { .. } => EventType::InputMIDISystemExclusive,
            Event::MIDITimeCodeQuarterFrameInput { .. } => EventType::InputMIDITimeCodeQuarterFrame,
            Event::MIDISongPositionInput { .. } => EventType::InputMIDISongPosition,
            Event::MIDISongSelectInput { .. } => EventType::InputMIDISongSelect,
            Event::MIDIKeyInput { .. } => EventType::InputMIDIKey,
            Event::MIDIClockInput { .. } => EventType::InputMIDIClock,
            Event::MIDIStartStopInput { .. } => EventType::InputMIDIStartStop,
            Event::FluidSynthEnable { .. } => EventType::FluidSynthEnable,
            Event::SetMidiTuning { .. } => EventType::SetMidiTuning,
            Event::BrowserMoveToItem { .. } => EventType::BrowserMoveToItem,
            Event::BrowserMoveToItemAbsolute { .. } => EventType::BrowserMoveToItemAbsolute,
            Event::BrowserSelectItem { .. } => EventType::BrowserSelectItem,
            Event::BrowserRenameItem { .. } => EventType::BrowserRenameItem,
            Event::BrowserItemBrowsed { .. } => EventType::BrowserItemBrowsed,
            Event::PatchBrowserMoveToBank { .. } => EventType::PatchBrowserMoveToBank,
            Event::PatchBrowserMoveToBankByIndex { .. } => EventType::PatchBrowserMoveToBankByIndex,
            Event::StartInterface { .. } => EventType::StartInterface,
            Event::VideoShowLayout { .. } => EventType::VideoShowLayout,
            Event::VideoSwitchInterface { .. } => EventType::VideoSwitchInterface,
            Event::VideoShowDisplay { .. } => EventType::VideoShowDisplay,
            Event::VideoShowHelp { .. } => EventType::VideoShowHelp,
            Event::VideoFullScreen { .. } => EventType::VideoFullScreen,
            Event::ShowDebugInfo { .. } => EventType::ShowDebugInfo,
            Event::VideoShowLoop { .. } => EventType::VideoShowLoop,
            Event::VideoShowSnapshotPage { .. } => EventType::VideoShowSnapshotPage,
            Event::VideoShowParamSetBank { .. } => EventType::VideoShowParamSetBank,
            Event::VideoShowParamSetPage { .. } => EventType::VideoShowParamSetPage,
            Event::SlideMasterInVolume { .. } => EventType::SlideMasterInVolume,
            Event::SlideMasterOutVolume { .. } => EventType::SlideMasterOutVolume,
            Event::SlideInVolume { .. } => EventType::SlideInVolume,
            Event::SetMasterInVolume { .. } => EventType::SetMasterInVolume,
            Event::SetMasterOutVolume { .. } => EventType::SetMasterOutVolume,
            Event::SetInVolume { .. } => EventType::SetInVolume,
            Event::ToggleInputRecord { .. } => EventType::ToggleInputRecord,
            Event::SetMidiEchoPort { .. } => EventType::SetMidiEchoPort,
            Event::SetMidiEchoChannel { .. } => EventType::SetMidiEchoChannel,
            Event::AdjustMidiTranspose { .. } => EventType::AdjustMidiTranspose,
            Event::SetTriggerVolume { .. } => EventType::SetTriggerVolume,
            Event::SlideLoopAmp { .. } => EventType::SlideLoopAmp,
            Event::SetLoopAmp { .. } => EventType::SetLoopAmp,
            Event::AdjustLoopAmp { .. } => EventType::AdjustLoopAmp,
            Event::ToggleSelectLoop { .. } => EventType::ToggleSelectLoop,
            Event::SelectOnlyPlayingLoops { .. } => EventType::SelectOnlyPlayingLoops,
            Event::SelectAllLoops { .. } => EventType::SelectAllLoops,
            Event::InvertSelection { .. } => EventType::InvertSelection,
            Event::TriggerSelectedLoops { .. } => EventType::TriggerSelectedLoops,
            Event::SetSelectedLoopsTriggerVolume { .. } => EventType::SetSelectedLoopsTriggerVolume,
            Event::AdjustSelectedLoopsAmp { .. } => EventType::AdjustSelectedLoopsAmp,
            Event::MoveLoop { .. } => EventType::MoveLoop,
            Event::EraseLoop { .. } => EventType::EraseLoop,
            Event::SaveLoop { .. } => EventType::SaveLoop,
            Event::RenameLoop { .. } => EventType::RenameLoop,
            Event::EraseSelectedLoops { .. } => EventType::EraseSelectedLoops,
            Event::SetAutoLoopSaving { .. } => EventType::SetAutoLoopSaving,
            Event::SetLoadLoopId { .. } => EventType::SetLoadLoopId,
            Event::SetDefaultLoopPlacement { .. } => EventType::SetDefaultLoopPlacement,
            Event::SelectPulse { .. } => EventType::SelectPulse,
            Event::DeletePulse { .. } => EventType::DeletePulse,
            Event::TapPulse { .. } => EventType::TapPulse,
            Event::SwitchMetronome { .. } => EventType::SwitchMetronome,
            Event::SetSyncType { .. } => EventType::SetSyncType,
            Event::SetSyncSpeed { .. } => EventType::SetSyncSpeed,
            Event::SetMidiSync { .. } => EventType::SetMidiSync,
            Event::SetVariable { .. } => EventType::SetVariable,
            Event::ToggleVariable { .. } => EventType::ToggleVariable,
            Event::SplitVariableMSBLSB { .. } => EventType::SplitVariableMSBLSB,
            Event::ParamSetGetAbsoluteParamIdx { .. } => EventType::ParamSetGetAbsoluteParamIdx,
            Event::ParamSetGetParam { .. } => EventType::ParamSetGetParam,
            Event::ParamSetSetParam { .. } => EventType::ParamSetSetParam,
            Event::LogFaderVolToLinear { .. } => EventType::LogFaderVolToLinear,
            Event::ALSAMixerControlSet { .. } => EventType::ALSAMixerControlSet,
            Event::CreateSnapshot { .. } => EventType::CreateSnapshot,
            Event::SwapSnapshots { .. } => EventType::SwapSnapshots,
            Event::RenameSnapshot { .. } => EventType::RenameSnapshot,
            Event::TriggerSnapshot { .. } => EventType::TriggerSnapshot,
        }
    }

    pub fn get_num_params(&self) -> usize {
        EventType::parameters(self.get_type()).len()
    }

    pub fn get_param(&self, idx: usize) -> Option<EventParameter> {
        EventType::parameters(self.get_type()).get(idx).copied()
    }
}

// ============================================================
// Event manager
// ============================================================

struct ListenerEntry {
    listener: Mutex<Box<dyn EventListener>>,
    _from: Option<String>,
}

pub struct EventManager {
    listeners: Arc<Mutex<HashMap<EventType, Vec<ListenerEntry>>>>,
    queue: Arc<Mutex<VecDeque<Event>>>,
    capacity: usize,
    dropped: Arc<std::sync::atomic::AtomicU64>,
    lock: Arc<(Mutex<bool>, Condvar)>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl EventManager {
    pub fn new() -> Self {
        Self::with_capacity(1024)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let lock = Arc::new((Mutex::new(false), Condvar::new()));

        let worker_lock = lock.clone();
        let worker_running = running.clone();
        let worker_queue = Arc::new(Mutex::new(VecDeque::<Event>::new()));
        let dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let worker_listeners: Arc<Mutex<HashMap<EventType, Vec<ListenerEntry>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let worker_queue_ref = worker_queue.clone();
        let worker_listeners_ref = worker_listeners.clone();
        let worker = thread::Builder::new()
            .stack_size(128 * 1024)
            .name("event-dispatch".into())
            .spawn(move || {
                while worker_running.load(Ordering::Acquire) {
                    let (state, wake) = &*worker_lock;
                    let mut ready = state.lock().unwrap();
                    while !*ready && worker_running.load(Ordering::Acquire) {
                        ready = wake.wait(ready).unwrap();
                    }
                    *ready = false;
                    drop(ready);
                    let events = std::mem::take(&mut *worker_queue_ref.lock().unwrap());
                    let lists = worker_listeners_ref.lock().unwrap();
                    for ev in events {
                        if let Some(entries) = lists.get(&ev.get_type()) {
                            for entry in entries {
                                if let Ok(mut listener) = entry.listener.lock() {
                                    listener.receive_event(&ev, &());
                                }
                            }
                        }
                    }
                }
            })
            .expect("event dispatch thread");

        // The worker thread and process_pending() both drain the same queue.
        // In production the main thread calls process_pending() from the SDL
        // event loop, making the worker redundant.  The worker is preserved
        // for C++ lifecycle compatibility; remove it once the inline dispatch
        // path is confirmed sufficient in all deployment scenarios.

        EventManager {
            listeners: worker_listeners,
            queue: worker_queue,
            capacity: capacity.max(1),
            dropped,
            lock,
            running,
            worker: Some(worker),
        }
    }

    pub fn listen(&self, listener: Box<dyn EventListener>, typ: EventType) {
        self.listeners
            .lock()
            .unwrap()
            .entry(typ)
            .or_default()
            .push(ListenerEntry {
                listener: Mutex::new(listener),
                _from: None,
            });
    }

    /// Enqueue without waiting for delivery. The newest event is rejected
    /// when the bounded queue is full.
    #[allow(clippy::result_large_err)]
    pub fn try_post_event(&self, ev: Event) -> Result<(), Event> {
        let mut queue = self.queue.lock().unwrap();
        if queue.len() >= self.capacity {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return Err(ev);
        }
        queue.push_back(ev);
        drop(queue);
        let (m, cv) = &*self.lock;
        *m.lock().unwrap() = true;
        cv.notify_one();
        Ok(())
    }

    pub fn post_event(&self, ev: Event) {
        let _ = self.try_post_event(ev);
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn process_pending(&self) {
        let lists = self.listeners.lock().unwrap();
        let mut q = self.queue.lock().unwrap();
        while let Some(ev) = q.pop_front() {
            let typ = ev.get_type();
            if let Some(entries) = lists.get(&typ) {
                for entry in entries {
                    if let Ok(mut listener) = entry.listener.lock() {
                        let stub: () = ();
                        listener.receive_event(&ev, &stub);
                    }
                }
            }
        }
    }
}

impl Default for EventManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EventManager {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        let (m, cv) = &*self.lock;
        drop(m.lock().unwrap());
        cv.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

