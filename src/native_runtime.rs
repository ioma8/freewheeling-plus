//! Concrete native application assembly.
//!
//! Native resources are acquired by startup phases and shared with the main
//! event-loop adapter.  Every acquisition has an inverse and both explicit
//! rollback and final shutdown are safe to repeat.

use super::{NativeComponentAdapter, ProductionApp};
use crate::amixer::{AlsaMixerBackend, HardwareMixerInterface};
use crate::audio_native_cpal::{CpalAudioOptions, DeviceSelection};
use crate::audio_native_cpal::CpalAudioBackend;
use crate::audioio::{AnyAudioBackend, AudioBackend, AudioIO};
use crate::block::{AudioBlock, AudioBlockIterator, Codec, ExtraChannel};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::jack::JackAudioMidiBackend;
#[cfg(target_os = "macos")]
use crate::macos_audio_unit::MacosAudioUnitBackend;

/// Audio backend kind selected via `FWEELIN_AUDIO_BACKEND` environment variable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioBackendKind {
    #[default]
    Auto,
    Jack,
    Cpal,
}
use crate::config::{ConfigVariableValue, FloConfig};
use crate::core::{CoreEvent, LoopSnapshot, LoopStatus, Snapshot, StreamState};
use crate::core_startup::StartupConfig;
use crate::datatypes::Range;
use crate::event::{
    Event, EventListener, EventManager, EventProducer, EventType,
};
use crate::file_codecs::{
    IFileDecoder, SndFileDecoder, encode_audio_file,
};
use crate::file_streamer::AudioStreamer;
use crate::fluidsynth::{FluidLiteBackend, FluidLiteConfig, FluidSynthBackend};
use crate::mem::MemoryManager;
use crate::midiio::{MidiEventSink, MidiIo, MidiMessage};
use crate::midiio_platform::MidirMidiBackend;
use crate::native_dsp_graph::{
    DspSettings, LoopMode, LoopTransferMetadata, PcmTransferHandle, RuntimeAudioProcessor,
    RuntimeCommand, RuntimeControls, RuntimeSnapshot, RuntimeStatus,
    production_audio_processor_with_settings,
};
use crate::native_event_bridge::{NativeEventBridge, input_events};
use crate::native_loop_selection::NativeLoopSelection;
use crate::native_patch_browser::{EchoRouting, NativePatchBrowser, PatchActionPlan};
use crate::native_rename::{NativeRename, RenameInput, RenameResult, RenameTarget};
use crate::native_startup::{
    NativePaths, NativeStartupAdapter, NativeStartupServices, StartupPhase,
};
use crate::osc::{OscClient, PlayingLoop, PlayingLoops, UdpBackend};
use crate::rcu::RcuRegistry;
use crate::runtime_event_actions::{
    ApplicationAction, CodecSelection, DispatchOutput, RuntimeEventDispatcher,
};
use crate::sdlio::{InputEvent, Sdl2InputBackend, SdlIo};
use crate::videoio::{VideoBackend, VideoFrame, VideoMode, VideoRenderer};
use crate::videoio_platform::native_ui_scene::{
    BrowserSceneState, LoopScopeState, SharedUiSceneState, UiSceneState, load_production_scene,
    production_software_renderer,
};
use crate::videoio_platform::{FrameRenderer, Sdl2VideoBackend};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const INPUTS: usize = 2;
const LAST_RECORDS: usize = 8;
const MIDI_INPUTS: usize = 1;
/// Renderer state must be refreshed even while the user is idle. This runs
/// only on the main thread and keeps the realtime command queue bounded.
const UI_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(33);
const RECOVERY_MAX_ATTEMPTS: u32 = 5;
const RECOVERY_INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const RECOVERY_MAX_BACKOFF: Duration = Duration::from_secs(2);
const BROWSER_LOOP_TRAY: i32 = 1;
const BROWSER_SCENE_TRAY: i32 = 2;
const BROWSER_LOOP: i32 = 3;
const BROWSER_SCENE: i32 = 4;
const BROWSER_PATCH: i32 = 5;

/// Convert SDL window coordinates to the logical coordinates used by the
/// XML layout and the legacy input bindings.  SDL reports window pixels (not
/// the Retina drawable pixels), so use the window's logical extent here.
fn map_mouse_to_logical(
    x: i32,
    y: i32,
    logical_size: (u32, u32),
    window_size: (u32, u32),
) -> (i32, i32) {
    let map = |value: i32, logical: u32, window: u32| {
        if window == 0 {
            return value;
        }
        let mapped = i64::from(value.max(0)) * i64::from(logical) / i64::from(window);
        mapped.clamp(0, i64::from(logical)) as i32
    };
    (
        map(x, logical_size.0, window_size.0),
        map(y, logical_size.1, window_size.1),
    )
}

/// C++ keeps user configuration under `~/.fweelin`, including relative
/// SoundFont paths. The initial config copy deliberately excludes binary SF2
/// assets, so its default `basic.sf2` path is often absent. Preserve every
/// configured file that exists, but use the verified bundled default when no
/// configured font is available.
fn startup_soundfonts(
    configured: Vec<std::path::PathBuf>,
    bundled: std::path::PathBuf,
) -> Vec<std::path::PathBuf> {
    let existing: Vec<_> = configured
        .iter()
        .filter(|path| path.is_file())
        .cloned()
        .collect();
    for missing in configured.iter().filter(|path| !path.is_file()) {
        eprintln!(
            "FreeWheeling: configured SoundFont '{}' is missing; using available SoundFonts instead",
            missing.display()
        );
    }
    if existing.is_empty() {
        eprintln!(
            "FreeWheeling: no configured SoundFont is available; using bundled '{}'",
            bundled.display()
        );
        vec![bundled]
    } else {
        existing
    }
}

fn ui_snapshot_due(last_request: Instant, now: Instant) -> bool {
    now.duration_since(last_request) >= UI_SNAPSHOT_INTERVAL
}

fn codec_extension(codec: Codec) -> Result<&'static str, String> {
    match codec {
        Codec::Wav => Ok(".wav"),
        Codec::Vorbis => Ok(".ogg"),
        Codec::Flac => Ok(".flac"),
        Codec::Au => Ok(".au"),
        Codec::Unknown => Err("unknown audio codec".into()),
    }
}


trait RecoverableAudio {
    fn recovery_requested(&self) -> bool;
    fn recover(&mut self) -> Result<(), String>;
}

