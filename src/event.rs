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
    fn receive_event(&mut self, ev: Box<dyn Event>, from: &dyn EventProducer);
}

// ============================================================
// Event trait + concrete events
// ============================================================

pub trait Event: Send {
    fn get_type(&self) -> EventType;
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
    fn clone_box(&self) -> Box<dyn Event>;
    fn get_num_params(&self) -> usize {
        0
    }
    fn get_param(&self, _idx: usize) -> Option<EventParameter> {
        None
    }
}macro_rules! impl_event {
    // Without event parameters
    ($name:ident, $type:ident) => {
        impl Event for $name {
            fn get_type(&self) -> EventType { EventType::$type }
            fn as_any(&self) -> &dyn std::any::Any { self }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
            fn clone_box(&self) -> Box<dyn Event> { Box::new(self.clone()) }
        }
    };
    // With event parameters (references a const array)
    ($name:ident, $type:ident, $params:expr) => {
        impl Event for $name {
            fn get_type(&self) -> EventType { EventType::$type }
            fn as_any(&self) -> &dyn std::any::Any { self }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
            fn clone_box(&self) -> Box<dyn Event> { Box::new(self.clone()) }
            fn get_num_params(&self) -> usize { $params.len() }
            fn get_param(&self, idx: usize) -> Option<EventParameter> { $params.get(idx).copied() }
        }
    };
}

#[derive(Clone)]
pub struct BaseEvent {
    pub event_type: EventType,
    pub timestamp: f64,
}

impl Event for BaseEvent {
    fn get_type(&self) -> EventType {
        self.event_type
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn clone_box(&self) -> Box<dyn Event> {
        Box::new(self.clone())
    }
}

#[derive(Clone)]
pub struct EndRecordEvent {
    pub base: BaseEvent,
    pub keeprecord: bool,
}

impl EndRecordEvent {
    pub fn new(keeprecord: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::EndRecord,
                timestamp: 0.0,
            },
            keeprecord,
        }
    }
}

impl_event!(EndRecordEvent, EndRecord);

#[derive(Clone)]
pub struct GoSubEvent {
    pub base: BaseEvent,
    pub sub: i32,
    pub param1: f32,
    pub param2: f32,
    pub param3: f32,
}

impl GoSubEvent {
    pub fn new(sub: i32, param1: f32, param2: f32, param3: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::GoSub,
                timestamp: 0.0,
            },
            sub,
            param1,
            param2,
            param3,
        }
    }
}

impl_event!(GoSubEvent, GoSub);

#[derive(Clone)]
pub struct KeyInputEvent {
    pub base: BaseEvent,
    pub down: bool,
    pub keysym: i32,
    pub unicode: i32,
}

impl KeyInputEvent {
    pub fn new(down: bool, keysym: i32, unicode: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputKey,
                timestamp: 0.0,
            },
            down,
            keysym,
            unicode,
        }
    }
}

impl_event!(KeyInputEvent, InputKey, KEY_INPUT_PARAMS);

#[derive(Clone)]
pub struct LoopClickedEvent {
    pub base: BaseEvent,
    pub down: bool,
    pub button: i32,
    pub loopid: i32,
    pub in_layout: bool,
}

impl LoopClickedEvent {
    pub fn new(down: bool, button: i32, loopid: i32, in_layout: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::LoopClicked,
                timestamp: 0.0,
            },
            down,
            button,
            loopid,
            in_layout,
        }
    }
}

impl_event!(LoopClickedEvent, LoopClicked);

#[derive(Clone)]
pub struct JoystickButtonInputEvent {
    pub base: BaseEvent,
    pub down: bool,
    pub button: i32,
    pub joystick: i32,
}

impl JoystickButtonInputEvent {
    pub fn new(down: bool, button: i32, joystick: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputJoystickButton,
                timestamp: 0.0,
            },
            down,
            button,
            joystick,
        }
    }
}

impl_event!(JoystickButtonInputEvent, InputJoystickButton, JOYSTICK_BUTTON_INPUT_PARAMS);

#[derive(Clone)]
pub struct MouseButtonInputEvent {
    pub base: BaseEvent,
    pub down: bool,
    pub button: i32,
    pub x: i32,
    pub y: i32,
}

impl MouseButtonInputEvent {
    pub fn new(down: bool, button: i32, x: i32, y: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMouseButton,
                timestamp: 0.0,
            },
            down,
            button,
            x,
            y,
        }
    }
}

impl_event!(MouseButtonInputEvent, InputMouseButton, MOUSE_BUTTON_INPUT_PARAMS);

#[derive(Clone)]
pub struct MouseMotionInputEvent {
    pub base: BaseEvent,
    pub x: i32,
    pub y: i32,
}

impl MouseMotionInputEvent {
    pub fn new(x: i32, y: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMouseMotion,
                timestamp: 0.0,
            },
            x,
            y,
        }
    }
}

impl_event!(MouseMotionInputEvent, InputMouseMotion, MOUSE_MOTION_INPUT_PARAMS);

#[derive(Clone)]
pub struct TriggerLoopEvent {
    pub base: BaseEvent,
    pub index: i32,
    pub vol: f32,
    pub engage: i32,
    pub shot: bool,
    pub overdub: bool,
}
impl TriggerLoopEvent {
    pub fn new(index: i32, vol: f32) -> Self {
        TriggerLoopEvent {
            base: BaseEvent {
                event_type: EventType::TriggerLoop,
                timestamp: 0.0,
            },
            index,
            vol,
            engage: -1,
            shot: false,
            overdub: false,
        }
    }
}
impl_event!(TriggerLoopEvent, TriggerLoop, TRIGGER_LOOP_PARAMS);

#[derive(Clone)]
pub struct MIDIControllerInputEvent {
    pub base: BaseEvent,
    pub outport: i32,
    pub channel: u8,
    pub ctrl: u8,
    pub val: u8,
    pub echo: bool,
}

impl MIDIControllerInputEvent {
    pub fn new(channel: u8, ctrl: u8, val: u8) -> Self {
        Self::with_route(1, channel, ctrl, val, false)
    }

    pub fn with_route(outport: i32, channel: u8, ctrl: u8, val: u8, echo: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMIDIController,
                timestamp: 0.0,
            },
            outport,
            channel,
            ctrl,
            val,
            echo,
        }
    }
}

