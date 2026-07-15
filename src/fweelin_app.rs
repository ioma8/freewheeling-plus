//! Public, top-level Fweelin orchestration.
//!
//! The C++ `Fweelin` object was an owner and a façade.  This equivalent keeps
//! that shape while making each platform boundary a generic, owned component.

use crate::application_services::{ApplicationServices, Components};
use crate::core::{Core, CoreEvent, LoopSnapshot, Snapshot, StreamState};
use crate::core_startup::{StartupConfig, StartupServices};

/// The explicitly owned application domains.  The fields are intentionally
/// public: adapters can inspect their state without making the façade know
/// about JACK, ALSA, SDL, or a particular persistence implementation.
pub struct FweelinComponents<Au, Mi, Vi, Br, Co, Pe> {
    pub audio: Au,
    pub midi: Mi,
    pub video: Vi,
    pub browser: Br,
    pub config: Co,
    pub persistence: Pe,
    pub services: Box<dyn Components>,
}

impl<Au, Mi, Vi, Br, Co, Pe> Components for FweelinComponents<Au, Mi, Vi, Br, Co, Pe> {
    fn start_session(&mut self) -> Result<(), String> {
        self.services.start_session()
    }
    fn start_interfaces(&mut self) -> Result<(), String> {
        self.services.start_interfaces()
    }
    fn next_event(&mut self) -> Result<Option<CoreEvent>, String> {
        self.services.next_event()
    }
    fn set_streaming(&mut self, enabled: bool, sequence: u64) -> Result<(), String> {
        self.services.set_streaming(enabled, sequence)
    }
    fn stream_state(&self) -> StreamState {
        self.services.stream_state()
    }
    fn stream_bytes(&self) -> u64 {
        self.services.stream_bytes()
    }
    fn close_video(&mut self) {
        self.services.close_video()
    }
    fn close_sdl(&mut self) {
        self.services.close_sdl()
    }
    fn close_midi(&mut self) {
        self.services.close_midi()
    }
    fn close_audio(&mut self) {
        self.services.close_audio()
    }
    fn shutdown(&mut self) {
        self.services.shutdown()
    }
    fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
        self.services.snapshot_loops()
    }
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        self.services.restore_snapshot(snapshot)
    }
}

/// A component bundle must provide the actual application operations.  The
/// domain values remain separately typed and owned for wiring and inspection.
/// `Components` is object-safe, so this also permits heterogeneous backends.
pub trait FweelinComponentSet: Components {
    type Audio;
    type Midi;
    type Video;
    type Browser;
    type Config;
    type Persistence;

    fn audio(&self) -> &Self::Audio;
    fn midi(&self) -> &Self::Midi;
    fn video(&self) -> &Self::Video;
    fn browser(&self) -> &Self::Browser;
    fn component_config(&self) -> &Self::Config;
    fn persistence(&self) -> &Self::Persistence;
}

/// Top-level public application façade, equivalent to the public `Fweelin`
/// lifecycle and control surface.
pub struct Fweelin<C: StartupConfig, S: StartupServices, P: Components> {
    core: Core<ApplicationServices<C, S, P>>,
}

impl<C: StartupConfig, S: StartupServices, P: Components> Fweelin<C, S, P> {
    pub fn new(config: C, startup: S, components: P, inputs: usize, last_records: usize) -> Self {
        Self {
            core: Core::new(ApplicationServices::new(
                config,
                startup,
                components,
                inputs,
                last_records,
            )),
        }
    }

    pub fn is_running(&self) -> bool {
        self.core.is_running()
    }
    pub fn setup(&mut self) -> Result<(), String> {
        self.core.setup()
    }
    pub fn go(&mut self) -> Result<(), String> {
        self.core.go()
    }
    pub fn handle_event(&mut self, event: CoreEvent) -> Result<(), String> {
        self.core.handle_event(event)
    }
    pub fn toggle_disk_output(&mut self) -> Result<(), String> {
        self.core.toggle_disk_output()
    }
    /// Stop output and clear the active stream through the same backend path
    /// used by `ToggleDiskOutput`.
    pub fn flush_stream_out_name(&mut self) -> Result<(), String> {
        if self.core.stream_stats().0 == StreamState::Writing {
            self.toggle_disk_output()?;
        }
        Ok(())
    }
    pub fn stream_name(&self) -> &str {
        self.core.stream_name()
    }
    pub fn stream_stats(&self) -> (StreamState, u64) {
        self.core.stream_stats()
    }
    pub fn create_snapshot(&mut self, index: usize, name: impl Into<String>) {
        self.core.create_snapshot(index, name);
    }
    pub fn trigger_snapshot(&mut self, index: usize) -> Result<(), String> {
        self.core.trigger_snapshot(index)
    }
    pub fn snapshot(&self, index: usize) -> Option<&Snapshot> {
        self.core.snapshot(index)
    }
    pub fn components(&self) -> &P {
        self.core.services().components()
    }
    pub fn components_mut(&mut self) -> &mut P {
        self.core.services_mut().components_mut()
    }
    pub fn config(&self) -> &C {
        self.core.services().config()
    }
    pub fn config_mut(&mut self) -> &mut C {
        self.core.services_mut().config_mut()
    }
    pub fn startup(&self) -> &S {
        self.core.services().startup()
    }
    pub fn startup_mut(&mut self) -> &mut S {
        self.core.services_mut().startup_mut()
    }
    pub fn loop_state(&self) -> Vec<LoopSnapshot> {
        self.components().snapshot_loops()
    }
    pub fn shutdown(&mut self) {
        self.core.shutdown();
    }
}
