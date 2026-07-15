use freewheeling_plus::core::{CoreEvent, LoopSnapshot, LoopStatus, Snapshot, StreamState};
use freewheeling_plus::core_startup::StartupConfig;
use freewheeling_plus::fluidsynth::{FluidSynthBackend, Patch};
use freewheeling_plus::native_startup::{NativePaths, NativeStartupAdapter, StartupPhase};
use freewheeling_plus::production_app::NativeComponentAdapter;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub const PHASES: [StartupPhase; 19] = [
    StartupPhase::LockMemory,
    StartupPhase::RtThreads,
    StartupPhase::MainThread,
    StartupPhase::PlatformThreads,
    StartupPhase::Sdl,
    StartupPhase::MemoryManager,
    StartupPhase::EventManager,
    StartupPhase::Video,
    StartupPhase::VideoReady,
    StartupPhase::Audio,
    StartupPhase::CoreGraph,
    StartupPhase::SynthAndBuffers,
    StartupPhase::Browsers,
    StartupPhase::InputAndMidi,
    StartupPhase::OscAndMixer,
    StartupPhase::SystemVariables,
    StartupPhase::SignalProcessing,
    StartupPhase::StreamersAndRings,
    StartupPhase::ProcessingElements,
];

#[derive(Default)]
pub struct FakeConfig {
    pub calls: Rc<RefCell<Vec<String>>>,
}

impl StartupConfig for FakeConfig {
    fn add_int_constant(&mut self, name: &str, _: i32) {
        self.calls.borrow_mut().push(format!("constant:{name}"));
    }
    fn add_empty_variable(&mut self, name: &str) {
        self.calls.borrow_mut().push(format!("variable:{name}"));
    }
    fn parse(&mut self) -> Result<(), String> {
        self.calls.borrow_mut().push("config:parse".into());
        Ok(())
    }
    fn start(&mut self) -> Result<(), String> {
        self.calls.borrow_mut().push("config:start".into());
        Ok(())
    }
}

#[derive(Clone)]
pub struct FakeStartup {
    pub log: Rc<RefCell<Vec<String>>>,
    pub fail_at: Option<StartupPhase>,
}

impl NativeStartupAdapter for FakeStartup {
    fn start(&mut self, phase: StartupPhase, _: &NativePaths) -> Result<(), String> {
        self.log.borrow_mut().push(format!("start:{phase}"));
        if self.fail_at == Some(phase) {
            Err("injected native failure".into())
        } else {
            Ok(())
        }
    }
    fn rollback(&mut self, phase: StartupPhase) {
        self.log.borrow_mut().push(format!("rollback:{phase}"));
    }
}

pub struct ComponentState {
    pub log: Vec<String>,
    pub stream: StreamState,
    pub bytes: u64,
    pub loops: Vec<LoopSnapshot>,
    pub restored: Option<Snapshot>,
    pub device_lost: bool,
}

impl Default for ComponentState {
    fn default() -> Self {
        Self {
            log: Vec::new(),
            stream: StreamState::Stopped,
            bytes: 0,
            loops: Vec::new(),
            restored: None,
            device_lost: false,
        }
    }
}

pub struct FakeNative {
    pub state: Rc<RefCell<ComponentState>>,
    events: VecDeque<CoreEvent>,
    stream_file: PathBuf,
}

impl FakeNative {
    pub fn new(
        state: Rc<RefCell<ComponentState>>,
        events: impl IntoIterator<Item = CoreEvent>,
        stream_file: PathBuf,
    ) -> Self {
        Self {
            state,
            events: events.into_iter().collect(),
            stream_file,
        }
    }
    pub fn lose_device_and_restart(&mut self) {
        let mut state = self.state.borrow_mut();
        state.device_lost = true;
        state.log.extend(
            [
                "device:lost",
                "audio:quiesce",
                "audio:close",
                "audio:open",
                "audio:activate",
            ]
            .map(str::to_owned),
        );
        state.device_lost = false;
    }
    pub fn reload_stream(&self) -> Vec<u8> {
        fs::read(&self.stream_file).expect("saved fake-native stream")
    }
}