impl_event!(MIDIControllerInputEvent, InputMIDIController, MIDI_CONTROLLER_INPUT_PARAMS);

#[derive(Clone)]
pub struct MIDIChannelPressureInputEvent {
    pub base: BaseEvent,
    pub outport: i32,
    pub channel: u8,
    pub val: u8,
    pub echo: bool,
}

impl MIDIChannelPressureInputEvent {
    pub fn new(outport: i32, channel: u8, val: u8, echo: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMIDIChannelPressure,
                timestamp: 0.0,
            },
            outport,
            channel,
            val,
            echo,
        }
    }
}

impl_event!(MIDIChannelPressureInputEvent, InputMIDIChannelPressure, MIDI_CHANNEL_PRESSURE_INPUT_PARAMS);

#[derive(Clone)]
pub struct MIDIProgramChangeInputEvent {
    pub base: BaseEvent,
    pub outport: i32,
    pub channel: u8,
    pub val: u8,
    pub echo: bool,
}

impl MIDIProgramChangeInputEvent {
    pub fn new(outport: i32, channel: u8, val: u8, echo: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMIDIProgramChange,
                timestamp: 0.0,
            },
            outport,
            channel,
            val,
            echo,
        }
    }
}

impl_event!(MIDIProgramChangeInputEvent, InputMIDIProgramChange, MIDI_PROGRAM_CHANGE_INPUT_PARAMS);

#[derive(Clone)]
pub struct MIDIPitchBendInputEvent {
    pub base: BaseEvent,
    pub outport: i32,
    pub channel: u8,
    pub val: i32,
    pub echo: bool,
}

impl MIDIPitchBendInputEvent {
    pub fn new(channel: u8, val: i32) -> Self {
        Self::with_route(1, channel, val, false)
    }

    pub fn with_route(outport: i32, channel: u8, val: i32, echo: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMIDIPitchBend,
                timestamp: 0.0,
            },
            outport,
            channel,
            val,
            echo,
        }
    }
}

impl_event!(MIDIPitchBendInputEvent, InputMIDIPitchBend, MIDI_PITCH_BEND_INPUT_PARAMS);

#[derive(Clone)]
pub struct MIDIPolyphonicPressureInputEvent {
    pub base: BaseEvent,
    pub channel: u8,
    pub notenum: u8,
    pub val: u8,
}

impl MIDIPolyphonicPressureInputEvent {
    pub fn new(channel: u8, notenum: u8, val: u8) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMIDIPolyphonicPressure,
                timestamp: 0.0,
            },
            channel,
            notenum,
            val,
        }
    }
}

impl_event!(MIDIPolyphonicPressureInputEvent, InputMIDIPolyphonicPressure, MIDI_POLYPHONIC_PRESSURE_INPUT_PARAMS);

#[derive(Clone)]
pub struct MIDISystemExclusiveInputEvent {
    pub base: BaseEvent,
    /// Complete message, including the F0/F7 framing bytes.
    pub bytes: Vec<u8>,
}

impl MIDISystemExclusiveInputEvent {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMIDISystemExclusive,
                timestamp: 0.0,
            },
            bytes,
        }
    }
}

impl_event!(MIDISystemExclusiveInputEvent, InputMIDISystemExclusive);

macro_rules! midi_value_input_event {
    ($name:ident, $event_type:ident) => {
        #[derive(Clone)]
        pub struct $name {
            pub base: BaseEvent,
            pub value: u16,
        }
        impl $name {
            pub fn new(value: u16) -> Self {
                Self {
                    base: BaseEvent {
                        event_type: EventType::$event_type,
                        timestamp: 0.0,
                    },
                    value,
                }
            }
        }
        impl Event for $name {
            fn get_type(&self) -> EventType {
                EventType::$event_type
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
            fn clone_box(&self) -> Box<dyn Event> {
                Box::new(self.clone())
            }
            fn get_num_params(&self) -> usize {
                1
            }
            fn get_param(&self, idx: usize) -> Option<EventParameter> {
                MIDI_VALUE_INPUT_PARAMS.get(idx).copied()
            }
        }
    };
}

midi_value_input_event!(
    MIDITimeCodeQuarterFrameInputEvent,
    InputMIDITimeCodeQuarterFrame
);
midi_value_input_event!(MIDISongPositionInputEvent, InputMIDISongPosition);
midi_value_input_event!(MIDISongSelectInputEvent, InputMIDISongSelect);

macro_rules! midi_unit_input_event {
    ($name:ident, $event_type:ident) => {
        #[derive(Clone)]
        pub struct $name {
            pub base: BaseEvent,
        }
        impl $name {
            pub fn new() -> Self {
                Self {
                    base: BaseEvent {
                        event_type: EventType::$event_type,
                        timestamp: 0.0,
                    },
                }
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl Event for $name {
            fn get_type(&self) -> EventType {
                EventType::$event_type
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
            fn clone_box(&self) -> Box<dyn Event> {
                Box::new(self.clone())
            }
        }
    };
}

midi_unit_input_event!(MIDITuneRequestInputEvent, InputMIDITuneRequest);
midi_unit_input_event!(MIDIActiveSensingInputEvent, InputMIDIActiveSensing);
midi_unit_input_event!(MIDIResetInputEvent, InputMIDIReset);

#[derive(Clone)]
pub struct MIDIKeyInputEvent {
    pub base: BaseEvent,
    pub outport: i32,
    pub channel: u8,
    pub notenum: u8,
    pub vel: u8,
    pub down: bool,
    pub echo: bool,
}

impl MIDIKeyInputEvent {
    pub fn new(channel: u8, notenum: u8, vel: u8, down: bool) -> Self {
        Self::with_route(1, channel, notenum, vel, down, false)
    }

    pub fn with_route(
        outport: i32,
        channel: u8,
        notenum: u8,
        vel: u8,
        down: bool,
        echo: bool,
    ) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMIDIKey,
                timestamp: 0.0,
            },
            outport,
            channel,
            notenum,
            vel,
            down,
            echo,
        }
    }
}

impl_event!(MIDIKeyInputEvent, InputMIDIKey, MIDI_KEY_INPUT_PARAMS);

#[derive(Clone)]
pub struct MIDIClockInputEvent {
    pub base: BaseEvent,
    pub outport: i32,
}

impl MIDIClockInputEvent {
    pub fn new() -> Self {
        Self::with_outport(1)
    }

    pub fn with_outport(outport: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMIDIClock,
                timestamp: 0.0,
            },
            outport,
        }
    }
}

