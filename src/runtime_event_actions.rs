//! Configuration-driven translation from native events to bounded runtime work.
//!
//! This module deliberately owns no runtime services.  The application feeds it
//! native input events, then drains the returned commands/actions into the
//! audio command ring and the appropriate application service.

use crate::config::{BindingRegistry, FloConfig, ResolvedBinding, StoredParameterValue};
use crate::datatypes::Range;
use crate::event::{Event, EventType};
use crate::midiio::MidiMessage;
use crate::native_dsp_graph::{LoopMode, MAX_RUNTIME_LOOPS, RuntimeCommand};

/// Non-audio work emitted by [`RuntimeEventDispatcher`].
#[derive(Clone, Debug, PartialEq)]
pub enum ApplicationAction {
    ExitSession,
    RenameLoop {
        loop_id: i32,
    },
    AlsamixerControlSet {
        hwid: i32,
        numid: i32,
        values: [i32; 4],
    },
    VideoShowDisplay {
        interface_id: i32,
        display_id: i32,
        show: bool,
    },
    VideoShowSnapshotPage {
        interface_id: i32,
        display_id: i32,
        page: i32,
    },
    VideoShowHelp(i32),
    ShowDebugInfo(bool),
    SaveLoop {
        loop_id: i32,
        codec: CodecSelection,
    },
    SetLoadLoopId(i32),
    SetDefaultLoopPlacement(Range),
    ImportSelectedLoop {
        browser: i32,
        codec: CodecSelection,
    },
    LoadSelectedScene {
        browser: i32,
    },
    SelectBrowserItem {
        browser: i32,
    },
    MoveBrowserItem {
        browser: i32,
        adjust: i32,
        jump_adjust: i32,
    },
    MoveBrowserItemAbsolute {
        browser: i32,
        index: i32,
    },
    BrowserItemBrowsed {
        browser: i32,
    },
    SaveScene {
        force_new: bool,
    },
    CreateSnapshot {
        snapshot: i32,
    },
    TriggerSnapshot {
        snapshot: i32,
    },
    RenameSnapshot {
        snapshot: i32,
    },
    SwapSnapshots {
        first: i32,
        second: i32,
    },
    RenameBrowserItem {
        browser: i32,
    },
    SetFullscreen(bool),
    VideoShowLoop {
        interface_id: i32,
        layout_id: i32,
        loop_ids: Range,
    },
    VideoShowLayout {
        interface_id: i32,
        layout_id: i32,
        show: bool,
        hide_others: bool,
    },
    VideoSwitchInterface(i32),
    ToggleStreaming {
        codec: CodecSelection,
    },
    SetAutoLoopSaving {
        enabled: bool,
        codec: CodecSelection,
    },
    MovePatchBank {
        direction: i32,
    },
    SelectPatchBank {
        index: i32,
    },
    SetSynthEnabled(bool),
    SetMidiSync(i32),
    SetSyncType(i32),
    SetSyncSpeed(i32),
    SelectPulse(i32),
    /// C++ `LoopManager::DeletePulse`: distinct from deselecting the pulse
    /// (`SelectPulse(-1)`) -- it also erases every loop attached to it.
    DeletePulse,
    /// C++ `LoopManager::TapPulse`: creates a pulse on the first tap,
    /// defines its length from the tap gap, and re-anchors the downbeat.
    TapPulse {
        new_len: bool,
    },
    MidiClock,
    MidiTransport {
        running: bool,
    },
    TransmitPlayingLoopsToDaw,
    EraseAllLoops,
    EraseSelectedLoops {
        set: i32,
    },
    ToggleLoopSelection {
        set: i32,
        loop_id: i32,
    },
    SelectPlayingLoops {
        set: i32,
        playing: bool,
    },
    SelectAllLoops {
        set: i32,
        selected: bool,
    },
    InvertLoopSelection {
        set: i32,
    },
    TriggerSelectedLoops {
        set: i32,
        gain: f32,
        toggle: bool,
    },
    SetSelectedTriggerVolume {
        set: i32,
        gain: f32,
    },
    AdjustSelectedLoopGain {
        set: i32,
        factor: f32,
    },
    SetLoopTriggerVolume {
        loop_id: i32,
        gain: f32,
    },
    SlideLoopGain {
        loop_id: i32,
        amount: f32,
    },
    SetLoopGain {
        loop_id: i32,
        gain: f32,
    },
    AdjustLoopGain {
        loop_id: i32,
        factor: f32,
    },
    MoveLoop {
        from: i32,
        to: i32,
    },
    StopSlidingLoopGain,
    SetMidiEchoPort(i32),
    SetMidiEchoChannel(i32),
    AdjustMidiTranspose(i32),
    OutputMidi {
        message: MidiMessage,
        outport: i32,
        route_through_patch: bool,
    },
}