impl NativeComponentAdapter for FakeNative {
    fn start_session(&mut self) -> Result<(), String> {
        self.state.borrow_mut().log.push("session:start".into());
        Ok(())
    }
    fn start_interfaces(&mut self) -> Result<(), String> {
        self.state.borrow_mut().log.push("interfaces:start".into());
        Ok(())
    }
    fn next_event(&mut self) -> Result<Option<CoreEvent>, String> {
        Ok(self.events.pop_front())
    }
    fn set_streaming(&mut self, enabled: bool, sequence: u64) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        if enabled {
            let bytes = format!("FWEELIN-FAKE-STREAM:{sequence}\n").into_bytes();
            fs::write(&self.stream_file, &bytes).map_err(|e| e.to_string())?;
            state.bytes = bytes.len() as u64;
            state.stream = StreamState::Writing;
            state.log.push(format!("stream:start:{sequence}"));
        } else {
            state.stream = StreamState::Stopped;
            state.log.push(format!("stream:stop:{sequence}"));
        }
        Ok(())
    }
    fn stream_state(&self) -> StreamState {
        self.state.borrow().stream
    }
    fn stream_bytes(&self) -> u64 {
        self.state.borrow().bytes
    }
    fn close_video(&mut self) {
        self.state.borrow_mut().log.push("close:video".into());
    }
    fn close_input(&mut self) {
        self.state.borrow_mut().log.push("close:input".into());
    }
    fn close_midi(&mut self) {
        self.state.borrow_mut().log.push("close:midi".into());
    }
    fn close_audio(&mut self) {
        self.state.borrow_mut().log.push("close:audio".into());
    }
    fn release_graph(&mut self) {
        self.state.borrow_mut().log.push("close:graph".into());
    }
    fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
        self.state.borrow().loops.clone()
    }
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        self.state.borrow_mut().restored = Some(snapshot.clone());
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SynthCall {
    Render(usize),
    Note(u8, i32, u8),
    Controller(u8, u8, u8),
    Bend(u8, i32),
    Patch(u8, i32, i32, i32),
    Tuning(f64),
    Shutdown,
}

pub struct FakeFluid {
    pub calls: Arc<Mutex<Vec<SynthCall>>>,
}
impl FluidSynthBackend for FakeFluid {
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.calls
            .lock()
            .unwrap()
            .push(SynthCall::Render(left.len()));
        left.fill(0.0);
        right.fill(0.0);
    }
    fn controller(&mut self, c: u8, k: u8, v: u8) {
        self.calls
            .lock()
            .unwrap()
            .push(SynthCall::Controller(c, k, v));
    }
    fn pitch_bend(&mut self, c: u8, v: i32) {
        self.calls.lock().unwrap().push(SynthCall::Bend(c, v));
    }
    fn note_on(&mut self, c: u8, n: i32, v: u8) {
        self.calls.lock().unwrap().push(SynthCall::Note(c, n, v));
    }
    fn note_off(&mut self, _: u8, _: i32) {}
    fn program_select(&mut self, c: u8, sf: i32, b: i32, p: i32) {
        self.calls
            .lock()
            .unwrap()
            .push(SynthCall::Patch(c, sf, b, p));
    }
    fn set_tuning(&mut self, cents: f64) {
        self.calls.lock().unwrap().push(SynthCall::Tuning(cents));
    }
    fn patches(&self) -> Vec<Patch> {
        vec![]
    }
    fn shutdown(&mut self) {
        self.calls.lock().unwrap().push(SynthCall::Shutdown);
    }
}

pub fn playing_loop() -> LoopSnapshot {
    LoopSnapshot {
        loop_id: 0,
        status: LoopStatus::Playing,
        loop_volume: 0.8,
        trigger_volume: 1.0,
    }
}

pub fn paths(root: PathBuf) -> NativePaths {
    NativePaths {
        resources: root.join("resources"),
        application_support: root.join("support"),
        config: root.join("resources/fweelin.xml"),
    }
}