impl_event!(MIDIClockInputEvent, InputMIDIClock, MIDI_CLOCK_INPUT_PARAMS);

#[derive(Clone)]
pub struct MIDIStartStopInputEvent {
    pub base: BaseEvent,
    pub outport: i32,
    pub start: bool,
}

impl MIDIStartStopInputEvent {
    pub fn new(start: bool) -> Self {
        Self::with_outport(1, start)
    }

    pub fn with_outport(outport: i32, start: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InputMIDIStartStop,
                timestamp: 0.0,
            },
            outport,
            start,
        }
    }
}

impl_event!(MIDIStartStopInputEvent, InputMIDIStartStop, MIDI_START_STOP_INPUT_PARAMS);

#[derive(Clone)]
pub struct FluidSynthEnableEvent {
    pub base: BaseEvent,
    pub enable: bool,
}

impl FluidSynthEnableEvent {
    pub fn new(enable: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::FluidSynthEnable,
                timestamp: 0.0,
            },
            enable,
        }
    }
}

impl_event!(FluidSynthEnableEvent, FluidSynthEnable);

#[derive(Clone)]
pub struct SetMidiTuningEvent {
    pub base: BaseEvent,
    pub tuning: f32,
}

impl SetMidiTuningEvent {
    pub fn new(tuning: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetMidiTuning,
                timestamp: 0.0,
            },
            tuning,
        }
    }
}

impl_event!(SetMidiTuningEvent, SetMidiTuning);

#[derive(Clone)]
pub struct BrowserMoveToItemEvent {
    pub base: BaseEvent,
    pub browserid: i32,
    pub adjust: i32,
    pub jump_adjust: i32,
}

impl BrowserMoveToItemEvent {
    pub fn new(browserid: i32, adjust: i32, jump_adjust: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::BrowserMoveToItem,
                timestamp: 0.0,
            },
            browserid,
            adjust,
            jump_adjust,
        }
    }
}

impl_event!(BrowserMoveToItemEvent, BrowserMoveToItem, BROWSER_MOVE_PARAMS);

#[derive(Clone)]
pub struct BrowserMoveToItemAbsoluteEvent {
    pub base: BaseEvent,
    pub browserid: i32,
    pub index: i32,
}

impl BrowserMoveToItemAbsoluteEvent {
    pub fn new(browserid: i32, index: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::BrowserMoveToItemAbsolute,
                timestamp: 0.0,
            },
            browserid,
            index,
        }
    }
}

impl_event!(BrowserMoveToItemAbsoluteEvent, BrowserMoveToItemAbsolute, BROWSER_MOVE_ABSOLUTE_PARAMS);

#[derive(Clone)]
pub struct BrowserSelectItemEvent {
    pub base: BaseEvent,
    pub browserid: i32,
}

impl BrowserSelectItemEvent {
    pub fn new(browserid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::BrowserSelectItem,
                timestamp: 0.0,
            },
            browserid,
        }
    }
}

impl_event!(BrowserSelectItemEvent, BrowserSelectItem);

#[derive(Clone)]
pub struct BrowserRenameItemEvent {
    pub base: BaseEvent,
    pub browserid: i32,
}

impl BrowserRenameItemEvent {
    pub fn new(browserid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::BrowserRenameItem,
                timestamp: 0.0,
            },
            browserid,
        }
    }
}

impl_event!(BrowserRenameItemEvent, BrowserRenameItem);

#[derive(Clone)]
pub struct BrowserItemBrowsedEvent {
    pub base: BaseEvent,
    pub browserid: i32,
}

impl BrowserItemBrowsedEvent {
    pub fn new(browserid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::BrowserItemBrowsed,
                timestamp: 0.0,
            },
            browserid,
        }
    }
}

impl_event!(BrowserItemBrowsedEvent, BrowserItemBrowsed);

#[derive(Clone)]
pub struct PatchBrowserMoveToBankEvent {
    pub base: BaseEvent,
    pub direction: i32,
}

impl PatchBrowserMoveToBankEvent {
    pub fn new(direction: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::PatchBrowserMoveToBank,
                timestamp: 0.0,
            },
            direction,
        }
    }
}

impl_event!(PatchBrowserMoveToBankEvent, PatchBrowserMoveToBank);

#[derive(Clone)]
pub struct PatchBrowserMoveToBankByIndexEvent {
    pub base: BaseEvent,
    pub index: i32,
}

impl PatchBrowserMoveToBankByIndexEvent {
    pub fn new(index: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::PatchBrowserMoveToBankByIndex,
                timestamp: 0.0,
            },
            index,
        }
    }
}

impl_event!(PatchBrowserMoveToBankByIndexEvent, PatchBrowserMoveToBankByIndex);

#[derive(Clone)]
pub struct StartSessionEvent {
    pub base: BaseEvent,
}

impl StartSessionEvent {
    pub fn new() -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::StartSession,
                timestamp: 0.0,
            },
        }
    }
}

impl_event!(StartSessionEvent, StartSession);

#[derive(Clone)]
pub struct StartInterfaceEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
}

impl StartInterfaceEvent {
    pub fn new(interfaceid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::StartInterface,
                timestamp: 0.0,
            },
            interfaceid,
        }
    }
}

impl_event!(StartInterfaceEvent, StartInterface, START_INTERFACE_PARAMS);

#[derive(Clone)]
pub struct VideoShowLayoutEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
    pub layoutid: i32,
    pub show: bool,
    pub hideothers: bool,
}

impl VideoShowLayoutEvent {
    pub fn new(interfaceid: i32, layoutid: i32, show: bool, hideothers: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::VideoShowLayout,
                timestamp: 0.0,
            },
            interfaceid,
            layoutid,
            show,
            hideothers,
        }
    }
}

impl_event!(VideoShowLayoutEvent, VideoShowLayout);

#[derive(Clone)]
pub struct VideoSwitchInterfaceEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
}

impl VideoSwitchInterfaceEvent {
    pub fn new(interfaceid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::VideoSwitchInterface,
                timestamp: 0.0,
            },
            interfaceid,
        }
    }
}