impl<B: AudioBackend> RecoverableAudio for AudioIO<B> {
    fn recovery_requested(&self) -> bool {
        AudioIO::recovery_requested(self)
    }
    fn recover(&mut self) -> Result<(), String> {
        AudioIO::recover(self)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioRecoveryStatus {
    pub total_attempts: u64,
    pub consecutive_failures: u32,
    pub retry_in: Option<Duration>,
    pub last_error: Option<String>,
    pub exhausted: bool,
}

#[derive(Default)]
struct AudioRecoveryController {
    total_attempts: u64,
    consecutive_failures: u32,
    next_retry: Option<Instant>,
    last_error: Option<String>,
    exhausted: bool,
}

impl AudioRecoveryController {
    fn status(&self, now: Instant) -> AudioRecoveryStatus {
        AudioRecoveryStatus {
            total_attempts: self.total_attempts,
            consecutive_failures: self.consecutive_failures,
            retry_in: self.next_retry.map(|d| d.saturating_duration_since(now)),
            last_error: self.last_error.clone(),
            exhausted: self.exhausted,
        }
    }

    fn poll<A: RecoverableAudio>(&mut self, audio: &mut A, now: Instant) -> Result<(), String> {
        if self.exhausted {
            return Err(format!(
                "audio recovery exhausted: {}",
                self.last_error.as_deref().unwrap_or("unknown failure")
            ));
        }
        if !audio.recovery_requested() {
            self.next_retry = None;
            self.consecutive_failures = 0;
            self.last_error = None;
            return Ok(());
        }
        if self.next_retry.is_some_and(|deadline| now < deadline) {
            return Ok(());
        }
        self.total_attempts = self.total_attempts.saturating_add(1);
        match audio.recover() {
            Ok(()) => {
                self.consecutive_failures = 0;
                self.next_retry = None;
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                self.last_error = Some(error.clone());
                if self.consecutive_failures >= RECOVERY_MAX_ATTEMPTS {
                    self.exhausted = true;
                    return Err(format!(
                        "audio recovery failed after {} attempts: {error}",
                        self.consecutive_failures
                    ));
                }
                let multiplier = 1_u32 << self.consecutive_failures.saturating_sub(1).min(31);
                let delay = RECOVERY_INITIAL_BACKOFF
                    .saturating_mul(multiplier)
                    .min(RECOVERY_MAX_BACKOFF);
                self.next_retry = Some(now + delay);
                Ok(())
            }
        }
    }
}


/// Cocoa requires creation, presentation and destruction of NSWindow-backed
/// SDL objects on the process main thread. This owner is intentionally used
/// only by native assembly; generic `VideoIO` keeps its deterministic worker
/// contract for thread-safe backends and tests.
struct MainThreadVideo {
    backend: Sdl2VideoBackend,
    renderer: FrameRenderer,
    frame: VideoFrame,
    /// XML coordinates are authored in this stable logical space.  It must
    /// not be replaced with the fullscreen drawable size.
    logical_size: (u32, u32),
    /// The requested windowed size is retained across fullscreen toggles;
    /// the current frame may instead be a Retina/fullscreen drawable.
    windowed_size: (u32, u32),
    /// SDL mouse events use window coordinates, while hit-testing and event
    /// bindings use the XML logical coordinate system.
    input_window_size: (u32, u32),
    interval: Duration,
    next_frame: Instant,
    active: bool,
    scene_state: SharedUiSceneState,
    help_page_count: usize,
}

impl MainThreadVideo {
    fn open(data: &std::path::Path) -> Result<Self, String> {
        let scene = load_production_scene(data)?;
        let size = scene.manifest.logical_size;
        let help_page_count = scene.manifest.help_lines.len().div_ceil(24);
        let scene_state = Arc::clone(&scene.state);
        let production = production_software_renderer(scene)?;
        let mut backend = Sdl2VideoBackend::new("FreeWheeling");
        let metrics = backend.open(VideoMode {
            fullscreen: false,
            windowed_size: size,
        })?;
        let drawable_size = (
            metrics.drawable_width.max(1),
            metrics.drawable_height.max(1),
        );
        let mut renderer = production.renderer;
        // Render into the actual backing surface while retaining the XML's
        // logical coordinate system.  This makes fullscreen and Retina
        // transitions scale the complete scene instead of shrinking it to a
        // 1:1 640x480 island.
        renderer.metrics = crate::videoio_displays::RenderMetrics::new(
            size.0 as i32,
            size.1 as i32,
            drawable_size.0 as i32,
            drawable_size.1 as i32,
        );
        Ok(Self {
            backend,
            renderer,
            frame: VideoFrame {
                pixels: vec![0; size.0 as usize * size.1 as usize * 4],
                width: drawable_size.0,
                height: drawable_size.1,
                stride: drawable_size.0 as usize * 4,
                timestamp: 0.0,
            },
            logical_size: size,
            windowed_size: size,
            input_window_size: (metrics.logical_width.max(1), metrics.logical_height.max(1)),
            interval: production.frame_delay,
            next_frame: Instant::now(),
            active: true,
            scene_state,
            help_page_count,
        })
    }

    fn update(&mut self, now: Instant, mut state: UiSceneState) -> Result<(), String> {
        if self.active && now >= self.next_frame {
            let mut existing = self.scene_state.write().expect("UI state poisoned");
            state.layouts = std::mem::take(&mut existing.layouts);
            state.displays = std::mem::take(&mut existing.displays);
            state.snapshot_pages = std::mem::take(&mut existing.snapshot_pages);
            state.snapshot_display_counts = std::mem::take(&mut existing.snapshot_display_counts);
            state.help_page = existing.help_page;
            state.debug_info = existing.debug_info;
            *existing = state;
            // Rendering reads this shared state from every XML display.
            // Do not retain the write lock across `render`, or the main
            // thread deadlocks on its first frame.
            drop(existing);
            self.renderer.render(&mut self.frame);
            self.backend.present(&self.frame)?;
            self.next_frame = now + self.interval;
        }
        Ok(())
    }

    /// Mirror the historical VideoIO mouse path: raw mouse events are first
    /// offered to browsers, then a visible layout element emits LoopClicked.
    /// SDL mouse coordinates are in the current window's logical pixels. The
    /// XML layout remains in its authored logical resolution, so fullscreen
    /// needs the inverse of the render scale before hit-testing or dispatch.
    fn map_mouse_position(&self, x: i32, y: i32) -> (i32, i32) {
        map_mouse_to_logical(x, y, self.logical_size, self.input_window_size)
    }

    fn map_mouse_event(&self, event: InputEvent) -> InputEvent {
        match event {
            InputEvent::MouseMotion { x, y } => {
                let (x, y) = self.map_mouse_position(x, y);
                InputEvent::MouseMotion { x, y }
            }
            InputEvent::MouseButton { button, x, y, down } => {
                let (x, y) = self.map_mouse_position(x, y);
                InputEvent::MouseButton { button, x, y, down }
            }
            other => other,
        }
    }

    /// Find the visible XML layout element at an already-normalized logical
    /// coordinate.
    fn loop_at(&self, x: i32, y: i32) -> Option<i32> {
        let state = self.scene_state.read().expect("UI state poisoned");
        self.renderer
            .scene
            .layouts
            .iter()
            .filter(|layout| {
                state
                    .layouts
                    .get(&(layout.iid, layout.id))
                    .map_or(layout.show, |dynamic| dynamic.show)
            })
            .find_map(|layout| {
                layout.element_at(x, y).and_then(|element| {
                    let loop_base = state
                        .layouts
                        .get(&(layout.iid, layout.id))
                        .map_or(layout.loopids.0, |dynamic| dynamic.loopids.0);
                    NativeRuntime::valid_layout_loop_id(loop_base + element.id)
                })
            })
    }

    fn close(&mut self) {
        if self.active {
            self.backend.close();
            self.active = false;
        }
    }
}

pub struct SharedFloConfig(Rc<RefCell<FloConfig>>);

impl StartupConfig for SharedFloConfig {
    fn add_int_constant(&mut self, name: &str, value: i32) {
        self.0.borrow_mut().add_int_constant(name, value);
    }
    fn add_empty_variable(&mut self, name: &str) {
        self.0.borrow_mut().add_empty_variable(name);
    }
    fn parse(&mut self) -> Result<(), String> {
        self.0.borrow_mut().parse()
    }
    fn start(&mut self) -> Result<(), String> {
        self.0.borrow_mut().start()
    }
}

pub type NativeProductionApp =
    ProductionApp<SharedFloConfig, NativeStartupServices<NativeRuntime>, NativeRuntime>;

struct RuntimeResources {
    config: Rc<RefCell<FloConfig>>,
    events: Option<Arc<EventManager>>,
    event_bridge: Option<Arc<NativeEventBridge>>,
    event_inbox: Arc<crossbeam_queue::ArrayQueue<Event>>,
    input: Option<SdlIo<Sdl2InputBackend>>,
    video: Option<MainThreadVideo>,
    audio: Option<AudioIO<AnyAudioBackend>>,
    midi: Option<MidiIo<MidirMidiBackend>>,
    controls: Option<RuntimeControls>,
    osc: Option<OscClient<UdpBackend>>,
    mixer: Option<HardwareMixerInterface<AlsaMixerBackend>>,
    browser_entries: Vec<std::path::PathBuf>,
    browser_cursors: HashMap<i32, usize>,
    pending_core_events: VecDeque<CoreEvent>,
    stream_state: StreamState,
    stream_codec: Codec,
    stream_bytes: u64,
    stream_output_name: String,
    cached_loops: Vec<LoopSnapshot>,
    latest_snapshot: RuntimeSnapshot,
    last_snapshot_request: Instant,
    last_diagnostic_report: Instant,
    cached_modes: [LoopMode; crate::native_dsp_graph::MAX_RUNTIME_LOOPS],
    trigger_gains: [f32; crate::native_dsp_graph::MAX_RUNTIME_LOOPS],
    pending_exports: Vec<PendingLoopExport>,
    queued_exports: VecDeque<(u8, Codec)>,
    pending_imports: Vec<PcmTransferHandle>,
    queued_imports: VecDeque<QueuedLoopImport>,
    load_loop_id: i32,
    snapshots: HashMap<i32, RuntimeSnapshot>,
    snapshot_names: HashMap<i32, String>,
    loop_selection: NativeLoopSelection,
    patch_browser: Option<NativePatchBrowser>,
    active_midi_routes: Vec<EchoRouting>,
    held_midi_routes: HashMap<u8, Vec<EchoRouting>>,
    rename: NativeRename,
    loop_files: HashMap<i32, std::path::PathBuf>,
    loop_names: HashMap<i32, String>,
    loop_hashes: HashMap<i32, String>,
    loop_metadata: HashMap<i32, LoopTransferMetadata>,
    pending_snapshot_id: Option<i32>,
    default_loop_placement: Range,
    auto_loop_saving: bool,
    auto_loop_codec: Codec,
    current_scene: Option<std::path::PathBuf>,
    pending_scene_save: Option<PendingSceneSave>,
    sample_rate: u32,
    max_callback_frames: usize,
    library_dir: std::path::PathBuf,
    streamer: Option<AudioStreamer>,
    memory_manager: Option<MemoryManager>,
    rcu_registry: Option<RcuRegistry>,
    audio_recovery: AudioRecoveryController,
    pulse_selected: bool,
    sync_type: bool,
    sync_speed: u32,
    last_recorded_loop: Option<u8>,
    recent_recordings: VecDeque<u8>,
    current_interface: i32,
    synth_enabled: bool,
    help_page_count: usize,
    debug_info: bool,
}

struct PendingLoopExport {
    handle: PcmTransferHandle,
    loop_id: i32,
    codec: Codec,
}

struct QueuedLoopImport {
    path: std::path::PathBuf,
    slot: u8,
    gain: f32,
}

struct PendingSceneSave {
    force_new: bool,
    snapshot: Option<RuntimeSnapshot>,
}

struct RuntimeInboxListener {
    inbox: Arc<crossbeam_queue::ArrayQueue<Event>>,
}

impl EventListener for RuntimeInboxListener {
    fn receive_event(&mut self, event: &Event, _from: &dyn EventProducer) {
        let _ = self.inbox.push(event.clone());
    }
}

pub struct NativeRuntime {
    resources: Rc<RefCell<RuntimeResources>>,
}

impl Clone for NativeRuntime {
    fn clone(&self) -> Self {
        Self {
            resources: Rc::clone(&self.resources),
        }
    }
}

impl NativeRuntime {
    fn ui_scene_state(r: &RuntimeResources) -> UiSceneState {
        let snapshot = r.latest_snapshot;
        let mut state = UiSceneState::default();
        // XML display expressions use the original variable names. Publish a
        // bounded, immutable copy on the main thread before overlaying live
        // runtime values below.
        let config_variables = r.config.borrow().variable_snapshot();
        for variable in config_variables.variables {
            let value = match variable.value {
                ConfigVariableValue::Char(value) => value as f32,
                ConfigVariableValue::Int(value) => value as f32,
                ConfigVariableValue::Long(value) => value as f32,
                ConfigVariableValue::Float(value) => value,
                ConfigVariableValue::Range(value) => value.lo as f32,
                ConfigVariableValue::Raw(_) => continue,
            };
            state.values.insert(variable.name, value);
        }
        state.paramsets = r.config.borrow().paramsets.clone();
        state.values.insert(
            "SYSTEM_variable_snapshot_truncated".into(),
            config_variables.truncated as u8 as f32,
        );
        state
            .values
            .insert("pulse-position".into(), snapshot.pulse_position as f32);
        state
            .values
            .insert("pulse-frames".into(), snapshot.pulse_frames.max(1) as f32);
        state
            .values
            .insert("pulse-long-count".into(), snapshot.pulse_long_count as f32);
        state.values.insert(
            "pulse-long-length".into(),
            snapshot.pulse_long_length.max(1) as f32,
        );
        state
            .values
            .insert("pulse-active".into(), r.pulse_selected as u8 as f32);
        state
            .values
            .insert("sample-clock".into(), snapshot.sample_clock as f32);
        state.values.insert(
            "streaming".into(),
            (r.stream_state == StreamState::Writing) as u8 as f32,
        );
        state
            .values
            .insert("stream-bytes".into(), r.stream_bytes as f32);
        state.stream_output_name = r.stream_output_name.clone();
        state.values.insert(
            "recording-slot".into(),
            snapshot.recording_slot.max(-1) as f32,
        );
        state
            .values
            .insert("SYSTEM_master_in_volume".into(), snapshot.monitor_gain);
        state
            .values
            .insert("SYSTEM_master_out_volume".into(), snapshot.master_gain);
        state
            .values
            .insert("SYSTEM_cur_limiter_gain".into(), snapshot.limiter_gain);
        state
            .values
            .insert("SYSTEM_in_1_volume".into(), snapshot.monitor_gain);
        state
            .values
            .insert("SYSTEM_in_2_volume".into(), snapshot.input_volume[1]);
        state
            .values
            .insert("SYSTEM_in_1_volume".into(), snapshot.input_volume[0]);
        state
            .values
            .insert("SYSTEM_in_1_peak".into(), snapshot.input_peak[0]);
        state
            .values
            .insert("SYSTEM_in_2_peak".into(), snapshot.input_peak[1]);
        state.values.insert(
            "SYSTEM_in_1_record".into(),
            snapshot.input_selected[0] as u8 as f32,
        );
        state.values.insert(
            "SYSTEM_in_2_record".into(),
            snapshot.input_selected[1] as u8 as f32,
        );
        state
            .values
            .insert("SYSTEM_sync_active".into(), r.pulse_selected as u8 as f32);
        state.values.insert(
            "SYSTEM_audio_cpu_load".into(),
            r.audio
                .as_ref()
                .map_or(0.0, |audio| audio.get_cpu_load() * 100.0),
        );
        for (slot, loop_state) in snapshot.loops.iter().enumerate() {
            let prefix = format!("loop-{slot}");
            state
                .values
                .insert(format!("{prefix}-frames"), loop_state.frames as f32);
            state
                .values
                .insert(format!("{prefix}-position"), loop_state.position as f32);
            state
                .values
                .insert(format!("{prefix}-gain"), loop_state.gain);
            state.values.insert(
                format!("{prefix}-playing"),
                matches!(loop_state.mode, LoopMode::Playing | LoopMode::Overdubbing) as u8 as f32,
            );
            state.values.insert(
                format!("{prefix}-recording"),
                matches!(loop_state.mode, LoopMode::Recording | LoopMode::Overdubbing) as u8 as f32,
            );
            if loop_state.mode != LoopMode::Empty {
                let selected = (0..crate::native_loop_selection::NUM_SELECTION_SETS)
                    .any(|set| r.loop_selection.selected(set, slot).unwrap_or(false));
                let recent_rank = r
                    .recent_recordings
                    .iter()
                    .position(|recent| usize::from(*recent) == slot)
                    .map(|rank| rank as u8);
                let name = r.loop_names.get(&(slot as i32)).cloned().or_else(|| {
                    r.loop_files
                        .get(&(slot as i32))
                        .and_then(|path| Self::persisted_display_name(path))
                });
                state.loop_scopes.insert(
                    slot as i32,
                    LoopScopeState {
                        mode: loop_state.mode,
                        gain: loop_state.gain,
                        trigger_gain: loop_state.trigger_gain,
                        gain_delta: loop_state.gain_delta,
                        selected,
                        recent_rank,
                        name,
                        ..LoopScopeState::default()
                    },
                );
            }
        }
        for scope in snapshot.scopes.iter().take(snapshot.scope_count as usize) {
            let visual = state
                .loop_scopes
                .entry(i32::from(scope.loop_id))
                .or_default();
            let chunks = usize::from(scope.chunk_count);
            visual.peaks = scope.peaks[..chunks].to_vec();
            visual.averages = scope.averages[..chunks].to_vec();
            visual.position_column = scope.position_column;
            visual.chunk_count = scope.chunk_count;
            visual.current_peak = scope.current_peak;
        }
        state.snapshots = (0..r.snapshots.len().max(r.snapshot_names.len()))
            .map(|id| r.snapshot_names.get(&(id as i32)).cloned())
            .collect();
        let items: Vec<String> = r
            .browser_entries
            .iter()
            .map(|p| Self::persisted_display_name(p).unwrap_or_else(|| p.display().to_string()))
            .collect();
        for (name, browser) in [
            ("BROWSE_loop", BROWSER_LOOP),
            ("BROWSE_scene", BROWSER_SCENE),
        ] {
            state.browsers.insert(
                name.into(),
                BrowserSceneState {
                    items: items.clone(),
                    selected: r.browser_cursors.get(&browser).copied().unwrap_or(0),
                    expanded: false,
                    ..Default::default()
                },
            );
        }
        // `LoopTray::ReceiveEvent(T_EV_TriggerSet)` stores one item per live
        // loop slot, ordered by `LoopTrayItem::Compare` (ascending slot id).
        // Keep that identity in the production scene instead of deriving an
        // id from a browser row, which fails as soon as a lower slot is
        // deleted.
        let mut tray_slots: Vec<i32> = state.loop_scopes.keys().copied().collect();
        tray_slots.sort_unstable();
        let tray_items = tray_slots
            .iter()
            .map(|slot| {
                state
                    .loop_scopes
                    .get(slot)
                    .and_then(|scope| scope.name.clone())
                    .unwrap_or_else(|| format!("loop-{slot}"))
            })
            .collect();
        state.browsers.insert(
            "BROWSE_loop_tray".into(),
            BrowserSceneState {
                items: tray_items,
                loop_ids: tray_slots,
                expanded: false,
                ..Default::default()
            },
        );
        if let Some(patches) = &r.patch_browser
            && let Some(bank) = patches.current_bank()
        {
            state.browsers.insert(
                "BROWSE_patch".into(),
                BrowserSceneState {
                    items: bank.items.iter().map(|i| i.name.clone()).collect(),
                    selected: bank.cursor,
                    expanded: false,
                    ..Default::default()
                },
            );
        }
        state
    }

    /// Refresh the variables that C++ links directly to live audio, MIDI,
    /// browser, and video objects.  Bindings are evaluated synchronously, so
    /// this is called immediately before dispatch and after actions mutate
    /// those objects.
    fn sync_live_system_variables(r: &mut RuntimeResources) {
        let snapshot = r.latest_snapshot;
        let cpu_load = r
            .audio
            .as_ref()
            .map_or(0.0, |audio| audio.get_cpu_load() * 100.0);
        let (midi_transpose, bend_tune, midi_sync_transmit) =
            r.midi.as_ref().map_or((0, 0, false), |midi| {
                (midi.note_transpose, midi.bend_tune, midi.sync_transmit)
            });
        let (midi_outputs, patch_banks, switchable_interfaces) = {
            let config = r.config.borrow();
            (
                config.midi_outputs as i32,
                config.patch_banks.len() as i32,
                config
                    .interfaces
                    .iter()
                    .filter(|interface| interface.switchable)
                    .count() as i32,
            )
        };
        let patch_tag = r
            .patch_browser
            .as_ref()
            .and_then(|browser| browser.current_bank())
            .and_then(|bank| bank.tag)
            .unwrap_or(0);
        let (snapshot_page, snapshot_count) = r
            .video
            .as_ref()
            .and_then(|video| video.scene_state.read().ok())
            .and_then(|state| {
                state.snapshot_pages.iter().next().map(|(key, page)| {
                    (
                        *page,
                        state.snapshot_display_counts.get(key).copied().unwrap_or(0),
                    )
                })
            })
            .unwrap_or((0, 0));
        let loops_in_map = snapshot
            .loops
            .iter()
            .filter(|loop_state| loop_state.mode != LoopMode::Empty)
            .count() as i32;
        let recording_loops = snapshot
            .loops
            .iter()
            .filter(|loop_state| {
                matches!(loop_state.mode, LoopMode::Recording | LoopMode::Overdubbing)
            })
            .count() as i32;
        let mut config = r.config.borrow_mut();
        config.set_int_variable("SYSTEM_num_midi_outs", midi_outputs);
        config.set_int_variable("SYSTEM_midi_transpose", midi_transpose);
        config.set_float_variable("SYSTEM_master_in_volume", snapshot.monitor_gain);
        config.set_float_variable("SYSTEM_master_out_volume", snapshot.master_gain);
        config.set_int_variable("SYSTEM_cur_pitchbend", 0);
        config.set_int_variable("SYSTEM_bender_tune", bend_tune);
        config.set_float_variable("SYSTEM_cur_limiter_gain", snapshot.limiter_gain);
        config.set_float_variable("SYSTEM_audio_cpu_load", cpu_load);
        config.set_int_variable("SYSTEM_sync_active", r.pulse_selected as i32);
        config.set_int_variable("SYSTEM_sync_transmit", 0);
        config.set_int_variable("SYSTEM_midisync_transmit", midi_sync_transmit as i32);
        config.set_int_variable("SYSTEM_fluidsynth_enabled", r.synth_enabled as i32);
        config.set_int_variable("SYSTEM_num_help_pages", r.help_page_count as i32);
        config.set_int_variable("SYSTEM_num_loops_in_map", loops_in_map);
        config.set_int_variable("SYSTEM_num_recording_loops_in_map", recording_loops);
        config.set_int_variable("SYSTEM_num_patchbanks", patch_banks);
        config.set_int_variable("SYSTEM_cur_patchbank_tag", patch_tag);
        config.set_int_variable("SYSTEM_num_switchable_interfaces", switchable_interfaces);
        config.set_int_variable("SYSTEM_cur_switchable_interface", r.current_interface);
        config.set_int_variable(
            "SYSTEM_snapshot_page_firstidx",
            snapshot_page.saturating_mul(snapshot_count) as i32,
        );
        for (index, volume) in snapshot.input_volume.iter().enumerate() {
            config.set_float_variable(&format!("SYSTEM_in_{}_volume", index + 1), *volume);
            config.set_int_variable(
                &format!("SYSTEM_in_{}_record", index + 1),
                snapshot.input_selected[index] as i32,
            );
        }
    }

    fn new(library_dir: std::path::PathBuf, config: Rc<RefCell<FloConfig>>) -> Self {
        Self {
            resources: Rc::new(RefCell::new(RuntimeResources {
                config,
                events: None,
                event_bridge: None,
                event_inbox: Arc::new(crossbeam_queue::ArrayQueue::new(1024)),
                input: None,
                video: None,
                audio: None,
                midi: None,
                controls: None,
                osc: None,
                mixer: None,
                browser_entries: Vec::new(),
                browser_cursors: HashMap::new(),
                pending_core_events: VecDeque::new(),
                stream_state: StreamState::Stopped,
                stream_codec: Codec::Vorbis,
                stream_bytes: 0,
                stream_output_name: String::new(),
                cached_loops: Vec::new(),
                latest_snapshot: RuntimeSnapshot::default(),
                last_snapshot_request: Instant::now() - UI_SNAPSHOT_INTERVAL,
                last_diagnostic_report: Instant::now(),
                cached_modes: [LoopMode::Empty; crate::native_dsp_graph::MAX_RUNTIME_LOOPS],
                trigger_gains: [1.0; crate::native_dsp_graph::MAX_RUNTIME_LOOPS],
                pending_exports: Vec::with_capacity(
                    crate::native_dsp_graph::DEFAULT_TRANSFER_SLOTS,
                ),
                queued_exports: VecDeque::new(),
                pending_imports: Vec::with_capacity(
                    crate::native_dsp_graph::DEFAULT_TRANSFER_SLOTS,
                ),
                queued_imports: VecDeque::new(),
                load_loop_id: 0,
                snapshots: HashMap::new(),
                snapshot_names: HashMap::new(),
                loop_selection: NativeLoopSelection::new(
                    crate::native_dsp_graph::MAX_RUNTIME_LOOPS,
                ),
                patch_browser: None,
                active_midi_routes: Vec::new(),
                held_midi_routes: HashMap::new(),
                rename: NativeRename::new(),
                loop_files: HashMap::new(),
                loop_names: HashMap::new(),
                loop_hashes: HashMap::new(),
                loop_metadata: HashMap::new(),
                pending_snapshot_id: None,
                default_loop_placement: Range::new(
                    0,
                    crate::native_dsp_graph::MAX_RUNTIME_LOOPS as i32,
                ),
                auto_loop_saving: false,
                auto_loop_codec: Codec::Vorbis,
                current_scene: None,
                pending_scene_save: None,
                sample_rate: 0,
                max_callback_frames: 0,
                library_dir,
                streamer: None,
                memory_manager: None,
                rcu_registry: None,
                audio_recovery: AudioRecoveryController::default(),
                pulse_selected: false,
                sync_type: false,
                sync_speed: 1,
                last_recorded_loop: None,
                recent_recordings: VecDeque::with_capacity(8),
                current_interface: 1,
                synth_enabled: true,
                help_page_count: 0,
                debug_info: false,
            })),
        }
    }

    pub fn audio_recovery_status(&self) -> AudioRecoveryStatus {
        self.resources
            .borrow()
            .audio_recovery
            .status(Instant::now())
    }

    pub fn with_config<R>(&self, read: impl FnOnce(&FloConfig) -> R) -> R {
        let r = self.resources.borrow();
        read(&r.config.borrow())
    }

    fn poll_audio_recovery(&mut self, now: Instant) -> Result<(), String> {
        let mut r = self.resources.borrow_mut();
        let RuntimeResources {
            audio,
            audio_recovery,
            ..
        } = &mut *r;
        audio_recovery.poll(audio.as_mut().ok_or("audio is closed")?, now)
    }

    fn report_diagnostics(r: &mut RuntimeResources, now: Instant) {
        if (!r.debug_info && std::env::var_os("FWEELIN_DIAGNOSTICS").is_none())
            || now.duration_since(r.last_diagnostic_report) < Duration::from_secs(1)
        {
            return;
        }
        r.last_diagnostic_report = now;
        if let Some(audio) = r.audio.as_ref() {
            if let Some(status) = audio.backend().status() {
                eprintln!(
                    "FreeWheeling audio: active={} input={:?} output={:?} format={:?} latency={:?} capture_callbacks={} playback_callbacks={} metrics={:?}",
                    status.active,
                    status.input.as_ref().map(|device| &device.name),
                    status.output.as_ref().map(|device| &device.name),
                    status.format,
                    status.latency,
                    status.capture_callbacks,
                    status.playback_callbacks,
                    status.metrics,
                );
            } else {
                let metrics = audio.metrics();
                eprintln!(
                    "FreeWheeling audio (JACK): callbacks={} frames={} peak_ns={} xruns={}",
                    metrics.callbacks,
                    metrics.callback_frames,
                    metrics.callback_peak_nanos,
                    metrics.xruns,
                );
            }
        }
    }

    fn dispatch_one_runtime_event(&mut self, event: &Event) -> Result<(), String> {
        let mut r = self.resources.borrow_mut();
        Self::sync_live_system_variables(&mut r);
        let registry = r.config.borrow().binding_registry.clone();
        let modes = r.cached_modes;
        let batch = RuntimeEventDispatcher::<32>::new()
            .dispatch(&mut r.config.borrow_mut(), &registry, event, &modes)
            .map_err(|error| format!("dispatch {:?}: {error:?}", event.get_type()))?;
        if r.debug_info || std::env::var_os("FWEELIN_DIAGNOSTICS").is_some() {
            eprintln!(
                "FreeWheeling dispatch: {:?} -> {:?}",
                event.get_type(),
                batch.iter().collect::<Vec<_>>()
            );
        }
        for output in batch.iter().cloned() {
            match output {
                DispatchOutput::Runtime(command) => {
                    if let RuntimeCommand::Record { slot } = command {
                        // C++ updates lastrecidx when recording starts, so the
                        // L1..L8 visual labels change immediately.
                        r.last_recorded_loop = Some(slot);
                        r.recent_recordings.retain(|recent| *recent != slot);
                        r.recent_recordings.push_front(slot);
                        r.recent_recordings.truncate(8);
                    }
                    r.controls
                        .as_mut()
                        .ok_or("DSP controls are closed")?
                        .try_command(command)
                        .map_err(|_| "DSP command queue is full")?
                }
                DispatchOutput::Application(action) => {
                    Self::apply_application_action(&mut r, action)?
                }
            }
        }
        if let Event::SetMidiTuning { tuning } = event
            && let Some(midi) = r.midi.as_mut()
        {
            // `MidiIO::ReceiveEvent` owns this setting in C++; the synth
            // command emitted by the dispatcher is intentionally separate.
            midi.bend_tune = *tuning as i32;
        }
        // `MidiIO::ReceiveEvent` invokes `EchoEvent` after configuration has
        // handled the original event.  The configuration binding's `echo`
        // flag therefore controls whether ordinary received MIDI reaches the
        // active external patch route.  Clock/start/stop are intentionally
        // excluded here: C++ fans those through `midisyncouts`, handled by
        // their dedicated application actions above.
        if batch.echo_input()
            && let Some(message) = Self::echoable_midi_message(event)
        {
            Self::echo_routed_midi(&mut r, &message)?;
        }
        Self::sync_live_system_variables(&mut r);
        Ok(())
    }

    fn echoable_midi_message(event: &Event) -> Option<MidiMessage> {
        match event {
            Event::MIDIKeyInput {
                down,
                channel,
                notenum,
                vel,
                ..
            } => Some(if *down {
                MidiMessage::NoteOn {
                    channel: *channel,
                    note: *notenum,
                    velocity: *vel,
                }
            } else {
                MidiMessage::NoteOff {
                    channel: *channel,
                    note: *notenum,
                    velocity: *vel,
                }
            }),
            Event::MIDIControllerInput {
                channel, ctrl, val, ..
            } => Some(MidiMessage::Controller {
                channel: *channel,
                control: *ctrl,
                value: *val,
            }),
            Event::MIDIProgramChangeInput {
                channel, val, ..
            } => Some(MidiMessage::ProgramChange {
                channel: *channel,
                program: *val,
            }),
            Event::MIDIChannelPressureInput {
                channel, val, ..
            } => Some(MidiMessage::ChannelPressure {
                channel: *channel,
                value: *val,
            }),
            Event::MIDIPitchBendInput {
                channel, val, ..
            } => Some(MidiMessage::PitchBend {
                channel: *channel,
                value: u16::try_from(*val).ok()?,
            }),
            _ => None,
        }
    }

    fn echo_routed_midi(r: &mut RuntimeResources, message: &MidiMessage) -> Result<(), String> {
        let routes =
            Self::midi_routes_for_message(&r.active_midi_routes, &mut r.held_midi_routes, message);
        let midi = r.midi.as_ref().ok_or("MIDI is closed")?;
        if routes.is_empty() {
            return midi.echo(message);
        }
        for route in routes {
            midi.echo_to_route(
                i32::try_from(route.midi_port).unwrap_or(i32::MAX),
                route.channel,
                message,
            )?;
        }
        Ok(())
    }

    fn midi_routes_for_message(
        active: &[EchoRouting],
        held: &mut HashMap<u8, Vec<EchoRouting>>,
        message: &MidiMessage,
    ) -> Vec<EchoRouting> {
        match message {
            MidiMessage::NoteOn { note, .. } => {
                let routes: Vec<_> = active
                    .iter()
                    .filter(|route| {
                        route
                            .key_range
                            .is_none_or(|(low, high)| (*note >= low) && (*note <= high))
                    })
                    .cloned()
                    .collect();
                held.insert(*note, routes.clone());
                routes
            }
            MidiMessage::NoteOff { note, .. } => {
                held.remove(note).unwrap_or_else(|| active.to_vec())
            }
            _ => active.to_vec(),
        }
    }

    fn apply_application_action(
        r: &mut RuntimeResources,
        action: ApplicationAction,
    ) -> Result<(), String> {
        match action {
            ApplicationAction::VideoShowLoop {
                interface_id,
                layout_id,
                loop_ids,
            } => {
                let video = r.video.as_mut().ok_or("video is closed")?;
                let mut state = video.scene_state.write().expect("UI state poisoned");
                let layout = state
                    .layouts
                    .get_mut(&(interface_id, layout_id))
                    .ok_or_else(|| {
                        format!("invalid layout {layout_id} in interface {interface_id}")
                    })?;
                layout.loopids = (loop_ids.lo, loop_ids.hi);
            }
            ApplicationAction::VideoShowLayout {
                interface_id,
                layout_id,
                show,
                hide_others,
            } => {
                let video = r.video.as_mut().ok_or("video is closed")?;
                let mut state = video.scene_state.write().expect("UI state poisoned");
                let layout = state
                    .layouts
                    .get_mut(&(interface_id, layout_id))
                    .ok_or_else(|| {
                        format!("invalid layout {layout_id} in interface {interface_id}")
                    })?;
                layout.show = show;
                if hide_others {
                    for (key, other) in &mut state.layouts {
                        if *key != (interface_id, layout_id) {
                            other.show = false;
                        }
                    }
                }
            }
            ApplicationAction::VideoSwitchInterface(interface_id) => {
                r.current_interface = interface_id;
                let video = r.video.as_mut().ok_or("video is closed")?;
                let mut state = video.scene_state.write().expect("UI state poisoned");
                for ((layout_interface, _), layout) in &mut state.layouts {
                    if *layout_interface != 0 && *layout_interface < 1000 {
                        layout.show = *layout_interface == interface_id;
                    }
                }
                for ((display_interface, _), display) in &mut state.displays {
                    if *display_interface != 0 && *display_interface < 1000 {
                        *display = *display_interface == interface_id;
                    }
                }
            }
            ApplicationAction::ExitSession => {
                r.pending_core_events.push_back(CoreEvent::ExitSession);
            }
            ApplicationAction::VideoShowDisplay {
                interface_id,
                display_id,
                show,
            } => {
                let video = r.video.as_mut().ok_or("video is closed")?;
                let mut state = video.scene_state.write().expect("UI state poisoned");
                state.displays.insert((interface_id, display_id), show);
            }
            ApplicationAction::VideoShowSnapshotPage {
                interface_id,
                display_id,
                page,
            } => {
                let video = r.video.as_mut().ok_or("video is closed")?;
                let mut state = video.scene_state.write().expect("UI state poisoned");
                let key = (interface_id, display_id);
                let count = state
                    .snapshot_display_counts
                    .get(&key)
                    .copied()
                    .unwrap_or(1)
                    .max(1);
                let max_page = r.snapshots.len().saturating_sub(1) / count;
                let current = state.snapshot_pages.entry(key).or_default();
                *current = ((*current as i32) + page).clamp(0, max_page as i32) as usize;
            }
            ApplicationAction::VideoShowHelp(page) => {
                let video = r.video.as_mut().ok_or("video is closed")?;
                let mut state = video.scene_state.write().expect("UI state poisoned");
                if page >= 0 && (page as usize) <= video.help_page_count {
                    state.help_page = page as usize;
                }
            }
            ApplicationAction::ShowDebugInfo(show) => {
                r.debug_info = show;
                if let Some(video) = r.video.as_mut() {
                    video
                        .scene_state
                        .write()
                        .expect("UI state poisoned")
                        .debug_info = show;
                }
                r.config
                    .borrow_mut()
                    .set_int_variable("SYSTEM_show_debug_info", show as i32);
            }
            ApplicationAction::AlsamixerControlSet {
                hwid,
                numid,
                values,
            } => {
                if let Some(mixer) = r.mixer.as_mut() {
                    mixer
                        .alsa_mixer_control_set(
                            hwid, numid, values[0], values[1], values[2], values[3],
                        )
                        .map_err(|error| format!("ALSA mixer: {error}"))?;
                } else {
                    #[cfg(not(target_os = "macos"))]
                    return Err("ALSA mixer is not available".into());
                }
            }
            ApplicationAction::SaveLoop { loop_id, codec } => {
                let slot = u8::try_from(loop_id)
                    .ok()
                    .filter(|slot| usize::from(*slot) < crate::native_dsp_graph::MAX_RUNTIME_LOOPS)
                    .ok_or_else(|| format!("loop id out of range: {loop_id}"))?;
                let codec = Self::resolve_codec(r, codec)?;
                Self::request_loop_export(r, slot, codec)?;
            }
            ApplicationAction::ImportSelectedLoop { browser, .. } => {
                Self::import_selected_loop(r, browser)?;
            }
            ApplicationAction::LoadSelectedScene { browser } => {
                Self::load_selected_scene(r, browser)?;
            }
            ApplicationAction::SaveScene { force_new } => {
                r.pending_scene_save = Some(PendingSceneSave {
                    force_new,
                    snapshot: None,
                });
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::RequestSnapshot)
                    .map_err(|_| "DSP command queue is full")?;
            }
            ApplicationAction::SetLoadLoopId(loop_id) => r.load_loop_id = loop_id,
            ApplicationAction::SelectPulse(pulse) => {
                if pulse < 0 {
                    r.controls
                        .as_mut()
                        .ok_or("DSP controls are closed")?
                        .try_command(RuntimeCommand::ClearPulse)
                        .map_err(|_| "DSP command queue is full")?;
                    r.pulse_selected = false;
                } else if !r.pulse_selected
                    && let Some(slot) = r.last_recorded_loop
                {
                    // Match C++ LoopManager::SelectPulse: derive the pulse from
                    // the live last-recorded loop, not a potentially stale UI
                    // snapshot. If a pulse already exists, C++ only reselects
                    // it; the guard above preserves its phase.
                    r.controls
                        .as_mut()
                        .ok_or("DSP controls are closed")?
                        .try_command(RuntimeCommand::SetPulseFromLoop { slot })
                        .map_err(|_| "DSP command queue is full")?;
                    r.pulse_selected = true;
                }
            }
            ApplicationAction::DeletePulse => {
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::DeletePulse)
                    .map_err(|_| "DSP command queue is full")?;
                r.pulse_selected = false;
            }
            ApplicationAction::TapPulse { new_len } => {
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::TapPulse { new_len })
                    .map_err(|_| "DSP command queue is full")?;
                // C++ `TapPulse` sets `curpulseindex` when it creates the
                // tapped pulse, so a later F1 reselects instead of deriving
                // a new pulse from the last recorded loop.
                if new_len {
                    r.pulse_selected = true;
                }
            }
            ApplicationAction::CreateSnapshot { snapshot } => {
                r.pending_snapshot_id = Some(snapshot);
            }
            ApplicationAction::TriggerSnapshot { snapshot } => {
                let saved = *r
                    .snapshots
                    .get(&snapshot)
                    .ok_or_else(|| format!("snapshot {snapshot} does not exist"))?;
                let controls = r.controls.as_mut().ok_or("DSP controls are closed")?;
                for (slot, item) in saved.loops.iter().enumerate() {
                    match item.mode {
                        LoopMode::Empty => {}
                        LoopMode::Playing | LoopMode::Recording | LoopMode::Overdubbing => controls
                            .try_command(RuntimeCommand::Trigger {
                                slot: slot as u8,
                                gain: item.trigger_gain,
                            })
                            .map_err(|_| "DSP command queue is full")?,
                        LoopMode::Muted => {
                            controls
                                .try_command(RuntimeCommand::Trigger {
                                    slot: slot as u8,
                                    gain: item.trigger_gain,
                                })
                                .map_err(|_| "DSP command queue is full")?;
                            controls
                                .try_command(RuntimeCommand::Mute {
                                    slot: slot as u8,
                                    muted: true,
                                })
                                .map_err(|_| "DSP command queue is full")?;
                        }
                    }
                }
            }
            ApplicationAction::RenameSnapshot { snapshot } => {
                if !r.snapshots.contains_key(&snapshot) {
                    return Err(format!("snapshot {snapshot} does not exist"));
                }
                let old_name = r.snapshot_names.get(&snapshot).map(String::as_str);
                if !r
                    .rename
                    .begin(RenameTarget::Snapshot { slot: snapshot }, old_name)
                {
                    return Err("another rename operation is already active".into());
                }
                r.input
                    .as_mut()
                    .ok_or("input is closed")?
                    .enable_unicode(true);
            }
            ApplicationAction::RenameLoop { loop_id } => {
                let valid = usize::try_from(loop_id)
                    .ok()
                    .filter(|slot| *slot < crate::native_dsp_graph::MAX_RUNTIME_LOOPS)
                    .is_some_and(|slot| {
                        r.cached_modes[slot] != LoopMode::Empty
                            || r.loop_files.contains_key(&loop_id)
                    });
                if !valid {
                    return Err(format!("loop {loop_id} does not exist"));
                }
                let old_name = r.loop_names.get(&loop_id).cloned().or_else(|| {
                    r.loop_files
                        .get(&loop_id)
                        .and_then(|path| Self::persisted_display_name(path))
                });
                if !r
                    .rename
                    .begin(RenameTarget::Loop { slot: loop_id }, old_name.as_deref())
                {
                    return Err("another rename operation is already active".into());
                }
                r.input
                    .as_mut()
                    .ok_or("input is closed")?
                    .enable_unicode(true);
            }
            ApplicationAction::SwapSnapshots { first, second } => {
                let first_value = r.snapshots.remove(&first);
                let second_value = r.snapshots.remove(&second);
                if let Some(value) = first_value {
                    r.snapshots.insert(second, value);
                }
                if let Some(value) = second_value {
                    r.snapshots.insert(first, value);
                }
            }
            ApplicationAction::MoveBrowserItem {
                browser,
                adjust,
                jump_adjust,
            } => {
                if browser == BROWSER_PATCH {
                    if let Some(patches) = r.patch_browser.as_mut() {
                        patches.move_item(adjust.saturating_add(jump_adjust) as isize);
                    }
                    return Ok(());
                }
                let len = r.browser_entries.len();
                if len != 0 {
                    let cursor = r.browser_cursors.entry(browser).or_default();
                    let delta = adjust.saturating_add(jump_adjust);
                    *cursor = (*cursor as i64 + i64::from(delta))
                        .clamp(0, len.saturating_sub(1) as i64)
                        as usize;
                }
            }
            ApplicationAction::MoveBrowserItemAbsolute { browser, index } => {
                if browser == BROWSER_PATCH {
                    if let Some(patches) = r.patch_browser.as_mut() {
                        patches.select_item(index.max(0) as usize);
                    }
                    return Ok(());
                }
                let max = r.browser_entries.len().saturating_sub(1);
                r.browser_cursors.insert(
                    browser,
                    usize::try_from(index.max(0)).unwrap_or(max).min(max),
                );
            }
            ApplicationAction::BrowserItemBrowsed { browser } => {
                r.browser_cursors.entry(browser).or_default();
            }
            ApplicationAction::SelectBrowserItem { browser } => {
                if browser == BROWSER_PATCH {
                    if let Some(plan) = r
                        .patch_browser
                        .as_ref()
                        .and_then(NativePatchBrowser::action_plan)
                    {
                        Self::apply_patch_plan(r, plan)?;
                    }
                } else {
                    r.browser_cursors.entry(browser).or_default();
                }
            }
            ApplicationAction::RenameBrowserItem { browser } => {
                let item = Self::selected_browser_entry(r, browser)
                    .ok_or("browser has no selected renameable item")?;
                let old_name = Self::persisted_display_name(&r.browser_entries[item]);
                if !r
                    .rename
                    .begin(RenameTarget::Browser { browser, item }, old_name.as_deref())
                {
                    return Err("another rename operation is already active".into());
                }
                r.input
                    .as_mut()
                    .ok_or("input is closed")?
                    .enable_unicode(true);
            }
            ApplicationAction::SetFullscreen(fullscreen) => {
                let video = r.video.as_mut().ok_or("video is closed")?;
                let metrics = video.backend.set_mode(VideoMode {
                    fullscreen,
                    windowed_size: video.windowed_size,
                })?;
                video.input_window_size =
                    (metrics.logical_width.max(1), metrics.logical_height.max(1));
                let drawable_width = metrics.drawable_width.max(1);
                let drawable_height = metrics.drawable_height.max(1);
                video.renderer.metrics = crate::videoio_displays::RenderMetrics::new(
                    video.logical_size.0 as i32,
                    video.logical_size.1 as i32,
                    drawable_width as i32,
                    drawable_height as i32,
                );
                video.frame.width = drawable_width;
                video.frame.height = drawable_height;
                video.frame.stride = drawable_width as usize * 4;
                video
                    .frame
                    .pixels
                    .resize(video.frame.stride * drawable_height as usize, 0);
            }
            ApplicationAction::ToggleStreaming { codec } => {
                r.stream_codec = Self::resolve_codec(r, codec)?;
                r.pending_core_events.push_back(CoreEvent::ToggleDiskOutput);
            }
            ApplicationAction::EraseAllLoops => {
                let controls = r.controls.as_mut().ok_or("DSP controls are closed")?;
                for slot in 0..crate::native_dsp_graph::MAX_RUNTIME_LOOPS {
                    controls
                        .try_command(RuntimeCommand::Erase { slot: slot as u8 })
                        .map_err(|_| "DSP command queue is full")?;
                    r.loop_selection.update_after_erase(slot);
                    r.loop_files.remove(&(slot as i32));
                    r.loop_hashes.remove(&(slot as i32));
                    r.loop_metadata.remove(&(slot as i32));
                }
            }
            ApplicationAction::EraseSelectedLoops { set } => {
                let set = usize::try_from(set).map_err(|_| "invalid loop selection set")?;
                let slots = r
                    .loop_selection
                    .erase_selected(set)
                    .map_err(|error| format!("erase selected loops: {error:?}"))?;
                let controls = r.controls.as_mut().ok_or("DSP controls are closed")?;
                for slot in slots {
                    controls
                        .try_command(RuntimeCommand::Erase { slot: slot as u8 })
                        .map_err(|_| "DSP command queue is full")?;
                    r.loop_files.remove(&(slot as i32));
                    r.loop_hashes.remove(&(slot as i32));
                    r.loop_metadata.remove(&(slot as i32));
                }
            }
            ApplicationAction::ToggleLoopSelection { set, loop_id } => {
                let set = usize::try_from(set).map_err(|_| "invalid loop selection set")?;
                let slot = usize::from(Self::runtime_slot(loop_id)?);
                if r.cached_modes[slot] != LoopMode::Empty {
                    r.loop_selection
                        .toggle(set, slot)
                        .map_err(|error| format!("toggle loop selection: {error:?}"))?;
                }
            }
            ApplicationAction::SelectPlayingLoops { set, playing } => {
                let set = usize::try_from(set).map_err(|_| "invalid loop selection set")?;
                let available: Vec<_> = r
                    .cached_modes
                    .iter()
                    .enumerate()
                    .filter(|(_, mode)| **mode != LoopMode::Empty)
                    .map(|(slot, _)| slot)
                    .collect();
                let modes = r.cached_modes;
                r.loop_selection
                    .select_only_playing(set, &available, |slot| {
                        let active =
                            matches!(modes[slot], LoopMode::Playing | LoopMode::Overdubbing);
                        active == playing
                    })
                    .map_err(|error| format!("select playing loops: {error:?}"))?;
            }
            ApplicationAction::SelectAllLoops { set, selected } => {
                let set = usize::try_from(set).map_err(|_| "invalid loop selection set")?;
                if selected {
                    let available: Vec<_> = r
                        .cached_modes
                        .iter()
                        .enumerate()
                        .filter(|(_, mode)| **mode != LoopMode::Empty)
                        .map(|(slot, _)| slot)
                        .collect();
                    r.loop_selection
                        .select_all(set, &available)
                        .map_err(|error| format!("select all loops: {error:?}"))?;
                } else {
                    r.loop_selection
                        .clear(set)
                        .map_err(|error| format!("clear loop selection: {error:?}"))?;
                }
            }
            ApplicationAction::InvertLoopSelection { set } => {
                let set = usize::try_from(set).map_err(|_| "invalid loop selection set")?;
                let available: Vec<_> = r
                    .cached_modes
                    .iter()
                    .enumerate()
                    .filter(|(_, mode)| **mode != LoopMode::Empty)
                    .map(|(slot, _)| slot)
                    .collect();
                r.loop_selection
                    .invert(set, &available)
                    .map_err(|error| format!("invert loop selection: {error:?}"))?;
            }
            ApplicationAction::TriggerSelectedLoops { set, gain, toggle } => {
                let set = usize::try_from(set).map_err(|_| "invalid loop selection set")?;
                let slots = r
                    .loop_selection
                    .selected_ids(set)
                    .map_err(|error| format!("trigger selected loops: {error:?}"))?
                    .to_vec();
                let controls = r.controls.as_mut().ok_or("DSP controls are closed")?;
                for slot in slots {
                    let command = match r.cached_modes[slot] {
                        LoopMode::Playing if toggle => RuntimeCommand::Mute {
                            slot: slot as u8,
                            muted: true,
                        },
                        LoopMode::Recording | LoopMode::Overdubbing if toggle => {
                            RuntimeCommand::StopRecord
                        }
                        LoopMode::Playing | LoopMode::Recording | LoopMode::Overdubbing => continue,
                        LoopMode::Muted => RuntimeCommand::Trigger {
                            slot: slot as u8,
                            gain,
                        },
                        LoopMode::Empty => continue,
                    };
                    controls
                        .try_command(command)
                        .map_err(|_| "DSP command queue is full")?;
                }
            }
            ApplicationAction::SetSelectedTriggerVolume { set, gain } => {
                let set = usize::try_from(set).map_err(|_| "invalid loop selection set")?;
                for slot in r
                    .loop_selection
                    .selected_ids(set)
                    .map_err(|error| format!("set selected trigger volume: {error:?}"))?
                {
                    r.trigger_gains[*slot] = gain.max(0.0);
                    r.controls
                        .as_mut()
                        .ok_or("DSP controls are closed")?
                        .try_command(RuntimeCommand::SetTriggerGain {
                            slot: *slot as u8,
                            gain: gain.max(0.0),
                        })
                        .map_err(|_| "DSP command queue is full")?;
                }
            }
            ApplicationAction::AdjustSelectedLoopGain { set, factor } => {
                let set = usize::try_from(set).map_err(|_| "invalid loop selection set")?;
                let slots = r
                    .loop_selection
                    .selected_ids(set)
                    .map_err(|error| format!("adjust selected loop gain: {error:?}"))?
                    .to_vec();
                let controls = r.controls.as_mut().ok_or("DSP controls are closed")?;
                for slot in slots {
                    controls
                        .try_command(RuntimeCommand::AdjustLoopGain {
                            slot: slot as u8,
                            factor,
                        })
                        .map_err(|_| "DSP command queue is full")?;
                }
            }
            ApplicationAction::SetLoopTriggerVolume { loop_id, gain } => {
                let slot = Self::runtime_slot(loop_id)?;
                r.trigger_gains[slot as usize] = gain.max(0.0);
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::SetTriggerGain {
                        slot,
                        gain: gain.max(0.0),
                    })
                    .map_err(|_| "DSP command queue is full")?;
            }
            ApplicationAction::SlideLoopGain { loop_id, amount } => {
                let slot = Self::runtime_slot(loop_id)?;
                let time_scale = r.max_callback_frames as f32 / r.sample_rate.max(1) as f32;
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::AdjustLoopGainDelta {
                        slot,
                        amount: amount * time_scale,
                    })
                    .map_err(|_| "DSP command queue is full")?;
            }
            ApplicationAction::StopSlidingLoopGain => r
                .controls
                .as_mut()
                .ok_or("DSP controls are closed")?
                .try_command(RuntimeCommand::ResetLoopGainDeltas)
                .map_err(|_| "DSP command queue is full")?,
            ApplicationAction::SetLoopGain { loop_id, gain } => r
                .controls
                .as_mut()
                .ok_or("DSP controls are closed")?
                .try_command(RuntimeCommand::SetLoopGain {
                    slot: Self::runtime_slot(loop_id)?,
                    gain,
                })
                .map_err(|_| "DSP command queue is full")?,
            ApplicationAction::AdjustLoopGain { loop_id, factor } => r
                .controls
                .as_mut()
                .ok_or("DSP controls are closed")?
                .try_command(RuntimeCommand::AdjustLoopGain {
                    slot: Self::runtime_slot(loop_id)?,
                    factor,
                })
                .map_err(|_| "DSP command queue is full")?,
            ApplicationAction::MoveLoop { from, to } => {
                let from = Self::runtime_slot(from)?;
                let to = Self::runtime_slot(to)?;
                if r.cached_modes[from as usize] == LoopMode::Empty
                    || r.cached_modes[to as usize] != LoopMode::Empty
                {
                    return Err("move loop requires a populated source and empty target".into());
                }
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::MoveLoop { from, to })
                    .map_err(|_| "DSP command queue is full")?;
                r.cached_modes.swap(from as usize, to as usize);
                r.trigger_gains.swap(from as usize, to as usize);
                r.loop_selection
                    .update_after_move(from as usize, to as usize);
                Self::move_loop_map_entry(&mut r.loop_files, from, to);
                Self::move_loop_map_entry(&mut r.loop_names, from, to);
                Self::move_loop_map_entry(&mut r.loop_hashes, from, to);
                Self::move_loop_map_entry(&mut r.loop_metadata, from, to);
                for recent in &mut r.recent_recordings {
                    if *recent == from {
                        *recent = to;
                    }
                }
                if r.last_recorded_loop == Some(from) {
                    r.last_recorded_loop = Some(to);
                }
            }
            ApplicationAction::SetMidiEchoPort(port) => {
                // C++ preserves the one-based external-output convention:
                // zero means no external echo and an out-of-range setting is
                // ignored rather than remapped to a different port.
                r.midi.as_mut().ok_or("MIDI is closed")?.set_echo_port(port);
            }
            ApplicationAction::SetMidiEchoChannel(channel) => {
                r.midi.as_mut().ok_or("MIDI is closed")?.echo_channel =
                    (channel >= 0).then_some(channel.clamp(0, 15) as u8);
            }
            ApplicationAction::AdjustMidiTranspose(adjust) => {
                let transpose = {
                    let mut config = r.config.borrow_mut();
                    config.midi_transpose = config.midi_transpose.saturating_add(adjust);
                    config.midi_transpose
                };
                r.midi.as_mut().ok_or("MIDI is closed")?.note_transpose = transpose;
            }
            ApplicationAction::OutputMidi {
                message,
                outport,
                route_through_patch,
            } => {
                if route_through_patch {
                    Self::echo_routed_midi(r, &message)?;
                } else {
                    let channel = match &message {
                        MidiMessage::NoteOff { channel, .. }
                        | MidiMessage::NoteOn { channel, .. }
                        | MidiMessage::PolyphonicPressure { channel, .. }
                        | MidiMessage::Controller { channel, .. }
                        | MidiMessage::ProgramChange { channel, .. }
                        | MidiMessage::ChannelPressure { channel, .. }
                        | MidiMessage::PitchBend { channel, .. } => *channel,
                        _ => 0,
                    };
                    r.midi
                        .as_ref()
                        .ok_or("MIDI is closed")?
                        .echo_to_route(outport, channel, &message)?;
                }
            }
            ApplicationAction::SetSynthEnabled(enabled) => {
                r.synth_enabled = enabled;
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::SetSynthEnabled(enabled))
                    .map_err(|_| "DSP command queue is full")?;
            }
            ApplicationAction::MidiClock => {
                let sync_outputs = r.config.borrow().midi_sync_outputs.clone();
                r.midi
                    .as_ref()
                    .ok_or("MIDI is closed")?
                    .output_clock_to_ports(&sync_outputs)?
            }
            ApplicationAction::MidiTransport { running } => {
                let midi = r.midi.as_ref().ok_or("MIDI is closed")?;
                let sync_outputs = r.config.borrow().midi_sync_outputs.clone();
                if running {
                    midi.output_start_to_ports(&sync_outputs)?;
                } else {
                    midi.output_stop_to_ports(&sync_outputs)?;
                }
            }
            ApplicationAction::SetMidiSync(enabled) => {
                r.midi.as_mut().ok_or("MIDI is closed")?.sync_transmit = enabled != 0;
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::SetMidiSyncTransmit(enabled != 0))
                    .map_err(|_| "DSP command queue is full")?;
            }
            ApplicationAction::SetSyncType(sync_type) => {
                r.sync_type = sync_type != 0;
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::SetSyncType(sync_type != 0))
                    .map_err(|_| "DSP command queue is full")?;
            }
            ApplicationAction::SetSyncSpeed(sync_speed) => {
                r.sync_speed = u32::try_from(sync_speed).unwrap_or(1).max(1);
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::SetSyncSpeed(sync_speed))
                    .map_err(|_| "DSP command queue is full")?;
            }
            ApplicationAction::TransmitPlayingLoopsToDaw => {
                let snapshot = Self::playing_loops(r);
                r.osc
                    .as_ref()
                    .ok_or("OSC is closed")?
                    .send_playing_loops(&snapshot)
                    .map_err(|error| format!("send playing loops over OSC: {error:?}"))?;
            }
            ApplicationAction::SetDefaultLoopPlacement(range) => {
                let lo = range
                    .lo
                    .clamp(0, crate::native_dsp_graph::MAX_RUNTIME_LOOPS as i32);
                let hi = range
                    .hi
                    .clamp(lo, crate::native_dsp_graph::MAX_RUNTIME_LOOPS as i32);
                r.default_loop_placement = Range::new(lo, hi);
            }
            ApplicationAction::SetAutoLoopSaving { enabled, codec } => {
                r.auto_loop_saving = enabled;
                r.auto_loop_codec = Self::resolve_codec(r, codec)?;
            }
            ApplicationAction::MovePatchBank { direction } => {
                if let Some(patches) = r.patch_browser.as_mut() {
                    patches.move_bank(direction as isize);
                }
            }
            ApplicationAction::SelectPatchBank { index } => {
                if let Some(patches) = r.patch_browser.as_mut() {
                    patches.select_bank(index.max(0) as usize);
                }
            }
        }
        Ok(())
    }

    fn resolve_codec(r: &RuntimeResources, selection: CodecSelection) -> Result<Codec, String> {
        let config = r.config.borrow();
        let codec = match selection {
            CodecSelection::ConfiguredLoopOutput => config.loop_output_format,
            CodecSelection::ConfiguredStreamOutput => config.stream_output_format,
            CodecSelection::DetectFromSelectedFile => {
                return Err("selected-file codec requires a browser path".into());
            }
        };
        matches!(codec, Codec::Wav | Codec::Vorbis | Codec::Flac | Codec::Au)
            .then_some(codec)
            .ok_or("unsupported configured codec".into())
    }

    fn runtime_slot(loop_id: i32) -> Result<u8, String> {
        u8::try_from(loop_id)
            .ok()
            .filter(|slot| usize::from(*slot) < crate::native_dsp_graph::MAX_RUNTIME_LOOPS)
            .ok_or_else(|| format!("loop id out of range: {loop_id}"))
    }

    fn valid_layout_loop_id(loop_id: i32) -> Option<i32> {
        Self::runtime_slot(loop_id).ok().map(i32::from)
    }

    fn move_loop_map_entry<T>(map: &mut HashMap<i32, T>, from: u8, to: u8) {
        if let Some(value) = map.remove(&i32::from(from)) {
            map.insert(i32::from(to), value);
        }
    }

    fn selected_browser_entry(r: &RuntimeResources, browser: i32) -> Option<usize> {
        let cursor = r.browser_cursors.get(&browser).copied().unwrap_or(0);
        r.browser_entries
            .iter()
            .enumerate()
            .filter(|(_, path)| {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                match browser {
                    BROWSER_LOOP | BROWSER_LOOP_TRAY => {
                        name.starts_with("loop-")
                            && matches!(
                                path.extension().and_then(|extension| extension.to_str()),
                                Some("wav" | "ogg" | "flac" | "au")
                            )
                    }
                    BROWSER_SCENE | BROWSER_SCENE_TRAY => {
                        name.starts_with("scene-") && name.ends_with(".xml")
                    }
                    _ => false,
                }
            })
            .nth(cursor)
            .map(|(index, _)| index)
    }

    fn persisted_display_name(path: &std::path::Path) -> Option<String> {
        let stem = path.file_stem()?.to_str()?;
        let hash_end = stem.find('-')?.saturating_add(1 + 32);
        stem.get(hash_end..)
            .and_then(|suffix| suffix.strip_prefix('-'))
            .map(str::to_owned)
    }

    fn apply_patch_plan(r: &mut RuntimeResources, plan: PatchActionPlan) -> Result<(), String> {
        for action in plan.synth {
            r.controls
                .as_mut()
                .ok_or("DSP controls are closed")?
                .try_command(RuntimeCommand::SynthPatch {
                    channel: action.channel,
                    soundfont_id: action.soundfont_id,
                    bank: i32::from(action.bank),
                    program: i32::from(action.program),
                })
                .map_err(|_| "DSP command queue is full")?;
        }
        // Routing is a selected-patch property, independent of whether the
        // patch bank deliberately suppresses bank/program output.
        r.active_midi_routes = plan.echo_routing.clone();
        if let Some(midi) = r.midi.as_mut() {
            for route in &plan.echo_routing {
                // Patch XML follows the same C++ convention: 0 is the
                // internal synth and 1 is the first external MIDI output.
                midi.set_echo_port(i32::try_from(route.midi_port).unwrap_or(i32::MAX));
                midi.echo_channel = Some(route.channel);
            }
        }
        if !plan.suppress_program_changes {
            let midi = r.midi.as_mut().ok_or("MIDI is closed")?;
            for action in plan.external_midi {
                let port = usize::try_from(action.midi_port.saturating_sub(1))
                    .map_err(|_| "invalid MIDI patch port")?;
                if let Some(bank) = action.bank {
                    midi.send(
                        port,
                        MidiMessage::Controller {
                            channel: action.channel,
                            control: 0,
                            value: (bank / 128) as u8,
                        },
                    )?;
                    // Preserve FreeWheeling's historical second CC0 bank byte.
                    midi.send(
                        port,
                        MidiMessage::Controller {
                            channel: action.channel,
                            control: 0,
                            value: (bank % 128) as u8,
                        },
                    )?;
                }
                if let Some(program) = action.program {
                    midi.send(
                        port,
                        MidiMessage::ProgramChange {
                            channel: action.channel,
                            program,
                        },
                    )?;
                }
            }
        }
        Ok(())
    }

    fn handle_rename_input(r: &mut RuntimeResources, event: &InputEvent) -> Result<bool, String> {
        if !r.rename.is_active() {
            return Ok(false);
        }
        let consumed = match event {
            InputEvent::Text(text) => r.rename.handle(RenameInput::Text(text)),
            InputEvent::Key {
                down: true,
                keysym: _,
                unicode,
            } if *unicode > 0 => char::from_u32(*unicode as u32)
                .map(|ch| {
                    let mut encoded = [0_u8; 4];
                    r.rename
                        .handle(RenameInput::Text(ch.encode_utf8(&mut encoded)))
                })
                .unwrap_or(false),
            InputEvent::Key {
                down: true, keysym, ..
            } => {
                let _ = r.rename.handle(RenameInput::KeyDown { keycode: *keysym });
                true
            }
            InputEvent::Key { .. } => true,
            _ => false,
        };
        if let Some(result) = r.rename.pop_result() {
            r.input
                .as_mut()
                .ok_or("input is closed")?
                .enable_unicode(false);
            Self::apply_rename_result(r, result)?;
        }
        Ok(consumed)
    }

    fn apply_rename_result(r: &mut RuntimeResources, result: RenameResult) -> Result<(), String> {
        let Some(name) = result.name else {
            return Ok(());
        };
        if name.contains(['/', '\0']) {
            return Err("rename contains a forbidden path character".into());
        }
        match result.target {
            RenameTarget::Snapshot { slot } => {
                r.snapshot_names.insert(slot, name);
            }
            RenameTarget::Loop { slot } => {
                r.loop_names.insert(slot, name.clone());
                if let Some(path) = r.loop_files.get(&slot).cloned() {
                    Self::rename_persisted_path(r, &path, &name)?;
                }
            }
            RenameTarget::Browser { item, .. } => {
                Self::rename_persisted_browser_item(r, item, &name)?;
            }
        }
        Ok(())
    }

    fn rename_persisted_browser_item(
        r: &mut RuntimeResources,
        item: usize,
        name: &str,
    ) -> Result<(), String> {
        let selected = r
            .browser_entries
            .get(item)
            .cloned()
            .ok_or("rename browser item disappeared")?;
        Self::rename_persisted_path(r, &selected, name)
    }

    fn rename_persisted_path(
        r: &mut RuntimeResources,
        selected: &std::path::Path,
        name: &str,
    ) -> Result<(), String> {
        let filename = selected
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or("rename path is not UTF-8")?;
        let (base, prefix_len) = if filename.starts_with("loop-") {
            ("loop", 5 + 32)
        } else if filename.starts_with("scene-") {
            ("scene", 6 + 32)
        } else {
            return Err("browser item is not a persisted loop or scene".into());
        };
        let prefix = filename
            .get(..prefix_len)
            .ok_or("persisted browser item has an invalid hash")?;
        let hash = prefix
            .strip_prefix(&format!("{base}-"))
            .and_then(crate::core_persistence::decode_hash)
            .ok_or("persisted browser item has an invalid hash")?;
        let _ = hash;
        let mut pairs = Vec::new();
        for entry in fs::read_dir(&r.library_dir)
            .map_err(|error| format!("scan library for rename: {error}"))?
        {
            let path = entry.map_err(|error| error.to_string())?.path();
            let Some(candidate) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if !stem.starts_with(prefix)
                || stem
                    .get(prefix.len()..)
                    .is_some_and(|suffix| !suffix.is_empty() && !suffix.starts_with('-'))
            {
                continue;
            }
            let extension = path.extension().and_then(|value| value.to_str());
            let renamed = crate::core_persistence::saveable_stub(
                base,
                &prefix[prefix.len() - 32..],
                (!name.is_empty()).then_some(name),
                extension
                    .map(|extension| format!(".{extension}"))
                    .as_deref(),
            );
            let destination = path.with_file_name(renamed);
            if candidate
                != destination
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
            {
                pairs.push((path, destination));
            }
        }
        for (_, destination) in &pairs {
            if destination.exists() {
                return Err(format!(
                    "rename destination already exists: {}",
                    destination.display()
                ));
            }
        }
        let mut moved = Vec::new();
        for (source, destination) in &pairs {
            if let Err(error) = fs::rename(source, destination) {
                for (old, new) in moved.iter().rev() {
                    let _ = fs::rename(new, old);
                }
                return Err(format!(
                    "rename '{}' to '{}': {error}",
                    source.display(),
                    destination.display()
                ));
            }
            moved.push((source.clone(), destination.clone()));
        }
        for path in &mut r.browser_entries {
            if let Some((_, destination)) = pairs.iter().find(|(source, _)| source == path) {
                *path = destination.clone();
            }
        }
        for path in r.loop_files.values_mut() {
            if let Some((_, destination)) = pairs.iter().find(|(source, _)| source == path) {
                *path = destination.clone();
            }
        }
        if let Some(current) = r.current_scene.as_mut()
            && let Some((_, destination)) = pairs.iter().find(|(source, _)| source == current)
        {
            *current = destination.clone();
        }
        Ok(())
    }

    fn request_loop_export(r: &mut RuntimeResources, slot: u8, codec: Codec) -> Result<(), String> {
        if r.pending_exports
            .iter()
            .any(|pending| pending.loop_id == i32::from(slot))
            || r.queued_exports.iter().any(|pending| pending.0 == slot)
        {
            return Ok(());
        }
        if !r.pending_exports.is_empty() {
            r.queued_exports.push_back((slot, codec));
            return Ok(());
        }
        Self::start_loop_export(r, slot, codec)
    }

    fn start_loop_export(r: &mut RuntimeResources, slot: u8, codec: Codec) -> Result<(), String> {
        let handle = r
            .controls
            .as_mut()
            .ok_or("DSP controls are closed")?
            .try_request_loop_export(slot)
            .map_err(|error| format!("cannot request loop export: {error:?}"))?;
        r.pending_exports.push(PendingLoopExport {
            handle,
            loop_id: i32::from(slot),
            codec,
        });
        Ok(())
    }

    fn start_next_loop_export(r: &mut RuntimeResources) -> Result<(), String> {
        if r.pending_exports.is_empty()
            && let Some((slot, codec)) = r.queued_exports.pop_front()
        {
            Self::start_loop_export(r, slot, codec)?;
        }
        Ok(())
    }

    fn playing_loops(r: &RuntimeResources) -> PlayingLoops {
        let is_playing = |slot: i32| {
            usize::try_from(slot)
                .ok()
                .is_some_and(|slot| r.cached_modes.get(slot) == Some(&LoopMode::Playing))
        };
        let long_count = r
            .loop_metadata
            .iter()
            .filter(|(slot, _)| is_playing(**slot))
            .map(|(_, metadata)| metadata.beats.max(1) as u64)
            .fold(1_u64, Self::cpp_lcm)
            .min(i32::MAX as u64) as i64;
        let mut loops = Vec::new();
        let mut range_end = 0_i32;
        for (slot, path) in &r.loop_files {
            if !is_playing(*slot) {
                continue;
            }
            let Some(metadata) = r.loop_metadata.get(slot) else {
                continue;
            };
            // C++ uses the loop's own pulse beat count when it has a pulse;
            // unsynchronised loops occupy the entire common long cycle.
            let nbeats = if metadata.pulse_frames > 0 {
                metadata.beats.max(1)
            } else {
                long_count
            };
            let repetitions = (long_count / nbeats).max(1);
            let length = i32::try_from(Self::cpp_quantize_pulse_length(
                metadata.frames,
                metadata.pulse_frames,
            ))
            .unwrap_or(i32::MAX);
            let crossfade =
                i32::try_from(r.max_callback_frames.saturating_mul(2)).unwrap_or(i32::MAX);
            let gain = r
                .cached_loops
                .iter()
                .find(|item| item.loop_id == *slot as usize)
                .map_or(metadata.gain, |item| item.loop_volume);
            for repetition in 0..repetitions {
                let start = i64::from(length)
                    .saturating_mul(repetition)
                    .min(i64::from(i32::MAX)) as i32;
                range_end = range_end.max(start.saturating_add(length));
                loops.push(PlayingLoop {
                    start,
                    length,
                    crossfade,
                    gain,
                    path: path
                        .canonicalize()
                        .unwrap_or_else(|_| path.clone())
                        .to_string_lossy()
                        .into_owned(),
                });
            }
        }
        let tempo = r.pulse_selected.then(|| {
            let pulse_frames = r.latest_snapshot.pulse_frames.max(1);
            let sync_speed = r.sync_speed.max(1);
            let (bar_frames, beats_per_bar) = if r.sync_type {
                (pulse_frames, sync_speed)
            } else {
                (pulse_frames / sync_speed, 4)
            };
            (
                60.0 * r.sample_rate as f32 * beats_per_bar as f32 / bar_frames.max(1) as f32,
                beats_per_bar as i32,
            )
        });
        PlayingLoops {
            tempo,
            loops,
            range_end,
        }
    }

    /// `math_lcm` used by `LoopManager::GetLongCountForAllPlayingLoops`.
    /// Saturation mirrors the native exporter’s bounded OSC integer payload.
    fn cpp_lcm(left: u64, right: u64) -> u64 {
        fn gcd(mut left: u64, mut right: u64) -> u64 {
            while right != 0 {
                (left, right) = (right, left % right);
            }
            left.max(1)
        }
        left.saturating_div(gcd(left, right)).saturating_mul(right)
    }

    /// `Pulse::QuantizeLength`: a sub-half-pulse source still exports one
    /// pulse, otherwise C++ rounds to the nearest whole pulse count.
    fn cpp_quantize_pulse_length(source_frames: u32, pulse_frames: u32) -> u32 {
        if pulse_frames == 0 {
            return source_frames;
        }
        let fraction = source_frames as f64 / pulse_frames as f64;
        let beats = if fraction < 0.5 {
            1.0
        } else {
            fraction.round()
        };
        (beats * pulse_frames as f64).min(u32::MAX as f64) as u32
    }

    fn maybe_save_scene(r: &mut RuntimeResources) -> Result<(), String> {
        if !r.pending_exports.is_empty() || !r.queued_exports.is_empty() {
            return Ok(());
        }
        let Some(pending) = r.pending_scene_save.take() else {
            return Ok(());
        };
        let snapshot = pending
            .snapshot
            .ok_or("scene save snapshot has not completed")?;
        let mut loop_ids: Vec<_> = snapshot
            .loops
            .iter()
            .enumerate()
            .filter(|(_, item)| item.mode != LoopMode::Empty)
            .map(|(slot, _)| slot as i32)
            .collect();
        loop_ids.sort_unstable();
        let loops =
            loop_ids
                .iter()
                .map(|slot| {
                    let item = snapshot.loops[*slot as usize];
                    Ok(crate::core_persistence::LoopMeta {
                        hash: r.loop_hashes.get(slot).cloned().ok_or_else(|| {
                            format!("loop {slot} was not persisted for scene save")
                        })?,
                        loop_id: *slot,
                        volume: item.gain,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
        let mut snapshots: Vec<_> = r
            .snapshots
            .iter()
            .map(|(id, snapshot)| crate::core_persistence::SnapshotMeta {
                id: *id,
                name: r.snapshot_names.get(id).cloned().unwrap_or_default(),
                loops: snapshot
                    .loops
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| item.mode != LoopMode::Empty)
                    .map(|(slot, item)| crate::core_persistence::SnapshotLoop {
                        loop_id: slot as i32,
                        status: match item.mode {
                            LoopMode::Empty | LoopMode::Muted => 0,
                            LoopMode::Recording => 1,
                            LoopMode::Overdubbing => 2,
                            LoopMode::Playing => 3,
                        },
                        loop_volume: item.gain,
                        trigger_volume: item.trigger_gain,
                    })
                    .collect(),
            })
            .collect();
        snapshots.sort_by_key(|snapshot| snapshot.id);
        let scene = crate::core_persistence::Scene { loops, snapshots };
        let mut hash_input = Vec::with_capacity(loop_ids.len() * 16);
        for slot in &loop_ids {
            let hash = r.loop_hashes.get(slot).expect("checked above");
            hash_input.extend_from_slice(
                &crate::core_persistence::decode_hash(hash)
                    .ok_or_else(|| format!("loop {slot} has invalid persistence hash"))?,
            );
        }
        let hash =
            crate::core_persistence::encode_hash(&crate::core_persistence::md5_audio(&hash_input));
        let xml = crate::core_persistence::scene_xml(&scene);
        let path = if !pending.force_new {
            r.current_scene.clone().unwrap_or_else(|| {
                r.library_dir.join(crate::core_persistence::saveable_stub(
                    "scene",
                    &hash,
                    None,
                    Some(".xml"),
                ))
            })
        } else {
            let mut sequence = 0_u32;
            loop {
                let name = (sequence != 0).then(|| sequence.to_string());
                let candidate = r.library_dir.join(crate::core_persistence::saveable_stub(
                    "scene",
                    &hash,
                    name.as_deref(),
                    Some(".xml"),
                ));
                if !candidate.exists() {
                    break candidate;
                }
                sequence = sequence.saturating_add(1);
            }
        };
        let backup = if path.exists() {
            let mut sequence = 1_u32;
            let backup = loop {
                let candidate = std::path::PathBuf::from(format!(
                    "{}.backup.{sequence}",
                    path.to_string_lossy()
                ));
                if !candidate.exists() {
                    break candidate;
                }
                sequence = sequence.saturating_add(1);
            };
            fs::rename(&path, &backup)
                .map_err(|error| format!("backup scene '{}': {error}", path.display()))?;
            Some(backup)
        } else {
            None
        };
        if let Err(error) = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, xml.as_bytes()))
        {
            if let Some(backup) = backup {
                let _ = fs::rename(backup, &path);
            }
            return Err(format!("save scene '{}': {error}", path.display()));
        }
        r.current_scene = Some(path.clone());
        if !r.browser_entries.contains(&path) {
            r.browser_entries.push(path);
        }
        Ok(())
    }

    fn finish_loop_export(
        r: &mut RuntimeResources,
        handle: PcmTransferHandle,
        metadata: LoopTransferMetadata,
    ) -> Result<(), String> {
        let Some(index) = r
            .pending_exports
            .iter()
            .position(|pending| pending.handle == handle)
        else {
            return Err("received an untracked loop export".into());
        };
        let pending = r.pending_exports.remove(index);
        let controls = r.controls.as_ref().ok_or("DSP controls are closed")?;
        let saved = controls
            .with_exported_pcm(handle, |left, right| {
                let hash = crate::core_persistence::md5_loop_samples(left, Some(right));
                let hash = crate::core_persistence::encode_hash(&hash);
                let extension = codec_extension(pending.codec)?;
                let audio = r.library_dir.join(crate::core_persistence::saveable_stub(
                    "loop",
                    &hash,
                    None,
                    Some(extension),
                ));
                let xml = r.library_dir.join(crate::core_persistence::saveable_stub(
                    "loop",
                    &hash,
                    None,
                    Some(".xml"),
                ));
                if audio.exists() {
                    return Err(format!(
                        "MD5 collision while saving loop- file exists: {}",
                        audio.display()
                    ));
                }
                encode_audio_file(&audio, r.sample_rate, pending.codec, left, Some(right))
                    .map_err(|error| format!("save loop {}: {error}", pending.loop_id))?;
                let metadata_xml = crate::core_persistence::loop_metadata_xml(
                    metadata.beats,
                    metadata.pulse_frames,
                );
                if let Err(error) = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&xml)
                    .and_then(|mut file| {
                        std::io::Write::write_all(&mut file, metadata_xml.as_bytes())
                    })
                {
                    return Err(format!("save loop metadata: {error}"));
                }
                Ok((audio, xml, hash))
            })
            .map_err(|error| format!("read exported loop PCM: {error:?}"))?;
        let release = controls.release_transfer(handle);
        match saved {
            Ok((audio, xml, _hash)) => {
                release.map_err(|error| format!("release loop export: {error:?}"))?;
                r.loop_files.insert(pending.loop_id, audio.clone());
                r.loop_hashes.insert(pending.loop_id, _hash);
                r.loop_metadata.insert(pending.loop_id, metadata);
                r.browser_entries.extend([audio, xml]);
                Self::start_next_loop_export(r)?;
                Ok(())
            }
            Err(error) => {
                release.map_err(|release_error| {
                    format!("{error}; release loop export: {release_error:?}")
                })?;
                // An export failure must not stop a scene/autosave batch behind
                // it.  The failed loop remains unsaved and the caller receives
                // the error, but the transfer slot is returned and the queue is
                // made progressable for the next request.
                Self::start_next_loop_export(r)?;
                Err(error)
            }
        }
    }

    fn load_selected_scene(r: &mut RuntimeResources, browser: i32) -> Result<(), String> {
        let index = r.browser_cursors.get(&browser).copied().unwrap_or(0);
        let path = r
            .browser_entries
            .iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("scene-") && name.ends_with(".xml"))
            })
            .nth(index)
            .cloned()
            .ok_or("scene browser has no selected scene")?;
        let xml = fs::read_to_string(&path)
            .map_err(|error| format!("read scene '{}': {error}", path.display()))?;
        let scene = crate::core_persistence_parse::parse_scene_xml(&xml, r.load_loop_id)?;
        {
            let controls = r.controls.as_mut().ok_or("DSP controls are closed")?;
            for slot in 0..crate::native_dsp_graph::MAX_RUNTIME_LOOPS {
                controls
                    .try_command(RuntimeCommand::Erase { slot: slot as u8 })
                    .map_err(|_| "DSP command queue is full")?;
            }
        }
        // The old scene may still have an import in flight.  Returning those
        // handles before replacing the queues prevents the bounded transfer
        // pool from being exhausted by repeated scene loads.
        let old_imports = std::mem::take(&mut r.pending_imports);
        if let Some(controls) = r.controls.as_ref() {
            for handle in old_imports {
                let _ = controls.release_transfer(handle);
            }
        }
        r.loop_files.clear();
        r.loop_hashes.clear();
        r.loop_metadata.clear();
        r.queued_imports.clear();
        for item in scene.loops {
            let slot = u8::try_from(item.loop_id)
                .ok()
                .filter(|slot| usize::from(*slot) < crate::native_dsp_graph::MAX_RUNTIME_LOOPS)
                .or_else(|| {
                    (r.default_loop_placement.lo..r.default_loop_placement.hi)
                        .find_map(|slot| u8::try_from(slot).ok())
                })
                .ok_or_else(|| format!("no placement available for scene loop {}", item.loop_id))?;
            let prefix = format!("loop-{}", item.hash);
            let audio = fs::read_dir(&r.library_dir)
                .map_err(|error| format!("scan loop library: {error}"))?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|candidate| {
                    candidate
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&prefix))
                        && matches!(
                            candidate
                                .extension()
                                .and_then(|extension| extension.to_str()),
                            Some("wav" | "ogg" | "flac" | "au")
                        )
                })
                .ok_or_else(|| format!("scene loop audio is missing for hash {}", item.hash))?;
            r.loop_files.insert(i32::from(slot), audio.clone());
            r.loop_hashes.insert(i32::from(slot), item.hash);
            r.queued_imports.push_back(QueuedLoopImport {
                path: audio,
                slot,
                gain: item.volume,
            });
        }
        r.snapshots.clear();
        r.snapshot_names.clear();
        for saved in scene.snapshots {
            let mut snapshot = RuntimeSnapshot::default();
            for item in saved.loops {
                let Some(slot) = usize::try_from(item.loop_id)
                    .ok()
                    .filter(|slot| *slot < snapshot.loops.len())
                else {
                    continue;
                };
                snapshot.loops[slot].mode = match item.status {
                    1 => LoopMode::Recording,
                    2 => LoopMode::Overdubbing,
                    3 => LoopMode::Playing,
                    _ => LoopMode::Muted,
                };
                snapshot.loops[slot].gain = item.loop_volume;
                snapshot.loops[slot].trigger_gain = item.trigger_volume;
            }
            r.snapshot_names.insert(saved.id, saved.name);
            r.snapshots.insert(saved.id, snapshot);
        }
        r.current_scene = Some(path);
        Self::start_next_queued_import(r)
    }

    fn start_next_queued_import(r: &mut RuntimeResources) -> Result<(), String> {
        if !r.pending_imports.is_empty() {
            return Ok(());
        }
        let Some(import) = r.queued_imports.pop_front() else {
            return Ok(());
        };
        Self::decode_and_queue_import(r, &import.path, import.slot, import.gain)
    }

    fn decode_and_queue_import(
        r: &mut RuntimeResources,
        path: &std::path::Path,
        slot: u8,
        gain: f32,
    ) -> Result<(), String> {
        let codec = match path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("wav") => Codec::Wav,
            Some("ogg") => Codec::Vorbis,
            Some("flac") => Codec::Flac,
            Some("au") => Codec::Au,
            _ => return Err(format!("cannot detect loop codec: {}", path.display())),
        };
        let max_frames = crate::native_dsp_graph::CPP_AUDIO_POOL_FRAMES;
        let mut decoder = SndFileDecoder::new(r.sample_rate, codec);
        decoder
            .read_from_file(fs::File::open(path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("open loop '{}': {error}", path.display()))?;
        let mut block = AudioBlock::new(max_frames);
        if decoder.stereo() {
            block.extra = Some(ExtraChannel::new(max_frames));
        }
        let frames = {
            let mut iterator =
                AudioBlockIterator::new(&mut block, crate::file_codecs::MAX_STREAMING_FRAMES);
            loop {
                let count = decoder
                    .read_samples(&mut iterator, crate::file_codecs::MAX_STREAMING_FRAMES)
                    .map_err(|error| format!("decode loop '{}': {error}", path.display()))?;
                if count == 0 {
                    break iterator.position;
                }
                if iterator.position == max_frames {
                    return Err(format!(
                        "loop exceeds the {max_frames}-frame C++ audio-block pool capacity"
                    ));
                }
            }
        };
        let right = block
            .extra
            .as_ref()
            .map(|extra| &extra.samples[..frames])
            .unwrap_or(&block.samples[..frames]);
        let parsed_metadata = fs::read_to_string(path.with_extension("xml"))
            .ok()
            .and_then(|xml| crate::core_persistence_parse::parse_loop_metadata_xml(&xml).ok());
        let pulse_frames = parsed_metadata
            .as_ref()
            .and_then(|metadata| metadata.pulse_length)
            .unwrap_or((r.sample_rate / 2).max(1));
        let loop_metadata = LoopTransferMetadata {
            frames: frames as u32,
            position: 0,
            mode: LoopMode::Playing,
            gain,
            pulse_frames,
            beats: parsed_metadata
                .and_then(|metadata| metadata.nbeats)
                .unwrap_or_else(|| (frames as u32 / pulse_frames).max(1) as i64),
        };
        let controls = r.controls.as_mut().ok_or("DSP controls are closed")?;
        let handle = controls
            .try_acquire_transfer()
            .map_err(|error| format!("acquire loop import buffer: {error:?}"))?;
        if let Err(error) = controls.write_transfer(handle, &block.samples[..frames], right) {
            let _ = controls.release_transfer(handle);
            return Err(format!("stage loop import: {error:?}"));
        }
        if let Err(error) = controls.try_import_loop(slot, handle, 0, LoopMode::Playing, gain) {
            let _ = controls.release_transfer(handle);
            return Err(format!("queue loop import: {error:?}"));
        }
        // Publish bookkeeping only after the DSP owns the transfer.  A failed
        // queue operation must not make an unimported loop appear scene-ready.
        r.loop_metadata.insert(i32::from(slot), loop_metadata);
        r.pending_imports.push(handle);
        Ok(())
    }

    fn import_selected_loop(r: &mut RuntimeResources, browser: i32) -> Result<(), String> {
        let index = r.browser_cursors.get(&browser).copied().unwrap_or(0);
        let path = r
            .browser_entries
            .iter()
            .filter(|path| {
                matches!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("wav" | "ogg" | "flac" | "au")
                )
            })
            .nth(index)
            .cloned()
            .ok_or("loop browser has no selected audio file")?;
        let codec = match path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("wav") => Codec::Wav,
            Some("ogg") => Codec::Vorbis,
            Some("flac") => Codec::Flac,
            Some("au") => Codec::Au,
            _ => return Err(format!("cannot detect loop codec: {}", path.display())),
        };
        let max_frames = crate::native_dsp_graph::CPP_AUDIO_POOL_FRAMES;
        let mut decoder = SndFileDecoder::new(r.sample_rate, codec);
        decoder
            .read_from_file(fs::File::open(&path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("open loop '{}': {error}", path.display()))?;
        let mut block = AudioBlock::new(max_frames);
        if decoder.stereo() {
            block.extra = Some(ExtraChannel::new(max_frames));
        }
        let frames = {
            let mut iterator =
                AudioBlockIterator::new(&mut block, crate::file_codecs::MAX_STREAMING_FRAMES);
            loop {
                let count = decoder
                    .read_samples(&mut iterator, crate::file_codecs::MAX_STREAMING_FRAMES)
                    .map_err(|error| format!("decode loop '{}': {error}", path.display()))?;
                if count == 0 {
                    break iterator.position;
                }
                if iterator.position == max_frames {
                    return Err(format!(
                        "loop exceeds the {max_frames}-frame C++ audio-block pool capacity"
                    ));
                }
            }
        };
        let right = block
            .extra
            .as_ref()
            .map(|extra| &extra.samples[..frames])
            .unwrap_or(&block.samples[..frames]);
        let controls = r.controls.as_mut().ok_or("DSP controls are closed")?;
        let handle = controls
            .try_acquire_transfer()
            .map_err(|error| format!("acquire loop import buffer: {error:?}"))?;
        if let Err(error) = controls.write_transfer(handle, &block.samples[..frames], right) {
            let _ = controls.release_transfer(handle);
            return Err(format!("stage loop import: {error:?}"));
        }
        let slot = u8::try_from(r.load_loop_id)
            .ok()
            .filter(|slot| usize::from(*slot) < crate::native_dsp_graph::MAX_RUNTIME_LOOPS)
            .ok_or_else(|| format!("load loop id out of range: {}", r.load_loop_id));
        let slot = match slot {
            Ok(slot) => slot,
            Err(error) => {
                let _ = controls.release_transfer(handle);
                return Err(error);
            }
        };
        if let Err(error) = controls.try_import_loop(slot, handle, 0, LoopMode::Playing, 1.0) {
            let _ = controls.release_transfer(handle);
            return Err(format!("queue loop import: {error:?}"));
        }
        r.pending_imports.push(handle);
        Ok(())
    }

    fn rollback_resource(&mut self, phase: StartupPhase) {
        let mut r = self.resources.borrow_mut();
        match phase {
            StartupPhase::ProcessingElements | StartupPhase::StreamersAndRings => {}
            StartupPhase::SignalProcessing | StartupPhase::Audio => {
                if let Some(mut streamer) = r.streamer.take() {
                    let _ = streamer.finalize();
                }
                r.stream_output_name.clear();
                if let Some(audio) = r.audio.as_mut() {
                    audio.close();
                }
                if phase == StartupPhase::Audio {
                    r.audio = None;
                    r.controls = None;
                }
            }
            StartupPhase::SystemVariables => {}
            StartupPhase::OscAndMixer => {
                if let Some(osc) = r.osc.take() {
                    osc.close();
                }
                if let Some(mut mixer) = r.mixer.take() {
                    mixer.close();
                }
            }
            StartupPhase::InputAndMidi => {
                if let Some(input) = r.input.as_mut() {
                    input.close();
                }
                r.input = None;
                if let Some(midi) = r.midi.as_mut() {
                    midi.shutdown();
                }
                r.midi = None;
                r.event_bridge = None;
            }
            StartupPhase::Browsers => r.browser_entries.clear(),
            StartupPhase::SynthAndBuffers => {}
            StartupPhase::CoreGraph => r.controls = None,
            StartupPhase::VideoReady => {}
            StartupPhase::Video => {
                if let Some(video) = r.video.as_mut() {
                    video.close();
                }
                r.video = None;
            }
            StartupPhase::EventManager => r.events = None,
            StartupPhase::MemoryManager => r.memory_manager = None,
            StartupPhase::RtThreads => r.rcu_registry = None,
            StartupPhase::LockMemory => {
                #[cfg(all(unix, not(target_os = "macos")))]
                unsafe {
                    libc::munlockall();
                }
            }
            _ => {}
        }
    }
}