/// Codec choice remains configuration-owned. The runtime resolves these
/// selectors from `loopoutformat`/`streamoutformat` when doing filesystem work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecSelection {
    ConfiguredLoopOutput,
    ConfiguredStreamOutput,
    DetectFromSelectedFile,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DispatchOutput {
    Runtime(RuntimeCommand),
    Application(ApplicationAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchError {
    OutputFull { capacity: usize },
    InvalidLoopId(i32),
    InvalidInputId(i32),
    RecursionLimit { depth: usize },
    MissingParameter(&'static str),
    InvalidParameter(&'static str),
}

/// A stack-backed output batch. Dispatch never grows a queue or silently drops
/// work; callers get `OutputFull` and can expose backpressure to the UI.
#[derive(Debug)]
pub struct ActionBatch<const N: usize> {
    echo: bool,
    len: usize,
    entries: [Option<DispatchOutput>; N],
}

impl<const N: usize> ActionBatch<N> {
    fn new(echo: bool) -> Self {
        Self {
            echo,
            len: 0,
            entries: std::array::from_fn(|_| None),
        }
    }

    pub fn echo_input(&self) -> bool {
        self.echo
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn iter(&self) -> impl Iterator<Item = &DispatchOutput> {
        self.entries[..self.len].iter().filter_map(Option::as_ref)
    }

    fn push(&mut self, output: DispatchOutput) -> Result<(), DispatchError> {
        if self.len == N {
            return Err(DispatchError::OutputFull { capacity: N });
        }
        self.entries[self.len] = Some(output);
        self.len += 1;
        Ok(())
    }

    fn append(&mut self, other: Self) -> Result<(), DispatchError> {
        let len = other.len;
        for output in other.entries.into_iter().take(len).flatten() {
            self.push(output)?;
        }
        Ok(())
    }
}

/// Read-only audio state needed to preserve trigger-loop's C++ state machine.
pub trait RuntimeLoopState {
    fn loop_mode(&self, slot: u8) -> LoopMode;
}

impl RuntimeLoopState for [LoopMode; MAX_RUNTIME_LOOPS] {
    fn loop_mode(&self, slot: u8) -> LoopMode {
        self[slot as usize]
    }
}

pub struct RuntimeEventDispatcher<const N: usize = 32>;

const MAX_GOSUB_DEPTH: usize = 32;

impl<const N: usize> Default for RuntimeEventDispatcher<N> {
    fn default() -> Self {
        Self
    }
}

impl<const N: usize> RuntimeEventDispatcher<N> {
    pub fn new() -> Self {
        Self
    }

    /// Resolve conditions/paramsets/variables through `FloConfig`, then map the
    /// resulting events. The registry is explicit so a runtime may atomically
    /// replace configuration without rebuilding this dispatcher.
    pub fn dispatch(
        &self,
        config: &mut FloConfig,
        registry: &BindingRegistry,
        input: &dyn Event,
        loops: &impl RuntimeLoopState,
    ) -> Result<ActionBatch<N>, DispatchError> {
        self.dispatch_at_depth(config, registry, input, loops, 0)
    }

    fn dispatch_at_depth(
        &self,
        config: &mut FloConfig,
        registry: &BindingRegistry,
        input: &dyn Event,
        loops: &impl RuntimeLoopState,
        depth: usize,
    ) -> Result<ActionBatch<N>, DispatchError> {
        if depth > MAX_GOSUB_DEPTH {
            return Err(DispatchError::RecursionLimit { depth });
        }
        let resolved = config.dispatch_registered_event_bindings(input, registry);
        let mut batch = ActionBatch::new(resolved.echo);
        let had_binding = !resolved.matched.is_empty();
        for mut binding in resolved.matched {
            // Outputs in a continued binding chain are delivered in order in
            // C++. Re-evaluation lets output1 variable changes feed output2.
            binding.parameters = config.set_dynamic_parameters(input, &binding.binding);
            if self.apply_variable_output(config, &binding)? {
                continue;
            }
            self.map_binding(config, registry, &binding, loops, &mut batch, depth)?;
        }
        if !had_binding {
            self.map_unbound_native_input(input, &mut batch)?;
        }
        Ok(batch)
    }

    fn map_unbound_native_input(
        &self,
        input: &dyn Event,
        out: &mut ActionBatch<N>,
    ) -> Result<(), DispatchError> {
        use crate::event::{
            AdjustMidiTransposeEvent, MIDIControllerInputEvent, MIDIKeyInputEvent,
            MIDIPitchBendInputEvent, MIDIProgramChangeInputEvent, MIDIStartStopInputEvent,
        };
        let runtime = |command| DispatchOutput::Runtime(command);
        let app = |action| DispatchOutput::Application(action);
        match input.get_type() {
            EventType::EndRecord => out.push(runtime(RuntimeCommand::StopRecord))?,
            EventType::InputMIDIClock => out.push(app(ApplicationAction::MidiClock))?,
            EventType::SetSyncType => {
                let event = input
                    .as_any()
                    .downcast_ref::<crate::event::SetSyncTypeEvent>()
                    .ok_or(DispatchError::InvalidParameter("stype"))?;
                out.push(app(ApplicationAction::SetSyncType(event.stype as i32)))?
            }
            EventType::SetSyncSpeed => {
                let event = input
                    .as_any()
                    .downcast_ref::<crate::event::SetSyncSpeedEvent>()
                    .ok_or(DispatchError::InvalidParameter("sspd"))?;
                out.push(app(ApplicationAction::SetSyncSpeed(event.sspd)))?
            }
            EventType::AdjustMidiTranspose => {
                let event = input
                    .as_any()
                    .downcast_ref::<AdjustMidiTransposeEvent>()
                    .ok_or(DispatchError::InvalidParameter("adjust"))?;
                out.push(app(ApplicationAction::AdjustMidiTranspose(event.adjust)))?
            }
            EventType::InputMIDIStartStop => {
                let event = input
                    .as_any()
                    .downcast_ref::<MIDIStartStopInputEvent>()
                    .ok_or(DispatchError::InvalidParameter("start"))?;
                out.push(app(ApplicationAction::MidiTransport {
                    running: event.start,
                }))?;
            }
            EventType::InputMIDIKey => {
                let event = input
                    .as_any()
                    .downcast_ref::<MIDIKeyInputEvent>()
                    .ok_or(DispatchError::InvalidParameter("midikey"))?;
                out.push(runtime(RuntimeCommand::SynthNote {
                    note: event.notenum,
                    velocity: if event.down { event.vel } else { 0 },
                }))?;
            }
            EventType::InputMIDIController => {
                let event = input
                    .as_any()
                    .downcast_ref::<MIDIControllerInputEvent>()
                    .ok_or(DispatchError::InvalidParameter("midicontroller"))?;
                out.push(runtime(RuntimeCommand::SynthController {
                    channel: event.channel,
                    control: event.ctrl,
                    value: event.val,
                }))?;
            }
            EventType::InputMIDIPitchBend => {
                let event = input
                    .as_any()
                    .downcast_ref::<MIDIPitchBendInputEvent>()
                    .ok_or(DispatchError::InvalidParameter("midipitchbend"))?;
                out.push(runtime(RuntimeCommand::SynthPitchBend {
                    channel: event.channel,
                    value: u16::try_from(event.val)
                        .map_err(|_| DispatchError::InvalidParameter("pitchval"))?,
                }))?;
            }
            EventType::InputMIDIProgramChange => {
                let event = input
                    .as_any()
                    .downcast_ref::<MIDIProgramChangeInputEvent>()
                    .ok_or(DispatchError::InvalidParameter("midiprogramchange"))?;
                out.push(runtime(RuntimeCommand::SynthPatch {
                    channel: event.channel,
                    soundfont_id: 0,
                    bank: 0,
                    program: event.val as i32,
                }))?;
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_variable_output(
        &self,
        config: &mut FloConfig,
        binding: &ResolvedBinding,
    ) -> Result<bool, DispatchError> {
        let p = &binding.parameters;
        match binding.binding.output_event {
            Some(EventType::SetVariable) => {
                let name = variable_ref(p, "var")?;
                let next = variable(p, "value")?.clone();
                config.set_variable(name, next);
                Ok(true)
            }
            Some(EventType::ToggleVariable) => {
                let name = variable_ref(p, "var")?;
                let min = int_or(p, "minvalue", 0)?;
                let max = int(p, "maxvalue")?;
                let current = config.get_int(name).unwrap_or(min);
                config.set_int_variable(name, if current >= max { min } else { current + 1 });
                Ok(true)
            }
            Some(EventType::SplitVariableMSBLSB) => {
                let raw = variable(p, "var")?.as_i32();
                config.set_int_variable(variable_ref(p, "msb")?, (raw >> 7) & 0x7f);
                config.set_int_variable(variable_ref(p, "lsb")?, raw & 0x7f);
                Ok(true)
            }
            Some(EventType::LogFaderVolToLinear) => {
                let name = variable_ref(p, "var")?;
                let fader = variable(p, "fadervol")?.as_f32();
                let scale = float(p, "scale")?;
                let db = crate::core_dsp::AudioLevel::fader_to_db(fader, config.fader_max_db());
                config.set_float_variable(name, 10.0_f32.powf(db / 20.0) * scale);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn map_binding(
        &self,
        config: &mut FloConfig,
        registry: &BindingRegistry,
        binding: &ResolvedBinding,
        loops: &impl RuntimeLoopState,
        out: &mut ActionBatch<N>,
        depth: usize,
    ) -> Result<(), DispatchError> {
        let Some(typ) = binding.binding.output_event else {
            return Ok(());
        };
        let p = &binding.parameters;
        let runtime = |command| DispatchOutput::Runtime(command);
        let app = |action| DispatchOutput::Application(action);
        match typ {
            // These event names are part of the XML action vocabulary, but
            // their native runtime representation is already a command.
            EventType::StartSession | EventType::StartInterface => {}
            EventType::ExitSession => out.push(app(ApplicationAction::ExitSession))?,
            EventType::GoSub => {
                let event = crate::event::GoSubEvent::new(
                    int(p, "sub")?,
                    float(p, "param1")?,
                    float(p, "param2")?,
                    float(p, "param3")?,
                );
                let nested = self.dispatch_at_depth(config, registry, &event, loops, depth + 1)?;
                out.append(nested)?;
            }
            EventType::ALSAMixerControlSet => {
                out.push(app(ApplicationAction::AlsamixerControlSet {
                    hwid: int(p, "hwid")?,
                    numid: int(p, "numid")?,
                    values: [
                        int(p, "val1")?,
                        int(p, "val2")?,
                        int(p, "val3")?,
                        int(p, "val4")?,
                    ],
                }))?
            }
            EventType::ParamSetGetAbsoluteParamIdx => {
                let key = (int(p, "interfaceid")?, int(p, "displayid")?);
                let relative = int(p, "paramidx")? as isize;
                let target = variable_ref(p, "absidx")?;
                if let Some(paramset) = config.paramsets.get(&key)
                    && let Some(index) = paramset.absolute_param_index(relative)
                {
                    config.set_int_variable(target, index as i32);
                }
            }
            EventType::ParamSetGetParam => {
                let key = (int(p, "interfaceid")?, int(p, "displayid")?);
                let relative = int(p, "paramidx")? as isize;
                let target = variable_ref(p, "var")?;
                let value = config
                    .paramsets
                    .get(&key)
                    .map_or(0.0, |paramset| paramset.get_param(relative));
                config.set_float_variable(target, value);
            }
            EventType::ParamSetSetParam => {
                let key = (int(p, "interfaceid")?, int(p, "displayid")?);
                let relative = int(p, "paramidx")? as isize;
                let value = float(p, "value")?;
                if let Some(paramset) = config.paramsets.get_mut(&key) {
                    paramset.set_param(relative, value);
                    paramset.link_active_params();
                }
            }
            EventType::VideoShowParamSetBank => {
                let key = (int(p, "interfaceid")?, int(p, "displayid")?);
                if let Some(paramset) = config.paramsets.get_mut(&key) {
                    paramset.show_bank(int(p, "bank")? as isize);
                    paramset.link_active_params();
                }
            }
            EventType::VideoShowParamSetPage => {
                let key = (int(p, "interfaceid")?, int(p, "displayid")?);
                if let Some(paramset) = config.paramsets.get_mut(&key) {
                    paramset.show_page(int(p, "page")? as isize);
                    paramset.link_active_params();
                }
            }
            EventType::EndRecord => out.push(runtime(RuntimeCommand::StopRecord))?,
            EventType::SetMasterInVolume => {
                let vol = float_or(p, "vol", -1.0)?;
                let gain = if vol >= 0.0 {
                    vol
                } else if let Some(fader) = p.iter().find(|(key, _)| key == "fadervol") {
                    let fader = match &fader.1 {
                        StoredParameterValue::Float(value) => *value,
                        StoredParameterValue::Int(value) => *value as f32,
                        _ => return Err(DispatchError::InvalidParameter("fadervol")),
                    };
                    let db = crate::core_dsp::AudioLevel::fader_to_db(fader, config.fader_max_db());
                    10.0_f32.powf(db / 20.0)
                } else {
                    -1.0
                };
                if gain >= 0.0 {
                    out.push(runtime(RuntimeCommand::SetInputMonitor(gain)))?;
                }
            }
            EventType::SetMasterOutVolume => {
                let vol = float_or(p, "vol", -1.0)?;
                let gain = if vol >= 0.0 {
                    vol
                } else if let Some(fader) = p.iter().find(|(key, _)| key == "fadervol") {
                    let fader = match &fader.1 {
                        StoredParameterValue::Float(value) => *value,
                        StoredParameterValue::Int(value) => *value as f32,
                        _ => return Err(DispatchError::InvalidParameter("fadervol")),
                    };
                    let db = crate::core_dsp::AudioLevel::fader_to_db(fader, config.fader_max_db());
                    10.0_f32.powf(db / 20.0)
                } else {
                    -1.0
                };
                if gain >= 0.0 {
                    out.push(runtime(RuntimeCommand::SetMasterGain(gain)))?;
                }
            }
            EventType::SlideMasterInVolume => out.push(runtime(
                RuntimeCommand::AdjustInputMonitor(float(p, "slide")?),
            ))?,
            EventType::SlideMasterOutVolume => out.push(runtime(
                RuntimeCommand::AdjustMasterGain(float(p, "slide")?),
            ))?,
            EventType::SwitchMetronome => out.push(runtime(RuntimeCommand::SetMetronome {
                enabled: bool_param(p, "metronome")?,
                gain: 1.0,
            }))?,
            EventType::TriggerLoop => {
                let slot = loop_slot(int(p, "loopid")?)?;
                let gain = float_or(p, "vol", 1.0)?;
                // Legacy XML contains two TriggerLoop-only fields which older
                // EventType schemas did not retain.  The selected binding is
                // nevertheless unambiguous: VAR_overdubmode is its condition.
                let overdub = bool_or(p, "overdub", config.get_int("VAR_overdubmode") == Some(1))?;
                if overdub {
                    out.push(runtime(RuntimeCommand::Overdub {
                        slot,
                        feedback: float_or(
                            p,
                            "overdubfeedback",
                            config.get_float("VAR_overdubfeedback").unwrap_or(1.0),
                        )?,
                        gain,
                    }))?;
                } else {
                    let command = match loops.loop_mode(slot) {
                        LoopMode::Empty => RuntimeCommand::Record { slot },
                        LoopMode::Recording | LoopMode::Overdubbing => RuntimeCommand::StopRecord,
                        LoopMode::Playing => RuntimeCommand::Mute { slot, muted: true },
                        LoopMode::Muted => RuntimeCommand::Trigger { slot, gain },
                    };
                    out.push(runtime(command))?;
                }
            }
            EventType::EraseLoop => out.push(runtime(RuntimeCommand::Erase {
                slot: loop_slot(int(p, "loopid")?)?,
            }))?,
            EventType::EraseAllLoops => out.push(app(ApplicationAction::EraseAllLoops))?,
            EventType::EraseSelectedLoops => {
                out.push(app(ApplicationAction::EraseSelectedLoops {
                    set: int(p, "setid")?,
                }))?
            }
            EventType::ToggleSelectLoop => {
                out.push(app(ApplicationAction::ToggleLoopSelection {
                    set: int(p, "setid")?,
                    loop_id: int(p, "loopid")?,
                }))?
            }
            EventType::SelectOnlyPlayingLoops => {
                out.push(app(ApplicationAction::SelectPlayingLoops {
                    set: int(p, "setid")?,
                    playing: bool_or(p, "playing", false)?,
                }))?
            }
            EventType::SelectAllLoops => out.push(app(ApplicationAction::SelectAllLoops {
                set: int(p, "setid")?,
                selected: bool_or(p, "select", false)?,
            }))?,
            EventType::InvertSelection => {
                out.push(app(ApplicationAction::InvertLoopSelection {
                    set: int(p, "setid")?,
                }))?
            }
            EventType::TriggerSelectedLoops => {
                out.push(app(ApplicationAction::TriggerSelectedLoops {
                    set: int(p, "setid")?,
                    gain: float_or(p, "vol", 1.0)?,
                    toggle: bool_or(p, "toggleloops", false)?,
                }))?
            }
            EventType::SetSelectedLoopsTriggerVolume => {
                out.push(app(ApplicationAction::SetSelectedTriggerVolume {
                    set: int(p, "setid")?,
                    gain: float_or(p, "vol", 1.0)?,
                }))?
            }
            EventType::AdjustSelectedLoopsAmp => {
                out.push(app(ApplicationAction::AdjustSelectedLoopGain {
                    set: int(p, "setid")?,
                    factor: float_or(p, "ampfactor", 1.0)?,
                }))?
            }
            EventType::SetTriggerVolume => {
                out.push(app(ApplicationAction::SetLoopTriggerVolume {
                    loop_id: int(p, "loopid")?,
                    gain: float(p, "vol")?,
                }))?
            }
            EventType::SlideLoopAmp => out.push(app(ApplicationAction::SlideLoopGain {
                loop_id: int(p, "loopid")?,
                amount: float(p, "slide")?,
            }))?,
            EventType::SetLoopAmp => out.push(app(ApplicationAction::SetLoopGain {
                loop_id: int(p, "loopid")?,
                gain: float(p, "amp")?,
            }))?,
            EventType::AdjustLoopAmp => out.push(app(ApplicationAction::AdjustLoopGain {
                loop_id: int(p, "loopid")?,
                factor: float(p, "ampfactor")?,
            }))?,
            EventType::MoveLoop => out.push(app(ApplicationAction::MoveLoop {
                from: int(p, "oldloopid")?,
                to: int(p, "newloopid")?,
            }))?,
            EventType::SlideLoopAmpStopAll => {
                out.push(app(ApplicationAction::StopSlidingLoopGain))?
            }
            EventType::CreateSnapshot => {
                out.push(runtime(RuntimeCommand::RequestSnapshot))?;
                out.push(app(ApplicationAction::CreateSnapshot {
                    snapshot: int(p, "snapid")?,
                }))?;
            }
            EventType::TriggerSnapshot => out.push(app(ApplicationAction::TriggerSnapshot {
                snapshot: int(p, "snapid")?,
            }))?,
            EventType::RenameSnapshot => out.push(app(ApplicationAction::RenameSnapshot {
                snapshot: int(p, "snapid")?,
            }))?,
            EventType::SwapSnapshots => out.push(app(ApplicationAction::SwapSnapshots {
                first: int(p, "snapid1")?,
                second: int(p, "snapid2")?,
            }))?,
            EventType::BrowserRenameItem => {
                out.push(app(ApplicationAction::RenameBrowserItem {
                    browser: int(p, "browserid")?,
                }))?
            }
            EventType::BrowserMoveToItem => out.push(app(ApplicationAction::MoveBrowserItem {
                browser: int(p, "browserid")?,
                adjust: int(p, "adjust")?,
                jump_adjust: int(p, "jumpadjust")?,
            }))?,
            EventType::BrowserMoveToItemAbsolute => {
                out.push(app(ApplicationAction::MoveBrowserItemAbsolute {
                    browser: int(p, "browserid")?,
                    index: int(p, "idx")?,
                }))?
            }
            EventType::BrowserItemBrowsed => {
                out.push(app(ApplicationAction::BrowserItemBrowsed {
                    browser: int(p, "browserid")?,
                }))?
            }
            EventType::BrowserSelectItem => {
                let browser = int(p, "browserid")?;
                if config.get_int("DISPLAY_browser_loop") == Some(browser) {
                    out.push(app(ApplicationAction::ImportSelectedLoop {
                        browser,
                        codec: CodecSelection::DetectFromSelectedFile,
                    }))?;
                } else if config.get_int("DISPLAY_browser_scene") == Some(browser) {
                    out.push(app(ApplicationAction::LoadSelectedScene { browser }))?;
                } else {
                    out.push(app(ApplicationAction::SelectBrowserItem { browser }))?;
                }
            }
            EventType::SaveLoop => out.push(app(ApplicationAction::SaveLoop {
                loop_id: int(p, "loopid")?,
                codec: CodecSelection::ConfiguredLoopOutput,
            }))?,
            EventType::SetLoadLoopId => {
                out.push(app(ApplicationAction::SetLoadLoopId(int(p, "loopid")?)))?
            }
            EventType::SetDefaultLoopPlacement => out.push(app(
                ApplicationAction::SetDefaultLoopPlacement(range(p, "looprange")?),
            ))?,
            EventType::SaveNewScene => {
                out.push(app(ApplicationAction::SaveScene { force_new: true }))?
            }
            EventType::SaveCurrentScene => {
                out.push(app(ApplicationAction::SaveScene { force_new: false }))?
            }
            EventType::VideoFullScreen => out.push(app(ApplicationAction::SetFullscreen(
                bool_param(p, "fullscreen")?,
            )))?,
            EventType::VideoShowLoop => out.push(app(ApplicationAction::VideoShowLoop {
                interface_id: int(p, "interfaceid")?,
                layout_id: int(p, "layoutid")?,
                loop_ids: range(p, "loopid")?,
            }))?,
            EventType::VideoShowLayout => out.push(app(ApplicationAction::VideoShowLayout {
                interface_id: int(p, "interfaceid")?,
                layout_id: int(p, "layoutid")?,
                show: bool_param(p, "show")?,
                hide_others: bool_param(p, "hideothers")?,
            }))?,
            EventType::VideoShowDisplay => out.push(app(ApplicationAction::VideoShowDisplay {
                interface_id: int(p, "interfaceid")?,
                display_id: int(p, "displayid")?,
                show: bool_param(p, "show")?,
            }))?,
            EventType::VideoShowSnapshotPage => {
                out.push(app(ApplicationAction::VideoShowSnapshotPage {
                    interface_id: int(p, "interfaceid")?,
                    display_id: int(p, "displayid")?,
                    page: int(p, "page")?,
                }))?
            }
            EventType::VideoShowHelp => {
                out.push(app(ApplicationAction::VideoShowHelp(int(p, "page")?)))?
            }
            EventType::ShowDebugInfo => out.push(app(ApplicationAction::ShowDebugInfo(
                bool_param(p, "show")?,
            )))?,
            EventType::VideoSwitchInterface => out.push(app(
                ApplicationAction::VideoSwitchInterface(int(p, "interfaceid")?),
            ))?,
            EventType::ToggleDiskOutput => out.push(app(ApplicationAction::ToggleStreaming {
                codec: CodecSelection::ConfiguredStreamOutput,
            }))?,
            EventType::SetAutoLoopSaving => {
                out.push(app(ApplicationAction::SetAutoLoopSaving {
                    enabled: bool_param(p, "save")?,
                    codec: CodecSelection::ConfiguredLoopOutput,
                }))?
            }
            EventType::PatchBrowserMoveToBank => {
                out.push(app(ApplicationAction::MovePatchBank {
                    direction: int(p, "direction")?,
                }))?
            }
            EventType::PatchBrowserMoveToBankByIndex => {
                out.push(app(ApplicationAction::SelectPatchBank {
                    index: int(p, "idx")?,
                }))?
            }
            EventType::FluidSynthEnable => {
                let enabled = bool_param(p, "enable")?;
                if !enabled {
                    out.push(runtime(RuntimeCommand::SynthOff))?;
                }
                out.push(app(ApplicationAction::SetSynthEnabled(enabled)))?;
            }
            EventType::SlideInVolume => out.push(runtime(RuntimeCommand::AdjustInputVolume {
                input: input_slot(int(p, "input")?)?,
                amount: float(p, "slide")?,
            }))?,
            EventType::SetInVolume => out.push(runtime(RuntimeCommand::SetInputVolume {
                input: input_slot(int(p, "input")?)?,
                volume: float_or(p, "vol", -1.0)?,
                fader_volume: float_or(p, "fadervol", -1.0)?,
            }))?,
            EventType::ToggleInputRecord => {
                out.push(runtime(RuntimeCommand::ToggleInputRecord {
                    input: input_slot(int(p, "input")?)?,
                }))?
            }
            EventType::RenameLoop => {
                if bool_param(p, "in")? {
                    out.push(app(ApplicationAction::RenameLoop {
                        loop_id: int(p, "loopid")?,
                    }))?;
                }
            }
            EventType::SetMidiTuning => out.push(runtime(RuntimeCommand::SynthTuning {
                cents: float(p, "tuning")?,
            }))?,
            EventType::SetSyncType => {
                out.push(app(ApplicationAction::SetSyncType(int(p, "stype")?)))?
            }
            EventType::SetSyncSpeed => {
                out.push(app(ApplicationAction::SetSyncSpeed(int(p, "sspd")?)))?
            }
            EventType::SetMidiEchoPort => {
                out.push(app(ApplicationAction::SetMidiEchoPort(int(p, "echoport")?)))?
            }
            EventType::SetMidiEchoChannel => out.push(app(
                ApplicationAction::SetMidiEchoChannel(int(p, "echochannel")?),
            ))?,
            EventType::AdjustMidiTranspose => out.push(app(
                ApplicationAction::AdjustMidiTranspose(int(p, "adjust")?),
            ))?,
            EventType::InputMIDIKey => {
                let channel = byte(p, "midichannel")?;
                let note = byte(p, "notenum")?;
                let velocity = byte(p, "velocity")?;
                let down = bool_param(p, "keydown")?;
                out.push(runtime(RuntimeCommand::SynthNote {
                    note,
                    velocity: if down { velocity } else { 0 },
                }))?;
                out.push(app(ApplicationAction::OutputMidi {
                    message: if down {
                        MidiMessage::NoteOn {
                            channel,
                            note,
                            velocity,
                        }
                    } else {
                        MidiMessage::NoteOff {
                            channel,
                            note,
                            velocity,
                        }
                    },
                    outport: int_or(p, "outport", 1)?,
                    route_through_patch: bool_or(p, "routethroughpatch", false)?,
                }))?;
            }
            EventType::InputMIDIController => {
                let channel = byte(p, "midichannel")?;
                let control = byte(p, "controlnum")?;
                let value = byte(p, "controlval")?;
                out.push(runtime(RuntimeCommand::SynthController {
                    channel,
                    control,
                    value,
                }))?;
                out.push(app(ApplicationAction::OutputMidi {
                    message: MidiMessage::Controller {
                        channel,
                        control,
                        value,
                    },
                    outport: int_or(p, "outport", 1)?,
                    route_through_patch: bool_or(p, "routethroughpatch", false)?,
                }))?;
            }
            EventType::InputMIDIPitchBend => {
                let channel = byte(p, "midichannel")?;
                let value = u16_param(p, "pitchval")?;
                out.push(runtime(RuntimeCommand::SynthPitchBend { channel, value }))?;
                out.push(app(ApplicationAction::OutputMidi {
                    message: MidiMessage::PitchBend { channel, value },
                    outport: int_or(p, "outport", 1)?,
                    route_through_patch: bool_or(p, "routethroughpatch", false)?,
                }))?;
            }
            EventType::InputMIDIProgramChange => {
                let channel = byte(p, "midichannel")?;
                let program = int(p, "programval")?;
                out.push(runtime(RuntimeCommand::SynthPatch {
                    channel,
                    soundfont_id: int_or(p, "soundfontid", 0)?,
                    bank: int_or(p, "bank", 0)?,
                    program,
                }))?;
                out.push(app(ApplicationAction::OutputMidi {
                    message: MidiMessage::ProgramChange {
                        channel,
                        program: program.clamp(0, 127) as u8,
                    },
                    outport: int_or(p, "outport", 1)?,
                    route_through_patch: bool_or(p, "routethroughpatch", false)?,
                }))?;
            }
            EventType::InputMIDIChannelPressure => {
                let channel = byte(p, "midichannel")?;
                let value = byte(p, "pressureval")?;
                out.push(app(ApplicationAction::OutputMidi {
                    message: MidiMessage::ChannelPressure { channel, value },
                    outport: int_or(p, "outport", 1)?,
                    route_through_patch: bool_or(p, "routethroughpatch", false)?,
                }))?;
            }
            EventType::InputMIDIClock => out.push(app(ApplicationAction::MidiClock))?,
            EventType::InputMIDIStartStop => out.push(app(ApplicationAction::MidiTransport {
                running: bool_param(p, "start")?,
            }))?,
            EventType::SetMidiSync => {
                out.push(app(ApplicationAction::SetMidiSync(int(p, "midisync")?)))?
            }
            EventType::SelectPulse => {
                out.push(app(ApplicationAction::SelectPulse(int(p, "pulse")?)))?
            }
            EventType::DeletePulse => out.push(app(ApplicationAction::DeletePulse))?,
            EventType::TapPulse => out.push(app(ApplicationAction::TapPulse {
                new_len: bool_param(p, "newlen")?,
            }))?,
            EventType::TransmitPlayingLoopsToDAW => {
                out.push(app(ApplicationAction::TransmitPlayingLoopsToDaw))?
            }
            _ => {}
        }
        Ok(())
    }
}

fn value<'a>(
    p: &'a [(String, StoredParameterValue)],
    name: &'static str,
) -> Result<&'a StoredParameterValue, DispatchError> {
    p.iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
        .ok_or(DispatchError::MissingParameter(name))
}
fn int(p: &[(String, StoredParameterValue)], name: &'static str) -> Result<i32, DispatchError> {
    match value(p, name)? {
        StoredParameterValue::Char(v) => Ok(*v as i32),
        StoredParameterValue::Int(v) => Ok(*v),
        StoredParameterValue::Long(v) => Ok(*v as i32),
        StoredParameterValue::Float(v) => Ok(*v as i32),
        _ => Err(DispatchError::InvalidParameter(name)),
    }
}
fn float(p: &[(String, StoredParameterValue)], name: &'static str) -> Result<f32, DispatchError> {
    match value(p, name)? {
        StoredParameterValue::Char(v) => Ok(*v as f32),
        StoredParameterValue::Int(v) => Ok(*v as f32),
        StoredParameterValue::Long(v) => Ok(*v as f32),
        StoredParameterValue::Float(v) => Ok(*v),
        _ => Err(DispatchError::InvalidParameter(name)),
    }
}
fn int_or(
    p: &[(String, StoredParameterValue)],
    name: &'static str,
    default: i32,
) -> Result<i32, DispatchError> {
    if p.iter().any(|(key, _)| key == name) {
        int(p, name)
    } else {
        Ok(default)
    }
}
fn float_or(
    p: &[(String, StoredParameterValue)],
    name: &'static str,
    default: f32,
) -> Result<f32, DispatchError> {
    if p.iter().any(|(key, _)| key == name) {
        float(p, name)
    } else {
        Ok(default)
    }
}
fn bool_param(
    p: &[(String, StoredParameterValue)],
    name: &'static str,
) -> Result<bool, DispatchError> {
    Ok(int(p, name)? != 0)
}
fn bool_or(
    p: &[(String, StoredParameterValue)],
    name: &'static str,
    default: bool,
) -> Result<bool, DispatchError> {
    if p.iter().any(|(key, _)| key == name) {
        bool_param(p, name)
    } else {
        Ok(default)
    }
}
fn byte(p: &[(String, StoredParameterValue)], name: &'static str) -> Result<u8, DispatchError> {
    u8::try_from(int(p, name)?).map_err(|_| DispatchError::InvalidParameter(name))
}
fn u16_param(
    p: &[(String, StoredParameterValue)],
    name: &'static str,
) -> Result<u16, DispatchError> {
    u16::try_from(int(p, name)?).map_err(|_| DispatchError::InvalidParameter(name))
}
fn variable<'a>(
    p: &'a [(String, StoredParameterValue)],
    name: &'static str,
) -> Result<&'a crate::datatypes::UserVariable, DispatchError> {
    match value(p, name)? {
        StoredParameterValue::Variable(v) => Ok(v),
        _ => Err(DispatchError::InvalidParameter(name)),
    }
}
fn variable_ref<'a>(
    p: &'a [(String, StoredParameterValue)],
    name: &'static str,
) -> Result<&'a str, DispatchError> {
    match value(p, name)? {
        StoredParameterValue::VariableRef(Some(v)) => Ok(v),
        _ => Err(DispatchError::InvalidParameter(name)),
    }
}
fn range(p: &[(String, StoredParameterValue)], name: &'static str) -> Result<Range, DispatchError> {
    match value(p, name)? {
        StoredParameterValue::Range(value) => Ok(*value),
        _ => Err(DispatchError::InvalidParameter(name)),
    }
}
fn loop_slot(id: i32) -> Result<u8, DispatchError> {
    if (0..MAX_RUNTIME_LOOPS as i32).contains(&id) {
        Ok(id as u8)
    } else {
        Err(DispatchError::InvalidLoopId(id))
    }
}

fn input_slot(id: i32) -> Result<u8, DispatchError> {
    if (1..=2).contains(&id) {
        Ok((id - 1) as u8)
    } else {
        Err(DispatchError::InvalidInputId(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{KeyInputEvent, LoopClickedEvent, MouseButtonInputEvent};
    use std::path::Path;

    fn shipped_config() -> FloConfig {
        let mut config = FloConfig::new();
        config
            .load_authoritative(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../data/fweelin.xml")
                    .as_path(),
            )
            .unwrap();
        config
    }

    fn outputs(batch: &ActionBatch<32>) -> Vec<DispatchOutput> {
        batch.iter().cloned().collect()
    }

    #[test]
    fn shipped_keyboard_binding_reaches_application_action() {
        let mut config = shipped_config();
        // SDL 1.2-compatible F3 keycode. This module is also compiled by an
        // integration-test path where `sdlkey_compat` is not re-exported.
        let input = KeyInputEvent::new(true, 284, 0);
        let registry = config.binding_registry.clone();
        let batch = RuntimeEventDispatcher::<32>::new()
            .dispatch(
                &mut config,
                &registry,
                &input,
                &[LoopMode::Empty; MAX_RUNTIME_LOOPS],
            )
            .unwrap();

        assert_eq!(
            outputs(&batch),
            vec![DispatchOutput::Application(
                ApplicationAction::SetFullscreen(true)
            )]
        );
    }

    #[test]
    fn shipped_loop_mouse_binding_reaches_record_command() {
        let mut config = shipped_config();
        let input = LoopClickedEvent::new(true, 1, 3, true);
        let registry = config.binding_registry.clone();
        let batch = RuntimeEventDispatcher::<32>::new()
            .dispatch(
                &mut config,
                &registry,
                &input,
                &[LoopMode::Empty; MAX_RUNTIME_LOOPS],
            )
            .unwrap();

        assert_eq!(
            outputs(&batch),
            vec![DispatchOutput::Runtime(RuntimeCommand::Record { slot: 3 })]
        );
    }

    #[test]
    fn shipped_loop_wheel_bindings_adjust_gain() {
        let registry_and_batch = |button| {
            let mut config = shipped_config();
            let input = LoopClickedEvent::new(true, button, 3, true);
            let registry = config.binding_registry.clone();
            RuntimeEventDispatcher::<32>::new()
                .dispatch(
                    &mut config,
                    &registry,
                    &input,
                    &[LoopMode::Playing; MAX_RUNTIME_LOOPS],
                )
                .unwrap()
        };

        assert_eq!(
            outputs(&registry_and_batch(4)),
            vec![DispatchOutput::Application(
                ApplicationAction::AdjustLoopGain {
                    loop_id: 3,
                    factor: 1.0 / 0.9,
                }
            )]
        );
        assert_eq!(
            outputs(&registry_and_batch(5)),
            vec![DispatchOutput::Application(
                ApplicationAction::AdjustLoopGain {
                    loop_id: 3,
                    factor: 0.9,
                }
            )]
        );
    }

    #[test]
    fn shipped_qwerty_binding_preserves_the_visible_legacy_loop_id() {
        let mut config = shipped_config();
        // The pckeyboard XML and graphics both address Q as SDL/ASCII 113.
        // Do not renumber this to a compact internal slot: doing so makes a
        // working keypress appear inert because the visible element is 113.
        let input = KeyInputEvent::new(true, b'q' as i32, 0);
        let registry = config.binding_registry.clone();
        let batch = RuntimeEventDispatcher::<32>::new()
            .dispatch(
                &mut config,
                &registry,
                &input,
                &[LoopMode::Empty; MAX_RUNTIME_LOOPS],
            )
            .unwrap();

        assert_eq!(
            outputs(&batch),
            vec![DispatchOutput::Runtime(RuntimeCommand::Record {
                slot: b'q'
            })]
        );
    }

    #[test]
    fn shipped_f1_binding_selects_the_primary_pulse() {
        let mut config = shipped_config();
        let input = KeyInputEvent::new(true, 282, 0);
        let registry = config.binding_registry.clone();
        let batch = RuntimeEventDispatcher::<32>::new()
            .dispatch(
                &mut config,
                &registry,
                &input,
                &[LoopMode::Empty; MAX_RUNTIME_LOOPS],
            )
            .unwrap();

        assert_eq!(
            outputs(&batch),
            vec![DispatchOutput::Application(ApplicationAction::SelectPulse(
                0
            ))]
        );
    }

    #[test]
    fn sync_type_and_speed_events_reach_runtime_state_actions() {
        let mut config = FloConfig::new();
        let registry = BindingRegistry::default();
        let modes = [LoopMode::Empty; MAX_RUNTIME_LOOPS];
        let dispatcher = RuntimeEventDispatcher::<32>::new();
        let type_batch = dispatcher
            .dispatch(
                &mut config,
                &registry,
                &crate::event::SetSyncTypeEvent::new(true),
                &modes,
            )
            .unwrap();
        assert_eq!(
            outputs(&type_batch),
            vec![DispatchOutput::Application(ApplicationAction::SetSyncType(
                1
            ))]
        );
        let speed_batch = dispatcher
            .dispatch(
                &mut config,
                &registry,
                &crate::event::SetSyncSpeedEvent::new(3),
                &modes,
            )
            .unwrap();
        assert_eq!(
            outputs(&speed_batch),
            vec![DispatchOutput::Application(
                ApplicationAction::SetSyncSpeed(3)
            )]
        );
    }

    #[test]
    fn midi_transpose_event_reaches_the_live_midi_action() {
        let mut config = FloConfig::new();
        let registry = BindingRegistry::default();
        let batch = RuntimeEventDispatcher::<32>::new()
            .dispatch(
                &mut config,
                &registry,
                &crate::event::AdjustMidiTransposeEvent::new(-12),
                &[LoopMode::Empty; MAX_RUNTIME_LOOPS],
            )
            .unwrap();
        assert_eq!(
            outputs(&batch),
            vec![DispatchOutput::Application(
                ApplicationAction::AdjustMidiTranspose(-12)
            )]
        );
    }

    #[test]
    fn configured_midi_output_preserves_direct_port_and_patch_flag() {
        let mut config = FloConfig::new();
        let registry = config
            .parse_binding_registry_xml(
                0,
                r#"<interface><bindings>
                    <binding input="key" output="midicontroller"
                     parameters="outport=2 and midichannel=3 and controlnum=7 and controlval=99 and routethroughpatch=0"/>
                   </bindings></interface>"#,
            )
            .unwrap();
        let batch = RuntimeEventDispatcher::<32>::new()
            .dispatch(
                &mut config,
                &registry,
                &KeyInputEvent::new(true, 65, 0),
                &[LoopMode::Empty; MAX_RUNTIME_LOOPS],
            )
            .unwrap();
        assert_eq!(
            outputs(&batch),
            vec![
                DispatchOutput::Runtime(RuntimeCommand::SynthController {
                    channel: 3,
                    control: 7,
                    value: 99,
                }),
                DispatchOutput::Application(ApplicationAction::OutputMidi {
                    message: MidiMessage::Controller {
                        channel: 3,
                        control: 7,
                        value: 99,
                    },
                    outport: 2,
                    route_through_patch: false,
                }),
            ]
        );
    }

    #[test]
    fn raw_mouse_button_is_not_mapped_by_shipped_config() {
        let mut config = shipped_config();
        let input = MouseButtonInputEvent::new(true, 1, 100, 100);
        let registry = config.binding_registry.clone();
        let batch = RuntimeEventDispatcher::<32>::new()
            .dispatch(
                &mut config,
                &registry,
                &input,
                &[LoopMode::Empty; MAX_RUNTIME_LOOPS],
            )
            .unwrap();

        // The native bridge must turn a hit-tested mouse click into
        // LoopClickedEvent; this dispatcher intentionally has no coordinates
        // to loop-slot mapping and therefore emits nothing for raw mouse input.
        assert!(batch.is_empty());
    }
}