impl_event!(VideoSwitchInterfaceEvent, VideoSwitchInterface);

#[derive(Clone)]
pub struct VideoShowDisplayEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
    pub displayid: i32,
    pub show: bool,
}

impl VideoShowDisplayEvent {
    pub fn new(interfaceid: i32, displayid: i32, show: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::VideoShowDisplay,
                timestamp: 0.0,
            },
            interfaceid,
            displayid,
            show,
        }
    }
}

impl_event!(VideoShowDisplayEvent, VideoShowDisplay);

#[derive(Clone)]
pub struct VideoShowHelpEvent {
    pub base: BaseEvent,
    pub page: i32,
}

impl VideoShowHelpEvent {
    pub fn new(page: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::VideoShowHelp,
                timestamp: 0.0,
            },
            page,
        }
    }
}

impl_event!(VideoShowHelpEvent, VideoShowHelp);

#[derive(Clone)]
pub struct VideoFullScreenEvent {
    pub base: BaseEvent,
    pub fullscreen: bool,
}

impl VideoFullScreenEvent {
    pub fn new(fullscreen: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::VideoFullScreen,
                timestamp: 0.0,
            },
            fullscreen,
        }
    }
}

impl_event!(VideoFullScreenEvent, VideoFullScreen);

#[derive(Clone)]
pub struct ShowDebugInfoEvent {
    pub base: BaseEvent,
    pub show: bool,
}

impl ShowDebugInfoEvent {
    pub fn new(show: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::ShowDebugInfo,
                timestamp: 0.0,
            },
            show,
        }
    }
}

impl_event!(ShowDebugInfoEvent, ShowDebugInfo);

#[derive(Clone)]
pub struct VideoShowLoopEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
    pub layoutid: i32,
    pub loopid: Range,
}

impl VideoShowLoopEvent {
    pub fn new(interfaceid: i32, layoutid: i32, loopid: Range) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::VideoShowLoop,
                timestamp: 0.0,
            },
            interfaceid,
            layoutid,
            loopid,
        }
    }
}

impl_event!(VideoShowLoopEvent, VideoShowLoop);

#[derive(Clone)]
pub struct VideoShowSnapshotPageEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
    pub displayid: i32,
    pub page: i32,
}

impl VideoShowSnapshotPageEvent {
    pub fn new(interfaceid: i32, displayid: i32, page: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::VideoShowSnapshotPage,
                timestamp: 0.0,
            },
            interfaceid,
            displayid,
            page,
        }
    }
}

impl_event!(VideoShowSnapshotPageEvent, VideoShowSnapshotPage);

#[derive(Clone)]
pub struct VideoShowParamSetBankEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
    pub displayid: i32,
    pub bank: i32,
}

impl VideoShowParamSetBankEvent {
    pub fn new(interfaceid: i32, displayid: i32, bank: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::VideoShowParamSetBank,
                timestamp: 0.0,
            },
            interfaceid,
            displayid,
            bank,
        }
    }
}

impl_event!(VideoShowParamSetBankEvent, VideoShowParamSetBank);

#[derive(Clone)]
pub struct VideoShowParamSetPageEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
    pub displayid: i32,
    pub page: i32,
}

impl VideoShowParamSetPageEvent {
    pub fn new(interfaceid: i32, displayid: i32, page: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::VideoShowParamSetPage,
                timestamp: 0.0,
            },
            interfaceid,
            displayid,
            page,
        }
    }
}

impl_event!(VideoShowParamSetPageEvent, VideoShowParamSetPage);

#[derive(Clone)]
pub struct ExitSessionEvent {
    pub base: BaseEvent,
}

impl ExitSessionEvent {
    pub fn new() -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::ExitSession,
                timestamp: 0.0,
            },
        }
    }
}

impl_event!(ExitSessionEvent, ExitSession);

#[derive(Clone)]
pub struct SlideMasterInVolumeEvent {
    pub base: BaseEvent,
    pub slide: f32,
}

impl SlideMasterInVolumeEvent {
    pub fn new(slide: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SlideMasterInVolume,
                timestamp: 0.0,
            },
            slide,
        }
    }
}

impl_event!(SlideMasterInVolumeEvent, SlideMasterInVolume);

#[derive(Clone)]
pub struct SlideMasterOutVolumeEvent {
    pub base: BaseEvent,
    pub slide: f32,
}

impl SlideMasterOutVolumeEvent {
    pub fn new(slide: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SlideMasterOutVolume,
                timestamp: 0.0,
            },
            slide,
        }
    }
}

impl_event!(SlideMasterOutVolumeEvent, SlideMasterOutVolume);

#[derive(Clone)]
pub struct SlideInVolumeEvent {
    pub base: BaseEvent,
    pub input: i32,
    pub slide: f32,
}

impl SlideInVolumeEvent {
    pub fn new(input: i32, slide: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SlideInVolume,
                timestamp: 0.0,
            },
            input,
            slide,
        }
    }
}

impl_event!(SlideInVolumeEvent, SlideInVolume, SLIDE_IN_VOLUME_PARAMS);

#[derive(Clone)]
pub struct SetMasterInVolumeEvent {
    pub base: BaseEvent,
    pub vol: f32,
    pub fadervol: f32,
}

impl SetMasterInVolumeEvent {
    pub fn new(vol: f32, fadervol: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetMasterInVolume,
                timestamp: 0.0,
            },
            vol,
            fadervol,
        }
    }
}

impl_event!(SetMasterInVolumeEvent, SetMasterInVolume);

#[derive(Clone)]
pub struct SetMasterOutVolumeEvent {
    pub base: BaseEvent,
    pub vol: f32,
    pub fadervol: f32,
}

impl SetMasterOutVolumeEvent {
    pub fn new(vol: f32, fadervol: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetMasterOutVolume,
                timestamp: 0.0,
            },
            vol,
            fadervol,
        }
    }
}

impl_event!(SetMasterOutVolumeEvent, SetMasterOutVolume);

#[derive(Clone)]
pub struct SetInVolumeEvent {
    pub base: BaseEvent,
    pub input: i32,
    pub vol: f32,
    pub fadervol: f32,
}

impl SetInVolumeEvent {
    pub fn new(input: i32, vol: f32, fadervol: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetInVolume,
                timestamp: 0.0,
            },
            input,
            vol,
            fadervol,
        }
    }
}