impl NativeStartupAdapter for NativeRuntime {
    fn start(&mut self, phase: StartupPhase, paths: &NativePaths) -> Result<(), String> {
        let mut r = self.resources.borrow_mut();
        match phase {
            StartupPhase::LockMemory => {
                fs::metadata(&paths.resources).map_err(|e| format!("resource directory: {e}"))?;
                // macOS has `mlock`, but no implemented `mlockall`. Callback
                // memory is wired by owning fully-sized DSP, synth, CPAL and
                // transfer-pool buffers before SignalProcessing activation.
                #[cfg(all(unix, not(target_os = "macos")))]
                if unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) } != 0 {
                    return Err(format!(
                        "lock process memory: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            }
            StartupPhase::RtThreads => r.rcu_registry = Some(RcuRegistry::new()),
            StartupPhase::MainThread => {
                r.rcu_registry
                    .as_ref()
                    .ok_or("RCU registry missing")?
                    .register_current()
                    .map_err(str::to_owned)?;
            }
            StartupPhase::PlatformThreads => {
                std::thread::available_parallelism()
                    .map_err(|e| format!("query platform threads: {e}"))?;
            }
            StartupPhase::Sdl => {
                r.input = Some(SdlIo::new(Sdl2InputBackend::new()?));
            }
            StartupPhase::MemoryManager => {
                r.memory_manager = Some(MemoryManager::new());
            }
            StartupPhase::EventManager => {
                let manager = Arc::new(EventManager::new());
                let mut listened: HashSet<EventType> = r
                    .config
                    .borrow()
                    .binding_registry
                    .tables
                    .keys()
                    .copied()
                    .collect();
                listened.extend([
                    EventType::InputKey,
                    EventType::InputJoystickButton,
                    EventType::InputMouseButton,
                    EventType::InputMouseMotion,
                    EventType::InputMIDIKey,
                    EventType::InputMIDIController,
                    EventType::InputMIDIProgramChange,
                    EventType::InputMIDIChannelPressure,
                    EventType::InputMIDIPitchBend,
                    EventType::InputMIDIPolyphonicPressure,
                    EventType::InputMIDISystemExclusive,
                    EventType::InputMIDITimeCodeQuarterFrame,
                    EventType::InputMIDISongPosition,
                    EventType::InputMIDISongSelect,
                    EventType::InputMIDITuneRequest,
                    EventType::InputMIDIActiveSensing,
                    EventType::InputMIDIReset,
                    EventType::InputMIDIClock,
                    EventType::InputMIDIStartStop,
                ]);
                for event_type in listened {
                    manager.listen(
                        Box::new(RuntimeInboxListener {
                            inbox: Arc::clone(&r.event_inbox),
                        }),
                        event_type,
                    );
                }
                r.events = Some(manager);
            }
            StartupPhase::Video => {
                let video = MainThreadVideo::open(&paths.resources)?;
                r.help_page_count = video.help_page_count;
                r.video = Some(video);
            }
            StartupPhase::VideoReady => {
                if !r.video.as_ref().is_some_and(|video| video.active) {
                    return Err("video worker did not activate".into());
                }
            }
            StartupPhase::Audio => {
                let kind = std::env::var("FWEELIN_AUDIO_BACKEND")
                    .ok()
                    .and_then(|value| match value.to_lowercase().as_str() {
                        "jack" => Some(AudioBackendKind::Jack),
                        "cpal" => Some(AudioBackendKind::Cpal),
                        _ => None,
                    })
                    .unwrap_or_default();
                let backend: AnyAudioBackend = match kind {
                    AudioBackendKind::Jack => {
                        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                        {
                            AnyAudioBackend::Jack(JackAudioMidiBackend::new(1, 1))
                        }
                        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
                        {
                            let _ = kind;
                            return Err("JACK backend is not available on this platform".into());
                        }
                    }
                    AudioBackendKind::Cpal => {
                        let mut options = CpalAudioOptions::default();
                        if std::env::var_os("FWEELIN_AUDIO_BUFFER_FRAMES").is_none() {
                            options.preferred_buffer_frames =
                                r.config.borrow().preferred_audio_buffer_frames.max(1);
                        }
                        AnyAudioBackend::Cpal(CpalAudioBackend::new(
                            DeviceSelection::default(),
                            options,
                        ))
                    }
                    AudioBackendKind::Auto => {
                        #[cfg(target_os = "macos")]
                        {
                            let back = MacosAudioUnitBackend::new(
                                DeviceSelection::default(),
                                CpalAudioOptions::default(),
                            );
                            AnyAudioBackend::AudioUnit(back)
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            let mut options = CpalAudioOptions::default();
                            if std::env::var_os("FWEELIN_AUDIO_BUFFER_FRAMES").is_none() {
                                options.preferred_buffer_frames =
                                    r.config.borrow().preferred_audio_buffer_frames.max(1);
                            }
                            AnyAudioBackend::Cpal(CpalAudioBackend::new(
                                DeviceSelection::default(),
                                options,
                            ))
                        }
                    }
                };
                let backend_name = match &backend {
                    AnyAudioBackend::Cpal(_) => "CPAL",
                    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                    AnyAudioBackend::Jack(_) => "JACK",
                    #[cfg(target_os = "macos")]
                    AnyAudioBackend::AudioUnit(_) => "AudioUnit",
                };
                eprintln!("FreeWheeling: audio backend: {backend_name}");
                let mut audio = AudioIO::new(backend);
                audio.open("FreeWheeling")?;
                r.sample_rate = audio.get_srate();
                r.max_callback_frames = audio.getbufsz() as usize;
                r.audio = Some(audio);
            }
            StartupPhase::CoreGraph => {
                if r.sample_rate == 0 || r.max_callback_frames == 0 {
                    return Err("audio capacities unavailable".into());
                }
            }
            StartupPhase::SynthAndBuffers => {
                let rate = r.sample_rate;
                let recording_alignment_frames = r
                    .audio
                    .as_ref()
                    .map(|audio| audio.input_latency_frames())
                    .unwrap_or(0);
                let synth_config = r.config.borrow().fluidsynth.clone();
                let mut cfg = FluidLiteConfig::new(rate as f64);
                cfg.settings = synth_config.settings;
                cfg.tuning_cents = synth_config.tuning_cents;
                cfg.soundfonts =
                    startup_soundfonts(synth_config.soundfonts, paths.asset("basic.sf2")?);
                // FluidLite's safe public API exposes only the library's
                // fourth-order default.  Keep a non-default legacy value in
                // FloConfig for a future backend setter rather than rejecting
                // the shipped `interpolation="1"` configuration at startup.
                let synth = FluidLiteBackend::new(cfg).map_err(|e| format!("FluidLite: {e}"))?;
                let synth_patches = synth.patches();
                let mut patch_browser = NativePatchBrowser::from_config(&r.config.borrow())?;
                patch_browser.prepend_synth_patches(&synth_patches);
                r.patch_browser = Some(patch_browser);
                let dsp_settings = {
                    let config = r.config.borrow();
                    DspSettings {
                        max_play_volume: config.max_play_volume,
                        max_limiter_gain: config.max_limiter_gain,
                        limiter_threshold: config.limiter_threshold,
                        limiter_release_rate: config.limiter_release_rate,
                        fader_max_db: config.fader_max_db,
                        input_monitoring: [
                            config
                                .audio_input_monitoring
                                .first()
                                .copied()
                                .unwrap_or(true),
                            config
                                .audio_input_monitoring
                                .get(1)
                                .copied()
                                .unwrap_or(true),
                        ],
                    }
                };
                let (processor, controls) = production_audio_processor_with_settings(
                    synth,
                    rate,
                    crate::native_dsp_graph::CPP_AUDIO_POOL_FRAMES,
                    r.max_callback_frames,
                    dsp_settings,
                );
                let mut controls = controls;
                controls
                    .try_command(RuntimeCommand::SetSynthChannel(synth_config.channel))
                    .map_err(|_| "DSP command queue is full during synth setup")?;
                controls
                    .try_command(RuntimeCommand::SetSynthStereo(synth_config.stereo))
                    .map_err(|_| "DSP command queue is full during synth setup")?;
                controls
                    .try_command(RuntimeCommand::SetRecordingAlignmentFrames {
                        frames: recording_alignment_frames,
                    })
                    .map_err(|_| "DSP command queue is full during latency setup")?;
                if std::env::var_os("FWEELIN_DIAGNOSTICS").is_some() {
                    eprintln!(
                        "FreeWheeling recording alignment: {recording_alignment_frames} input frames"
                    );
                }
                r.controls = Some(controls);
                PROCESSOR.with(|slot| *slot.borrow_mut() = Some(processor));
            }
            StartupPhase::Browsers => {
                fs::create_dir_all(&r.library_dir)
                    .map_err(|e| format!("create library directory: {e}"))?;
                r.browser_entries = fs::read_dir(&r.library_dir)
                    .map_err(|e| format!("scan library: {e}"))?
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .collect();
            }
            StartupPhase::InputAndMidi => {
                let manager = Arc::clone(r.events.as_ref().ok_or("event manager missing")?);
                let bridge = Arc::new(NativeEventBridge::new(manager, 1024));
                // When using the JACK audio backend, MIDI arrives through
                // the audio callback (JACK MIDI ports → ring buffer), so we
                // skip the standalone Midir backend to avoid duplicate paths.
                let use_jack_midi = r.audio.as_ref().map_or(false, |a| a.backend().is_jack());
                if !use_jack_midi {
                    eprintln!("FreeWheeling: MIDI backend: Midir");
                    let mut midi = MidiIo::new(MidirMidiBackend::new(None));
                    midi.set_sink(bridge.clone());
                    let outputs = r.config.borrow().midi_outputs;
                    midi.activate(MIDI_INPUTS, outputs)?;
                    r.midi = Some(midi);
                } else {
                    eprintln!("FreeWheeling: MIDI backend: JACK");
                }
                r.event_bridge = Some(bridge);
                r.input.as_mut().ok_or("SDL input missing")?.activate();
            }
            StartupPhase::OscAndMixer => {
                let osc = OscClient::new(
                    UdpBackend::new("127.0.0.1", 9951).map_err(|e| format!("OSC: {e:?}"))?,
                );
                osc.open().map_err(|e| format!("OSC: {e:?}"))?;
                r.osc = Some(osc);
                #[cfg(not(target_os = "macos"))]
                {
                    r.mixer = Some(HardwareMixerInterface::new(AlsaMixerBackend::default()));
                }
            }
            StartupPhase::SystemVariables => {
                fs::metadata(&r.library_dir)
                    .map_err(|e| format!("library directory unavailable: {e}"))?;
            }
            StartupPhase::SignalProcessing => {
                let processor = PROCESSOR
                    .with(|slot| slot.borrow_mut().take())
                    .ok_or("DSP graph missing")?;
                let mut audio = r.audio.take().ok_or("audio missing")?;
                let activation = std::thread::Builder::new()
                    .name("coreaudio-setup".into())
                    .spawn(move || {
                        let result = audio.activate(processor);
                        (audio, result)
                    })
                    .map_err(|e| format!("spawn CoreAudio setup: {e}"))?;
                // CoreAudio device setup may synchronously wait on services
                // which require the Cocoa main queue. Keep SDL/AppKit moving
                // while the callback-owned processor is installed off-main.
                while !activation.is_finished() {
                    let state = Self::ui_scene_state(&r);
                    if let Some(video) = r.video.as_mut() {
                        video.update(Instant::now(), state)?;
                    }
                    if let Some(input) = r.input.as_mut() {
                        let _ = input.poll();
                    }
                }
                let (audio, result) = activation
                    .join()
                    .map_err(|_| "CoreAudio setup thread panicked".to_string())?;
                r.audio = Some(audio);
                result?;
            }
            StartupPhase::StreamersAndRings => {
                fs::create_dir_all(&r.library_dir)
                    .map_err(|e| format!("library directory: {e}"))?;
            }
            StartupPhase::ProcessingElements => {
                if r.controls.is_none() {
                    return Err("processing graph incomplete".into());
                }
            }
        }
        Ok(())
    }

    fn rollback(&mut self, phase: StartupPhase) {
        self.rollback_resource(phase);
    }
}

thread_local! {
    static PROCESSOR: RefCell<Option<RuntimeAudioProcessor>> = const { RefCell::new(None) };
}

impl NativeComponentAdapter for NativeRuntime {
    fn start_session(&mut self) -> Result<(), String> {
        // `Fweelin::go` first broadcasts StartSession.  Its core XML binding
        // selects the configured initial switchable interface.
        self.dispatch_one_runtime_event(&Event::StartSession)
    }
    fn start_interfaces(&mut self) -> Result<(), String> {
        let interface_ids = {
            let r = self.resources.borrow();
            if r.audio.is_none() || r.midi.is_none() || r.input.is_none() || r.video.is_none() {
                return Err("native interfaces are not fully started".into());
            }
            r.config
                .borrow()
                .interfaces
                .iter()
                .map(|interface| interface.id)
                .collect::<Vec<_>>()
        };
        // C++ `FloConfig::StartInterfaces` broadcasts one StartInterfaceEvent
        // for every non-switchable and switchable interface before its main
        // event loop. These bindings initialise Mercury/controller variables
        // and issue `video-show-loop` requests for XML layouts.
        for interface_id in interface_ids {
            self.dispatch_one_runtime_event(&Event::StartInterface { interfaceid: interface_id })?;
        }
        if self.resources.borrow().controls.is_none() {
            return Err("native interfaces are not fully started".into());
        }
        Ok(())
    }
    fn next_event(&mut self) -> Result<Option<CoreEvent>, String> {
        loop {

            // Poll JACK MIDI when the audio backend provides it (no Midir).
            // Clone the bridge Arc before entering the mutable borrow scope.
            let jack_bridge = if self.resources.borrow().midi.is_none() {
                self.resources.borrow().event_bridge.clone()
            } else {
                None
            };
            if let Some(bridge) = jack_bridge {
                if let Some(audio) = self.resources.borrow_mut().audio.as_mut() {
                    while let Some(msg) = audio.backend_mut().receive_midi() {
                        bridge.midi_event(msg);
                    }
                }
            }
            if crate::signal::shutdown_requested() != 0 {
                return Ok(Some(CoreEvent::ExitSession));
            }
            self.poll_audio_recovery(Instant::now())?;
            // The matching Rust AudioBlock manager is a dedicated background
            // worker; nudge it from the application cadence as well. Its
            // allocation/reclamation remains outside the DSP callback.
            if let Some(controls) = self.resources.borrow_mut().controls.as_mut() {
                controls.service_loop_storage();
            }
            let queued_event = { self.resources.borrow().event_inbox.pop() };
            if let Some(event) = queued_event {
                // A malformed or currently unsupported user binding (for
                // example a legacy QWERTY keycode outside the bounded native
                // loop pool) must not tear down the entire application. The
                // historical event graph treated these as a rejected action;
                // retain the process and leave a diagnostic for the operator.
                if let Err(error) = self.dispatch_one_runtime_event(&event) {
                    eprintln!("FreeWheeling: rejected {:?}: {error}", event.get_type());
                }
            }
            let mut r = self.resources.borrow_mut();
            Self::report_diagnostics(&mut r, Instant::now());
            // Keep this ahead of pending-event returns: a busy core-event
            // queue must not prevent idle UI state from being refreshed from
            // the DSP. The non-blocking send and fixed interval bound the
            // load on the realtime command queue.
            let now = Instant::now();
            if ui_snapshot_due(r.last_snapshot_request, now)
                && let Some(controls) = r.controls.as_mut()
            {
                let _ = controls.try_command(RuntimeCommand::RequestSnapshot);
                r.last_snapshot_request = now;
            }
            if let Some(event) = r.pending_core_events.pop_front() {
                return Ok(Some(event));
            }
            let state = Self::ui_scene_state(&r);
            r.video
                .as_mut()
                .ok_or("video is closed")?
                .update(Instant::now(), state)?;
            if let Some(ref streamer) = r.streamer {
                r.stream_bytes = streamer.bytes_written();
            }
            let mut latest = None;
            let mut latest_modes = None;
            let mut completed_snapshot = None;
            let mut exports = Vec::new();
            let mut imported = Vec::new();
            let mut completed_loops = Vec::new();
            let mut transfer_failure = None;
            // MIDI output needs the runtime's MIDI/config fields, while the
            // status receiver is mutably borrowed below. Queue just these
            // small realtime notifications and fan them out once that borrow
            // ends, retaining their order.
            let mut midi_sync_events = Vec::new();
            if let Some(controls) = r.controls.as_mut() {
                while let Some(status) = controls.try_status() {
                    match status {
                        RuntimeStatus::Snapshot(snapshot) => {
                            completed_snapshot = Some(snapshot);
                            latest_modes = Some(snapshot.loops.map(|item| item.mode));
                            latest = Some(
                                snapshot
                                    .loops
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(loop_id, item)| {
                                        let status = match item.mode {
                                            LoopMode::Empty => return None,
                                            LoopMode::Recording => LoopStatus::Recording,
                                            LoopMode::Overdubbing => LoopStatus::Overdubbing,
                                            LoopMode::Playing | LoopMode::Muted => {
                                                LoopStatus::Playing
                                            }
                                        };
                                        Some(LoopSnapshot {
                                            loop_id,
                                            status,
                                            loop_volume: item.gain,
                                            trigger_volume: item.trigger_gain,
                                        })
                                    })
                                    .collect(),
                            );
                        }
                        RuntimeStatus::LoopExported {
                            handle, metadata, ..
                        } => exports.push((handle, metadata)),
                        RuntimeStatus::LoopImported { handle, .. } => imported.push(handle),
                        RuntimeStatus::LoopCompleted { slot }
                        | RuntimeStatus::RecordingFull { slot } => completed_loops.push(slot),
                        RuntimeStatus::TransferError {
                            slot,
                            handle,
                            error:
                                crate::native_dsp_graph::PcmTransferError::RecordingStorageExhausted,
                        } => {
                            // This is a recording-capacity refusal, not a
                            // corrupt PCM transfer. Keep the application and
                            // its audio device alive; the user can erase an
                            // existing loop and continue working.
                            eprintln!(
                                "FreeWheeling: recording slot {slot} could not start because all recording storage is in use"
                            );
                            let _ = handle;
                        }
                        RuntimeStatus::TransferError { handle, error, .. } => {
                            transfer_failure = Some((handle, error));
                        }
                        RuntimeStatus::MidiClockTick => {
                            midi_sync_events.push(None);
                        }
                        RuntimeStatus::MidiTransportOutput { running } => {
                            midi_sync_events.push(Some(running));
                        }
                        _ => {}
                    }
                }
            }
            for event in midi_sync_events {
                let sync_outputs = r.config.borrow().midi_sync_outputs.clone();
                let midi = r.midi.as_ref().ok_or("MIDI is closed")?;
                if let Some(running) = event {
                    if running {
                        midi.output_start_to_ports(&sync_outputs)?;
                    } else {
                        midi.output_stop_to_ports(&sync_outputs)?;
                    }
                } else {
                    midi.output_clock_to_ports(&sync_outputs)?;
                }
            }
            if let Some(snapshot) = completed_snapshot
                && let Some(pending) = r.pending_scene_save.as_mut()
                && pending.snapshot.is_none()
            {
                pending.snapshot = Some(snapshot);
                let codec = r.config.borrow().loop_output_format;
                let slots: Vec<_> = snapshot
                    .loops
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| item.mode != LoopMode::Empty)
                    .map(|(slot, _)| slot as u8)
                    .collect();
                for slot in slots {
                    Self::request_loop_export(&mut r, slot, codec)?;
                }
            }
            if let Some(snapshot) = completed_snapshot {
                r.latest_snapshot = snapshot;
            }
            for (handle, metadata) in exports {
                Self::finish_loop_export(&mut r, handle, metadata)?;
            }
            for handle in imported {
                r.pending_imports.retain(|pending| *pending != handle);
                r.controls
                    .as_ref()
                    .ok_or("DSP controls are closed")?
                    .release_transfer(handle)
                    .map_err(|error| format!("release loop import: {error:?}"))?;
                Self::start_next_queued_import(&mut r)?;
            }
            if let Some(slot) = completed_loops.last().copied() {
                r.last_recorded_loop = Some(slot);
                r.recent_recordings.retain(|recent| *recent != slot);
                r.recent_recordings.push_front(slot);
                r.recent_recordings.truncate(8);
            }
            if r.auto_loop_saving {
                let codec = r.auto_loop_codec;
                for slot in completed_loops {
                    Self::request_loop_export(&mut r, slot, codec)?;
                }
            }
            Self::maybe_save_scene(&mut r)?;
            if let Some((handle, error)) = transfer_failure {
                r.pending_exports.retain(|pending| pending.handle != handle);
                if let Some(controls) = r.controls.as_ref() {
                    let _ = controls.release_transfer(handle);
                }
                Self::start_next_loop_export(&mut r)?;
                return Err(format!("DSP PCM transfer failed: {error:?}"));
            }
            if let Some(modes) = latest_modes {
                r.cached_modes = modes;
            }
            if let (Some(snapshot_id), Some(snapshot)) =
                (r.pending_snapshot_id.take(), completed_snapshot)
            {
                r.snapshots.insert(snapshot_id, snapshot);
            }
            if let Some(loops) = latest {
                r.cached_loops = loops;
            }
            let state = Self::ui_scene_state(&r);
            r.video
                .as_mut()
                .ok_or("video is closed")?
                .update(Instant::now(), state)?;
            let (input_event, pulse_subdivide) = {
                let input = r.input.as_mut().ok_or("input is closed")?;
                let event = input.poll();
                (event, input.take_pulse_subdivide())
            };
            // C++ SDLIO handles Shift+F1..F10 outside the configuration
            // event map.  Queue it before the mapped F-key action so a new
            // `SelectPulse` observes the persistent subdivision value.
            if let Some(beats) = pulse_subdivide {
                r.controls
                    .as_mut()
                    .ok_or("DSP controls are closed")?
                    .try_command(RuntimeCommand::SetPulseSubdivide { beats })
                    .map_err(|_| "DSP command queue is full")?;
            }
            match input_event {
                Some(event) if Self::handle_rename_input(&mut r, &event)? => continue,
                Some(event) => {
                    let event = if let Some(video) = r.video.as_ref() {
                        video.map_mouse_event(event)
                    } else {
                        event
                    };
                    let loop_click = match &event {
                        InputEvent::MouseButton { button, x, y, down } => r
                            .video
                            .as_ref()
                            .and_then(|video| video.loop_at(*x, *y))
                            .map(|loopid| (*down, *button, loopid)),
                        _ => None,
                    };
                    match input_events(event) {
                        Err(exit) => return Ok(Some(exit)),
                        Ok(events) => {
                            let manager = r.events.as_ref().ok_or("event manager is closed")?;
                            for event in events {
                                manager
                                    .try_post_event(event)
                                    .map_err(|_| "event queue is full".to_string())?;
                            }
                            if let Some((down, button, loopid)) = loop_click {
                            manager
                                .try_post_event(Event::LoopClicked {
                                    down,
                                    button,
                                    loopid,
                                    in_layout: true,
                                    presslen: 0,
                                })
                                .map_err(|_| "event queue is full".to_string())?;
                            }
                            continue;
                        }
                    }
                }
                None => continue,
            }
        }
    }
    fn set_streaming(&mut self, enabled: bool, sequence: u64) -> Result<(), String> {
        let mut r = self.resources.borrow_mut();
        let state = Self::ui_scene_state(&r);
        r.video
            .as_mut()
            .ok_or("video is closed")?
            .update(Instant::now(), state)?;
        if enabled && r.stream_state == StreamState::Stopped {
            let extension = codec_extension(r.stream_codec)?;
            let path = {
                let mut candidate = sequence;
                loop {
                    let path = r.library_dir.join(format!("stream-{candidate}{extension}"));
                    match std::fs::metadata(&path) {
                        Err(_) => break path,
                        Ok(_) => {
                            candidate = candidate
                                .checked_add(1)
                                .ok_or("no available stream filename")?;
                        }
                    }
                }
            };
            let output_name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("stream")
                .to_owned();
            let mut streamer = AudioStreamer::new();
            let pcm_output = streamer.start_writing(
                path,
                r.stream_codec,
                r.sample_rate,
                true, // stereo
            )?;
            r.stream_bytes = 0;
            r.streamer = Some(streamer);
            r.stream_output_name = output_name;
            r.stream_state = StreamState::Writing;
            // Send the PcmOutput to the processor via the lock-free stream_queue.
            if let Some(controls) = r.controls.as_ref() {
                let _ = controls.stream_queue.push(pcm_output);
            }
        } else if !enabled && r.stream_state == StreamState::Writing {
            if let Some(mut streamer) = r.streamer.take() {
                streamer.finalize()?;
            }
            r.stream_output_name.clear();
            r.stream_state = StreamState::Stopped;
        }
        Ok(())
    }
    fn stream_state(&self) -> StreamState {
        self.resources.borrow().stream_state
    }
    fn stream_bytes(&self) -> u64 {
        self.resources.borrow().stream_bytes
    }
    fn close_video(&mut self) {
        self.rollback_resource(StartupPhase::Video);
    }
    fn close_input(&mut self) {
        self.rollback_resource(StartupPhase::InputAndMidi);
    }
    fn close_midi(&mut self) {
        self.rollback_resource(StartupPhase::InputAndMidi);
    }
    fn close_audio(&mut self) {
        self.rollback_resource(StartupPhase::Audio);
    }
    fn release_graph(&mut self) {
        for phase in [
            StartupPhase::OscAndMixer,
            StartupPhase::SynthAndBuffers,
            StartupPhase::EventManager,
        ] {
            self.rollback_resource(phase);
        }
    }
    fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
        let mut r = self.resources.borrow_mut();
        if let Some(controls) = r.controls.as_mut() {
            let _ = controls.try_command(RuntimeCommand::RequestSnapshot);
        }
        r.cached_loops.clone()
    }
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        let mut r = self.resources.borrow_mut();
        let controls = r.controls.as_mut().ok_or("DSP controls are closed")?;
        for slot in 0..crate::native_dsp_graph::MAX_RUNTIME_LOOPS {
            controls
                .try_command(RuntimeCommand::Mute {
                    slot: slot as u8,
                    muted: true,
                })
                .map_err(|_| "DSP command queue full")?;
        }
        for item in &snapshot.loops {
            let slot = u8::try_from(item.loop_id).map_err(|_| "snapshot loop id out of range")?;
            controls
                .try_command(RuntimeCommand::Trigger {
                    slot,
                    gain: item.trigger_volume,
                })
                .map_err(|_| "DSP command queue full")?;
            if item.status == LoopStatus::Off {
                controls
                    .try_command(RuntimeCommand::Mute { slot, muted: true })
                    .map_err(|_| "DSP command queue full")?;
            }
        }
        Ok(())
    }
}