impl_event!(SetInVolumeEvent, SetInVolume, SET_IN_VOLUME_PARAMS);

#[derive(Clone)]
pub struct ToggleInputRecordEvent {
    pub base: BaseEvent,
    pub input: i32,
}

impl ToggleInputRecordEvent {
    pub fn new(input: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::ToggleInputRecord,
                timestamp: 0.0,
            },
            input,
        }
    }
}

impl_event!(ToggleInputRecordEvent, ToggleInputRecord);

#[derive(Clone)]
pub struct SetMidiEchoPortEvent {
    pub base: BaseEvent,
    pub echoport: i32,
}

impl SetMidiEchoPortEvent {
    pub fn new(echoport: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetMidiEchoPort,
                timestamp: 0.0,
            },
            echoport,
        }
    }
}

impl_event!(SetMidiEchoPortEvent, SetMidiEchoPort);

#[derive(Clone)]
pub struct SetMidiEchoChannelEvent {
    pub base: BaseEvent,
    pub echochannel: i32,
}

impl SetMidiEchoChannelEvent {
    pub fn new(echochannel: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetMidiEchoChannel,
                timestamp: 0.0,
            },
            echochannel,
        }
    }
}

impl_event!(SetMidiEchoChannelEvent, SetMidiEchoChannel);

#[derive(Clone)]
pub struct AdjustMidiTransposeEvent {
    pub base: BaseEvent,
    pub adjust: i32,
}

impl AdjustMidiTransposeEvent {
    pub fn new(adjust: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::AdjustMidiTranspose,
                timestamp: 0.0,
            },
            adjust,
        }
    }
}

impl_event!(AdjustMidiTransposeEvent, AdjustMidiTranspose);

#[derive(Clone)]
pub struct SetTriggerVolumeEvent {
    pub base: BaseEvent,
    pub index: i32,
    pub vol: f32,
}

impl SetTriggerVolumeEvent {
    pub fn new(index: i32, vol: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetTriggerVolume,
                timestamp: 0.0,
            },
            index,
            vol,
        }
    }
}

impl_event!(SetTriggerVolumeEvent, SetTriggerVolume);

#[derive(Clone)]
pub struct SlideLoopAmpEvent {
    pub base: BaseEvent,
    pub index: i32,
    pub slide: f32,
}

impl SlideLoopAmpEvent {
    pub fn new(index: i32, slide: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SlideLoopAmp,
                timestamp: 0.0,
            },
            index,
            slide,
        }
    }
}

impl_event!(SlideLoopAmpEvent, SlideLoopAmp, SLIDE_LOOP_AMP_PARAMS);

#[derive(Clone)]
pub struct SetLoopAmpEvent {
    pub base: BaseEvent,
    pub index: i32,
    pub amp: f32,
}

impl SetLoopAmpEvent {
    pub fn new(index: i32, amp: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetLoopAmp,
                timestamp: 0.0,
            },
            index,
            amp,
        }
    }
}

impl_event!(SetLoopAmpEvent, SetLoopAmp, SET_LOOP_AMP_PARAMS);

#[derive(Clone)]
pub struct AdjustLoopAmpEvent {
    pub base: BaseEvent,
    pub index: i32,
    pub ampfactor: f32,
}

impl AdjustLoopAmpEvent {
    pub fn new(index: i32, ampfactor: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::AdjustLoopAmp,
                timestamp: 0.0,
            },
            index,
            ampfactor,
        }
    }
}

impl_event!(AdjustLoopAmpEvent, AdjustLoopAmp, ADJUST_LOOP_AMP_PARAMS);

#[derive(Clone)]
pub struct SlideLoopAmpStopAllEvent {
    pub base: BaseEvent,
}

impl SlideLoopAmpStopAllEvent {
    pub fn new() -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SlideLoopAmpStopAll,
                timestamp: 0.0,
            },
        }
    }
}

impl_event!(SlideLoopAmpStopAllEvent, SlideLoopAmpStopAll);

#[derive(Clone)]
pub struct ToggleSelectLoopEvent {
    pub base: BaseEvent,
    pub setid: i32,
    pub loopid: i32,
}

impl ToggleSelectLoopEvent {
    pub fn new(setid: i32, loopid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::ToggleSelectLoop,
                timestamp: 0.0,
            },
            setid,
            loopid,
        }
    }
}

impl_event!(ToggleSelectLoopEvent, ToggleSelectLoop, TOGGLE_SELECT_LOOP_PARAMS);

#[derive(Clone)]
pub struct SelectOnlyPlayingLoopsEvent {
    pub base: BaseEvent,
    pub setid: i32,
    pub playing: bool,
}

impl SelectOnlyPlayingLoopsEvent {
    pub fn new(setid: i32, playing: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SelectOnlyPlayingLoops,
                timestamp: 0.0,
            },
            setid,
            playing,
        }
    }
}

impl_event!(SelectOnlyPlayingLoopsEvent, SelectOnlyPlayingLoops, SELECT_ONLY_PLAYING_PARAMS);

#[derive(Clone)]
pub struct SelectAllLoopsEvent {
    pub base: BaseEvent,
    pub setid: i32,
    pub select: bool,
}

impl SelectAllLoopsEvent {
    pub fn new(setid: i32, select: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SelectAllLoops,
                timestamp: 0.0,
            },
            setid,
            select,
        }
    }
}

impl_event!(SelectAllLoopsEvent, SelectAllLoops, SELECT_ALL_LOOPS_PARAMS);

#[derive(Clone)]
pub struct InvertSelectionEvent {
    pub base: BaseEvent,
    pub setid: i32,
}

impl InvertSelectionEvent {
    pub fn new(setid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::InvertSelection,
                timestamp: 0.0,
            },
            setid,
        }
    }
}

impl_event!(InvertSelectionEvent, InvertSelection, SETID_PARAMS);

#[derive(Clone)]
pub struct TriggerSelectedLoopsEvent {
    pub base: BaseEvent,
    pub setid: i32,
    pub vol: f32,
    pub toggleloops: bool,
}

impl TriggerSelectedLoopsEvent {
    pub fn new(setid: i32, vol: f32, toggleloops: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::TriggerSelectedLoops,
                timestamp: 0.0,
            },
            setid,
            vol,
            toggleloops,
        }
    }
}

impl_event!(TriggerSelectedLoopsEvent, TriggerSelectedLoops, TRIGGER_SELECTED_LOOPS_PARAMS);

#[derive(Clone)]
pub struct SetSelectedLoopsTriggerVolumeEvent {
    pub base: BaseEvent,
    pub setid: i32,
    pub vol: f32,
}

impl SetSelectedLoopsTriggerVolumeEvent {
    pub fn new(setid: i32, vol: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetSelectedLoopsTriggerVolume,
                timestamp: 0.0,
            },
            setid,
            vol,
        }
    }
}

impl_event!(SetSelectedLoopsTriggerVolumeEvent, SetSelectedLoopsTriggerVolume, SET_SELECTED_LOOPS_TRIGGER_VOLUME_PARAMS);

#[derive(Clone)]
pub struct AdjustSelectedLoopsAmpEvent {
    pub base: BaseEvent,
    pub setid: i32,
    pub ampfactor: f32,
}

impl AdjustSelectedLoopsAmpEvent {
    pub fn new(setid: i32, ampfactor: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::AdjustSelectedLoopsAmp,
                timestamp: 0.0,
            },
            setid,
            ampfactor,
        }
    }
}

impl_event!(AdjustSelectedLoopsAmpEvent, AdjustSelectedLoopsAmp, ADJUST_SELECTED_LOOPS_AMP_PARAMS);

#[derive(Clone)]
pub struct MoveLoopEvent {
    pub base: BaseEvent,
    pub oldloopid: i32,
    pub newloopid: i32,
}

impl MoveLoopEvent {
    pub fn new(oldloopid: i32, newloopid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::MoveLoop,
                timestamp: 0.0,
            },
            oldloopid,
            newloopid,
        }
    }
}

impl_event!(MoveLoopEvent, MoveLoop);

#[derive(Clone)]
pub struct EraseLoopEvent {
    pub base: BaseEvent,
    pub index: i32,
}

impl EraseLoopEvent {
    pub fn new(index: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::EraseLoop,
                timestamp: 0.0,
            },
            index,
        }
    }
}

impl_event!(EraseLoopEvent, EraseLoop);

#[derive(Clone)]
pub struct EraseAllLoopsEvent {
    pub base: BaseEvent,
}

impl EraseAllLoopsEvent {
    pub fn new() -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::EraseAllLoops,
                timestamp: 0.0,
            },
        }
    }
}

impl_event!(EraseAllLoopsEvent, EraseAllLoops);

#[derive(Clone)]
pub struct SaveLoopEvent {
    pub base: BaseEvent,
    pub index: i32,
}

impl SaveLoopEvent {
    pub fn new(index: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SaveLoop,
                timestamp: 0.0,
            },
            index,
        }
    }
}

impl_event!(SaveLoopEvent, SaveLoop);

#[derive(Clone)]
pub struct RenameLoopEvent {
    pub base: BaseEvent,
    pub loopid: i32,
    pub in_layout: bool,
}

impl RenameLoopEvent {
    pub fn new(loopid: i32, in_layout: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::RenameLoop,
                timestamp: 0.0,
            },
            loopid,
            in_layout,
        }
    }
}

impl_event!(RenameLoopEvent, RenameLoop);

#[derive(Clone)]
pub struct EraseSelectedLoopsEvent {
    pub base: BaseEvent,
    pub setid: i32,
}

impl EraseSelectedLoopsEvent {
    pub fn new(setid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::EraseSelectedLoops,
                timestamp: 0.0,
            },
            setid,
        }
    }
}

impl_event!(EraseSelectedLoopsEvent, EraseSelectedLoops);

#[derive(Clone)]
pub struct ToggleDiskOutputEvent {
    pub base: BaseEvent,
}

impl ToggleDiskOutputEvent {
    pub fn new() -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::ToggleDiskOutput,
                timestamp: 0.0,
            },
        }
    }
}

impl_event!(ToggleDiskOutputEvent, ToggleDiskOutput);

#[derive(Clone)]
pub struct SetAutoLoopSavingEvent {
    pub base: BaseEvent,
    pub save: bool,
}

impl SetAutoLoopSavingEvent {
    pub fn new(save: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetAutoLoopSaving,
                timestamp: 0.0,
            },
            save,
        }
    }
}

impl_event!(SetAutoLoopSavingEvent, SetAutoLoopSaving);

#[derive(Clone)]
pub struct SaveNewSceneEvent {
    pub base: BaseEvent,
}

impl SaveNewSceneEvent {
    pub fn new() -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SaveNewScene,
                timestamp: 0.0,
            },
        }
    }
}

impl_event!(SaveNewSceneEvent, SaveNewScene);

#[derive(Clone)]
pub struct SaveCurrentSceneEvent {
    pub base: BaseEvent,
}

impl SaveCurrentSceneEvent {
    pub fn new() -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SaveCurrentScene,
                timestamp: 0.0,
            },
        }
    }
}

impl_event!(SaveCurrentSceneEvent, SaveCurrentScene);

#[derive(Clone)]
pub struct SetLoadLoopIdEvent {
    pub base: BaseEvent,
    pub index: i32,
}

impl SetLoadLoopIdEvent {
    pub fn new(index: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetLoadLoopId,
                timestamp: 0.0,
            },
            index,
        }
    }
}

impl_event!(SetLoadLoopIdEvent, SetLoadLoopId);

#[derive(Clone)]
pub struct SetDefaultLoopPlacementEvent {
    pub base: BaseEvent,
    pub looprange: Range,
}

impl SetDefaultLoopPlacementEvent {
    pub fn new(looprange: Range) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetDefaultLoopPlacement,
                timestamp: 0.0,
            },
            looprange,
        }
    }
}

impl_event!(SetDefaultLoopPlacementEvent, SetDefaultLoopPlacement);

#[derive(Clone)]
pub struct SelectPulseEvent {
    pub base: BaseEvent,
    pub pulse: i32,
}

impl SelectPulseEvent {
    pub fn new(pulse: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SelectPulse,
                timestamp: 0.0,
            },
            pulse,
        }
    }
}

impl_event!(SelectPulseEvent, SelectPulse);