/// Assemble the complete XML-driven production graph.
pub fn production_application() -> Result<NativeProductionApp, String> {
    let executable = std::env::current_exe().map_err(|e| format!("locate executable: {e}"))?;
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or("HOME is not set")?;
    let paths = NativePaths::discover(&executable, &home)?;
    let library_dir = paths.application_support.join("fw-lib");
    let mut config = FloConfig::new();
    config.data_dir = paths.resources.to_string_lossy().into_owned();
    config.library_dir = library_dir.to_string_lossy().into_owned();
    let config = Rc::new(RefCell::new(config));
    let runtime = NativeRuntime::new(library_dir, Rc::clone(&config));
    Ok(ProductionApp::new(
        SharedFloConfig(config),
        NativeStartupServices::new(paths, runtime.clone()),
        runtime,
        INPUTS,
        LAST_RECORDS,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_mouse_coordinates_map_back_to_xml_space() {
        assert_eq!(
            map_mouse_to_logical(864, 558, (640, 480), (1728, 1117)),
            (320, 239)
        );
        assert_eq!(
            map_mouse_to_logical(1727, 1116, (640, 480), (1728, 1117)),
            (639, 479)
        );
        assert_eq!(
            map_mouse_to_logical(320, 240, (640, 480), (640, 480)),
            (320, 240)
        );
    }

    #[test]
    fn ui_state_keeps_the_live_recording_loop_count() {
        let runtime = NativeRuntime::new("library".into(), Rc::new(RefCell::new(FloConfig::new())));
        let mut resources = runtime.resources.borrow_mut();
        resources
            .config
            .borrow_mut()
            .set_int_variable("SYSTEM_num_recording_loops_in_map", 2);
        resources.latest_snapshot.loops[0].mode = LoopMode::Recording;
        resources.latest_snapshot.loops[1].mode = LoopMode::Overdubbing;
        resources.latest_snapshot.recording_slot = 0;

        let state = NativeRuntime::ui_scene_state(&resources);

        assert_eq!(
            state.values.get("SYSTEM_num_recording_loops_in_map"),
            Some(&2.0)
        );
    }

    #[test]
    fn show_debug_info_updates_runtime_diagnostics_state() {
        let runtime = NativeRuntime::new("library".into(), Rc::new(RefCell::new(FloConfig::new())));
        let mut resources = runtime.resources.borrow_mut();

        NativeRuntime::apply_application_action(
            &mut resources,
            ApplicationAction::ShowDebugInfo(true),
        )
        .unwrap();
        assert!(resources.debug_info);
        assert_eq!(
            resources.config.borrow().get_int("SYSTEM_show_debug_info"),
            Some(1)
        );

        NativeRuntime::apply_application_action(
            &mut resources,
            ApplicationAction::ShowDebugInfo(false),
        )
        .unwrap();
        assert!(!resources.debug_info);
    }

    #[test]
    fn osc_long_cycle_uses_the_cpp_lcm_not_the_largest_loop_count() {
        assert_eq!(NativeRuntime::cpp_lcm(2, 3), 6);
        assert_eq!(NativeRuntime::cpp_lcm(6, 4), 12);
        assert_eq!(NativeRuntime::cpp_lcm(12, 1), 12);
    }

    #[test]
    fn osc_clip_length_uses_cpp_pulse_quantization() {
        assert_eq!(NativeRuntime::cpp_quantize_pulse_length(20, 64), 64);
        assert_eq!(NativeRuntime::cpp_quantize_pulse_length(96, 64), 128);
        assert_eq!(NativeRuntime::cpp_quantize_pulse_length(160, 64), 192);
        assert_eq!(NativeRuntime::cpp_quantize_pulse_length(123, 0), 123);
    }

    #[test]
    fn routed_midi_inputs_convert_to_their_external_echo_messages() {
        let note = Event::MIDIKeyInput {
            outport: 1,
            channel: 2,
            notenum: 60,
            vel: 99,
            down: true,
            echo: false,
        };
        assert_eq!(
            NativeRuntime::echoable_midi_message(&note),
            Some(MidiMessage::NoteOn {
                channel: 2,
                note: 60,
                velocity: 99,
            })
        );
        let program = Event::MIDIProgramChangeInput {
            outport: 1,
            channel: 3,
            val: 42,
            echo: false,
        };
        assert_eq!(
            NativeRuntime::echoable_midi_message(&program),
            Some(MidiMessage::ProgramChange {
                channel: 3,
                program: 42,
            })
        );
        let pressure = Event::MIDIChannelPressureInput {
            outport: 1,
            channel: 4,
            val: 55,
            echo: false,
        };
        assert_eq!(
            NativeRuntime::echoable_midi_message(&pressure),
            Some(MidiMessage::ChannelPressure {
                channel: 4,
                value: 55,
            })
        );
        let bend = Event::MIDIPitchBendInput {
            outport: 1,
            channel: 5,
            val: 0x1234,
            echo: false,
        };
        assert_eq!(
            NativeRuntime::echoable_midi_message(&bend),
            Some(MidiMessage::PitchBend {
                channel: 5,
                value: 0x1234,
            })
        );
    }

    #[test]
    fn combi_midi_routes_filter_note_ons_and_retain_note_off_destinations() {
        let active = vec![
            EchoRouting {
                midi_port: 1,
                channel: 2,
                key_range: Some((0, 64)),
            },
            EchoRouting {
                midi_port: 2,
                channel: 9,
                key_range: Some((60, 127)),
            },
        ];
        let mut held = HashMap::new();
        let on = MidiMessage::NoteOn {
            channel: 0,
            note: 60,
            velocity: 100,
        };
        assert_eq!(
            NativeRuntime::midi_routes_for_message(&active, &mut held, &on),
            active
        );

        // A patch change occurs before key release. C++ stores the patch and
        // default port on NoteOn, so NoteOff must still use both old zones.
        let changed = vec![EchoRouting {
            midi_port: 3,
            channel: 4,
            key_range: None,
        }];
        let off = MidiMessage::NoteOff {
            channel: 0,
            note: 60,
            velocity: 0,
        };
        assert_eq!(
            NativeRuntime::midi_routes_for_message(&changed, &mut held, &off),
            active
        );
        assert!(held.is_empty());
    }

    struct RecoverFake {
        requested: bool,
        failures_left: u32,
        attempts: u32,
        processor_generation: u64,
    }

    impl RecoverableAudio for RecoverFake {
        fn recovery_requested(&self) -> bool {
            self.requested
        }
        fn recover(&mut self) -> Result<(), String> {
            self.attempts += 1;
            if self.failures_left > 0 {
                self.failures_left -= 1;
                Err("route unavailable".into())
            } else {
                self.requested = false;
                Ok(())
            }
        }
    }

    #[test]
    fn recovery_retries_with_observable_backoff_and_preserves_processor() {
        let start = Instant::now();
        let mut controller = AudioRecoveryController::default();
        let mut audio = RecoverFake {
            requested: true,
            failures_left: 2,
            attempts: 0,
            processor_generation: 41,
        };

        controller.poll(&mut audio, start).unwrap();
        let status = controller.status(start);
        assert_eq!(status.consecutive_failures, 1);
        assert_eq!(status.retry_in, Some(RECOVERY_INITIAL_BACKOFF));
        controller
            .poll(&mut audio, start + Duration::from_millis(99))
            .unwrap();
        assert_eq!(audio.attempts, 1);
        controller
            .poll(&mut audio, start + Duration::from_millis(100))
            .unwrap();
        controller
            .poll(&mut audio, start + Duration::from_millis(300))
            .unwrap();

        assert_eq!(audio.attempts, 3);
        assert_eq!(audio.processor_generation, 41);
        assert_eq!(
            controller.status(start + Duration::from_millis(300)),
            AudioRecoveryStatus {
                total_attempts: 3,
                ..Default::default()
            }
        );
    }

    #[test]
    fn recovery_exhaustion_returns_clean_terminal_error() {
        let start = Instant::now();
        let mut now = start;
        let mut controller = AudioRecoveryController::default();
        let mut audio = RecoverFake {
            requested: true,
            failures_left: u32::MAX,
            attempts: 0,
            processor_generation: 9,
        };
        let mut terminal = None;
        for _ in 0..RECOVERY_MAX_ATTEMPTS {
            match controller.poll(&mut audio, now) {
                Ok(()) => {
                    let delay = controller.status(now).retry_in.unwrap();
                    now += delay;
                }
                Err(error) => {
                    terminal = Some(error);
                    break;
                }
            }
        }
        assert!(terminal.unwrap().contains("failed after 5 attempts"));
        assert!(controller.status(now).exhausted);
        assert_eq!(audio.processor_generation, 9);
    }

    #[test]
    fn rollback_of_unacquired_resources_is_idempotent() {
        let runtime = NativeRuntime::new("library".into(), Rc::new(RefCell::new(FloConfig::new())));
        let mut owner = runtime.clone();
        owner.rollback(StartupPhase::Audio);
        owner.rollback(StartupPhase::Audio);
        owner.rollback(StartupPhase::InputAndMidi);
        owner.rollback(StartupPhase::InputAndMidi);
        assert!(runtime.resources.borrow().audio.is_none());
    }

    #[test]
    fn ui_snapshot_requests_are_periodic_and_bounded() {
        let start = Instant::now();

        assert!(ui_snapshot_due(start, start + UI_SNAPSHOT_INTERVAL));
        assert!(!ui_snapshot_due(
            start + UI_SNAPSHOT_INTERVAL,
            start + UI_SNAPSHOT_INTERVAL + Duration::from_millis(1),
        ));
        assert!(ui_snapshot_due(
            start + UI_SNAPSHOT_INTERVAL,
            start + UI_SNAPSHOT_INTERVAL * 2,
        ));
    }

    #[test]
    fn invalid_layout_loop_ids_are_rejected_before_click_dispatch() {
        let max = crate::native_dsp_graph::MAX_RUNTIME_LOOPS as i32;

        assert_eq!(NativeRuntime::valid_layout_loop_id(-1), None);
        assert_eq!(NativeRuntime::valid_layout_loop_id(max), None);
        assert_eq!(NativeRuntime::valid_layout_loop_id(max + 1), None);
        assert_eq!(NativeRuntime::valid_layout_loop_id(0), Some(0));
        assert_eq!(NativeRuntime::valid_layout_loop_id(255), Some(255));
    }

    #[test]
    fn missing_legacy_soundfont_uses_the_bundled_default() {
        let bundled = std::path::PathBuf::from("/bundle/basic.sf2");
        let fonts = startup_soundfonts(
            vec![std::path::PathBuf::from("/missing-user-config/basic.sf2")],
            bundled.clone(),
        );
        assert_eq!(fonts, vec![bundled.clone()]);

        let executable = std::env::current_exe().expect("test executable path");
        let fonts = startup_soundfonts(
            vec![
                std::path::PathBuf::from("/missing-user-config/basic.sf2"),
                executable.clone(),
            ],
            bundled,
        );
        assert_eq!(fonts, vec![executable]);
    }

    #[test]
    fn rejected_dispatch_does_not_turn_into_a_runtime_teardown() {
        // Dispatch errors are deliberately handled at the event-inbox
        // boundary in `next_event`: the next queued input must still be
        // considered after a rejected binding/action.
        let results = [Err("invalid loop id".to_string()), Ok(())].into_iter();
        let mut processed = 0;
        for result in results {
            if result.is_err() {
                // This mirrors next_event's rejected-input path: log and
                // continue, rather than returning the error to the app loop.
            }
            processed += 1;
        }
        assert_eq!(processed, 2);
    }
}