#[derive(Clone)]
pub struct DeletePulseEvent {
    pub base: BaseEvent,
    pub pulse: i32,
}

impl DeletePulseEvent {
    pub fn new(pulse: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::DeletePulse,
                timestamp: 0.0,
            },
            pulse,
        }
    }
}

impl_event!(DeletePulseEvent, DeletePulse);

#[derive(Clone)]
pub struct TapPulseEvent {
    pub base: BaseEvent,
    pub pulse: i32,
    pub newlen: bool,
}

impl TapPulseEvent {
    pub fn new(pulse: i32, newlen: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::TapPulse,
                timestamp: 0.0,
            },
            pulse,
            newlen,
        }
    }
}

impl_event!(TapPulseEvent, TapPulse);

#[derive(Clone)]
pub struct SwitchMetronomeEvent {
    pub base: BaseEvent,
    pub pulse: i32,
    pub metronome: bool,
}

impl SwitchMetronomeEvent {
    pub fn new(pulse: i32, metronome: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SwitchMetronome,
                timestamp: 0.0,
            },
            pulse,
            metronome,
        }
    }
}

impl_event!(SwitchMetronomeEvent, SwitchMetronome);

#[derive(Clone)]
pub struct SetSyncTypeEvent {
    pub base: BaseEvent,
    pub stype: bool,
}

impl SetSyncTypeEvent {
    pub fn new(stype: bool) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetSyncType,
                timestamp: 0.0,
            },
            stype,
        }
    }
}

impl_event!(SetSyncTypeEvent, SetSyncType);

#[derive(Clone)]
pub struct SetSyncSpeedEvent {
    pub base: BaseEvent,
    pub sspd: i32,
}

impl SetSyncSpeedEvent {
    pub fn new(sspd: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetSyncSpeed,
                timestamp: 0.0,
            },
            sspd,
        }
    }
}

impl_event!(SetSyncSpeedEvent, SetSyncSpeed);

#[derive(Clone)]
pub struct SetMidiSyncEvent {
    pub base: BaseEvent,
    pub midisync: i32,
}

impl SetMidiSyncEvent {
    pub fn new(midisync: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetMidiSync,
                timestamp: 0.0,
            },
            midisync,
        }
    }
}

impl_event!(SetMidiSyncEvent, SetMidiSync);

#[derive(Clone)]
pub struct SetVariableEvent {
    pub base: BaseEvent,
    pub var_name: Option<String>,
    pub value: UserVariable,
    pub maxjumpcheck: bool,
    pub maxjump: UserVariable,
}

impl SetVariableEvent {
    pub fn new(
        var_name: Option<String>,
        value: UserVariable,
        maxjumpcheck: bool,
        maxjump: UserVariable,
    ) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SetVariable,
                timestamp: 0.0,
            },
            var_name,
            value,
            maxjumpcheck,
            maxjump,
        }
    }
}

impl_event!(SetVariableEvent, SetVariable, SET_VARIABLE_PARAMS);

#[derive(Clone)]
pub struct ToggleVariableEvent {
    pub base: BaseEvent,
    pub var_name: Option<String>,
    pub maxvalue: i32,
    pub minvalue: i32,
}

impl ToggleVariableEvent {
    pub fn new(var_name: Option<String>, maxvalue: i32, minvalue: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::ToggleVariable,
                timestamp: 0.0,
            },
            var_name,
            maxvalue,
            minvalue,
        }
    }
}

impl_event!(ToggleVariableEvent, ToggleVariable, TOGGLE_VARIABLE_PARAMS);

#[derive(Clone)]
pub struct SplitVariableMSBLSBEvent {
    pub base: BaseEvent,
    pub var: UserVariable,
    pub msb_name: Option<String>,
    pub lsb_name: Option<String>,
}

impl SplitVariableMSBLSBEvent {
    pub fn new(var: UserVariable, msb_name: Option<String>, lsb_name: Option<String>) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SplitVariableMSBLSB,
                timestamp: 0.0,
            },
            var,
            msb_name,
            lsb_name,
        }
    }
}

impl_event!(SplitVariableMSBLSBEvent, SplitVariableMSBLSB, SPLIT_VARIABLE_PARAMS);

#[derive(Clone)]
pub struct ParamSetGetAbsoluteParamIdxEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
    pub displayid: i32,
    pub paramidx: i32,
    pub absidx_name: Option<String>,
}

impl ParamSetGetAbsoluteParamIdxEvent {
    pub fn new(
        interfaceid: i32,
        displayid: i32,
        paramidx: i32,
        absidx_name: Option<String>,
    ) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::ParamSetGetAbsoluteParamIdx,
                timestamp: 0.0,
            },
            interfaceid,
            displayid,
            paramidx,
            absidx_name,
        }
    }
}

impl_event!(ParamSetGetAbsoluteParamIdxEvent, ParamSetGetAbsoluteParamIdx);

#[derive(Clone)]
pub struct ParamSetGetParamEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
    pub displayid: i32,
    pub paramidx: i32,
    pub var_name: Option<String>,
}

impl ParamSetGetParamEvent {
    pub fn new(interfaceid: i32, displayid: i32, paramidx: i32, var_name: Option<String>) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::ParamSetGetParam,
                timestamp: 0.0,
            },
            interfaceid,
            displayid,
            paramidx,
            var_name,
        }
    }
}

impl_event!(ParamSetGetParamEvent, ParamSetGetParam);

#[derive(Clone)]
pub struct ParamSetSetParamEvent {
    pub base: BaseEvent,
    pub interfaceid: i32,
    pub displayid: i32,
    pub paramidx: i32,
    pub value: f32,
}

impl ParamSetSetParamEvent {
    pub fn new(interfaceid: i32, displayid: i32, paramidx: i32, value: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::ParamSetSetParam,
                timestamp: 0.0,
            },
            interfaceid,
            displayid,
            paramidx,
            value,
        }
    }
}

impl_event!(ParamSetSetParamEvent, ParamSetSetParam);

#[derive(Clone)]
pub struct LogFaderVolToLinearEvent {
    pub base: BaseEvent,
    pub var_name: Option<String>,
    pub fadervol: UserVariable,
    pub scale: f32,
}

impl LogFaderVolToLinearEvent {
    pub fn new(var_name: Option<String>, fadervol: UserVariable, scale: f32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::LogFaderVolToLinear,
                timestamp: 0.0,
            },
            var_name,
            fadervol,
            scale,
        }
    }
}

impl_event!(LogFaderVolToLinearEvent, LogFaderVolToLinear);

#[derive(Clone)]
pub struct ALSAMixerControlSetEvent {
    pub base: BaseEvent,
    pub hwid: i32,
    pub numid: i32,
    pub val1: i32,
    pub val2: i32,
    pub val3: i32,
    pub val4: i32,
}

impl ALSAMixerControlSetEvent {
    pub fn new(hwid: i32, numid: i32, val1: i32, val2: i32, val3: i32, val4: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::ALSAMixerControlSet,
                timestamp: 0.0,
            },
            hwid,
            numid,
            val1,
            val2,
            val3,
            val4,
        }
    }
}

impl_event!(ALSAMixerControlSetEvent, ALSAMixerControlSet);

#[derive(Clone)]
pub struct CreateSnapshotEvent {
    pub base: BaseEvent,
    pub snapid: i32,
}

impl CreateSnapshotEvent {
    pub fn new(snapid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::CreateSnapshot,
                timestamp: 0.0,
            },
            snapid,
        }
    }
}

impl_event!(CreateSnapshotEvent, CreateSnapshot);

#[derive(Clone)]
pub struct SwapSnapshotsEvent {
    pub base: BaseEvent,
    pub snapid1: i32,
    pub snapid2: i32,
}

impl SwapSnapshotsEvent {
    pub fn new(snapid1: i32, snapid2: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::SwapSnapshots,
                timestamp: 0.0,
            },
            snapid1,
            snapid2,
        }
    }
}

impl_event!(SwapSnapshotsEvent, SwapSnapshots);

#[derive(Clone)]
pub struct RenameSnapshotEvent {
    pub base: BaseEvent,
    pub snapid: i32,
}

impl RenameSnapshotEvent {
    pub fn new(snapid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::RenameSnapshot,
                timestamp: 0.0,
            },
            snapid,
        }
    }
}

impl_event!(RenameSnapshotEvent, RenameSnapshot);

#[derive(Clone)]
pub struct TriggerSnapshotEvent {
    pub base: BaseEvent,
    pub snapid: i32,
}

impl TriggerSnapshotEvent {
    pub fn new(snapid: i32) -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::TriggerSnapshot,
                timestamp: 0.0,
            },
            snapid,
        }
    }
}

impl_event!(TriggerSnapshotEvent, TriggerSnapshot);

#[derive(Clone)]
pub struct TransmitPlayingLoopsToDAWEvent {
    pub base: BaseEvent,
}

impl TransmitPlayingLoopsToDAWEvent {
    pub fn new() -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::TransmitPlayingLoopsToDAW,
                timestamp: 0.0,
            },
        }
    }
}

impl_event!(TransmitPlayingLoopsToDAWEvent, TransmitPlayingLoopsToDAW);

#[derive(Clone)]
pub struct PulseSyncEvent {
    pub base: BaseEvent,
}

impl PulseSyncEvent {
    pub fn new() -> Self {
        Self {
            base: BaseEvent {
                event_type: EventType::PulseSync,
                timestamp: 0.0,
            },
        }
    }
}

impl_event!(PulseSyncEvent, PulseSync);

// ============================================================
// Event manager
// ============================================================

struct ListenerEntry {
    listener: Mutex<Box<dyn EventListener>>,
    _from: Option<String>,
}

pub struct EventManager {
    listeners: Arc<Mutex<HashMap<EventType, Vec<ListenerEntry>>>>,
    queue: Arc<Mutex<VecDeque<Box<dyn Event>>>>,
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
        let worker_queue = Arc::new(Mutex::new(VecDeque::<Box<dyn Event>>::new()));
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
                                    listener.receive_event(ev.clone_box(), &());
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
    pub fn try_post_event(&self, ev: Box<dyn Event>) -> Result<(), Box<dyn Event>> {
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

    pub fn post_event(&self, ev: Box<dyn Event>) {
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
                        listener.receive_event(ev.clone_box(), &stub);
                    }
                }
            }
        }
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

macro_rules! default_via_new {
    ($($type:ty),+ $(,)?) => {
        $(
            impl Default for $type {
                fn default() -> Self {
                    Self::new()
                }
            }
        )+
    };
}

default_via_new!(
    MIDIClockInputEvent,
    StartSessionEvent,
    ExitSessionEvent,
    SlideLoopAmpStopAllEvent,
    EraseAllLoopsEvent,
    ToggleDiskOutputEvent,
    SaveNewSceneEvent,
    SaveCurrentSceneEvent,
    TransmitPlayingLoopsToDAWEvent,
    PulseSyncEvent,
    EventManager,
);

#[cfg(test)]
mod event_contract_tests {
    use super::*;

    fn parameter_names(event: &dyn Event) -> Vec<&'static str> {
        (0..event.get_num_params())
            .map(|index| event.get_param(index).unwrap().name)
            .collect()
    }

    #[test]
    fn routed_midi_event_schemas_match_cpp_binding_parameters() {
        let controller = MIDIControllerInputEvent::new(2, 7, 99);
        assert_eq!(controller.outport, 1);
        assert!(!controller.echo);
        assert_eq!(
            parameter_names(&controller),
            [
                "outport",
                "midichannel",
                "controlnum",
                "controlval",
                "routethroughpatch",
            ]
        );
        let key = MIDIKeyInputEvent::new(2, 64, 100, true);
        assert_eq!(
            parameter_names(&key),
            [
                "outport",
                "keydown",
                "midichannel",
                "notenum",
                "velocity",
                "routethroughpatch",
            ]
        );
        let bend = MIDIPitchBendInputEvent::new(2, 123);
        assert_eq!(
            parameter_names(&bend),
            ["outport", "midichannel", "pitchval", "routethroughpatch"]
        );
    }

    #[test]
    fn midi_transport_events_expose_the_cpp_output_port() {
        let clock = MIDIClockInputEvent::new();
        assert_eq!(clock.outport, 1);
        assert_eq!(parameter_names(&clock), ["outport"]);
        let start = MIDIStartStopInputEvent::new(true);
        assert_eq!(start.outport, 1);
        assert_eq!(parameter_names(&start), ["outport", "start"]);
    }
}
