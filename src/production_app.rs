//! Production application graph and lifecycle orchestration.

use crate::application_services::Components;
use crate::core::{CoreEvent, LoopSnapshot, Snapshot, StreamState};
use crate::core_startup::{StartupConfig, StartupServices};
use crate::fweelin_app::Fweelin;

/// Operations supplied by the fully assembled audio/MIDI/video/DSP graph.
/// Event polling should block for a short bounded interval and return `None`
/// only when the native event source has ended.
pub trait NativeComponentAdapter {
    fn start_session(&mut self) -> Result<(), String>;
    fn start_interfaces(&mut self) -> Result<(), String>;
    fn next_event(&mut self) -> Result<Option<CoreEvent>, String>;
    fn set_streaming(&mut self, enabled: bool, sequence: u64) -> Result<(), String>;
    fn stream_state(&self) -> StreamState;
    fn stream_bytes(&self) -> u64;
    fn close_video(&mut self);
    fn close_input(&mut self);
    fn close_midi(&mut self);
    fn close_audio(&mut self);
    fn release_graph(&mut self);
    fn snapshot_loops(&self) -> Vec<LoopSnapshot>;
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String>;
}

#[path = "native_runtime.rs"]
pub mod native_runtime;

/// Concrete owner for the native graph. Cleanup methods are idempotent so
/// setup errors, run errors, explicit shutdown and `Drop` share one path.
pub struct NativeComponents<A: NativeComponentAdapter> {
    adapter: A,
    video_open: bool,
    input_open: bool,
    midi_open: bool,
    audio_open: bool,
    graph_open: bool,
}

impl<A: NativeComponentAdapter> NativeComponents<A> {
    pub fn new(adapter: A) -> Self {
        Self {
            adapter,
            video_open: false,
            input_open: false,
            midi_open: false,
            audio_open: false,
            graph_open: false,
        }
    }
    pub fn adapter(&self) -> &A {
        &self.adapter
    }
    pub fn adapter_mut(&mut self) -> &mut A {
        &mut self.adapter
    }
}

impl<A: NativeComponentAdapter> Components for NativeComponents<A> {
    fn start_session(&mut self) -> Result<(), String> {
        self.adapter.start_session()?;
        self.graph_open = true;
        Ok(())
    }
    fn start_interfaces(&mut self) -> Result<(), String> {
        self.adapter.start_interfaces()?;
        self.video_open = true;
        self.input_open = true;
        self.midi_open = true;
        self.audio_open = true;
        Ok(())
    }
    fn next_event(&mut self) -> Result<Option<CoreEvent>, String> {
        self.adapter.next_event()
    }
    fn set_streaming(&mut self, enabled: bool, sequence: u64) -> Result<(), String> {
        self.adapter.set_streaming(enabled, sequence)
    }
    fn stream_state(&self) -> StreamState {
        self.adapter.stream_state()
    }
    fn stream_bytes(&self) -> u64 {
        self.adapter.stream_bytes()
    }
    fn close_video(&mut self) {
        if self.video_open {
            self.adapter.close_video();
            self.video_open = false;
        }
    }
    fn close_sdl(&mut self) {
        if self.input_open {
            self.adapter.close_input();
            self.input_open = false;
        }
    }
    fn close_midi(&mut self) {
        if self.midi_open {
            self.adapter.close_midi();
            self.midi_open = false;
        }
    }
    fn close_audio(&mut self) {
        if self.audio_open {
            self.adapter.close_audio();
            self.audio_open = false;
        }
    }
    fn shutdown(&mut self) {
        self.close_video();
        self.close_sdl();
        self.close_midi();
        self.close_audio();
        if self.graph_open {
            self.adapter.release_graph();
            self.graph_open = false;
        }
    }
    fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
        self.adapter.snapshot_loops()
    }
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        self.adapter.restore_snapshot(snapshot)
    }
}

impl<A: NativeComponentAdapter> Drop for NativeComponents<A> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct ProductionApp<C: StartupConfig, S: StartupServices, A: NativeComponentAdapter> {
    app: Fweelin<C, S, NativeComponents<A>>,
}

impl<C: StartupConfig, S: StartupServices, A: NativeComponentAdapter> ProductionApp<C, S, A> {
    pub fn new(config: C, startup: S, components: A, inputs: usize, last_records: usize) -> Self {
        Self {
            app: Fweelin::new(
                config,
                startup,
                NativeComponents::new(components),
                inputs,
                last_records,
            ),
        }
    }
    pub fn app(&self) -> &Fweelin<C, S, NativeComponents<A>> {
        &self.app
    }
    pub fn app_mut(&mut self) -> &mut Fweelin<C, S, NativeComponents<A>> {
        &mut self.app
    }

    /// Set up, run the main-thread event loop, and always perform clean
    /// shutdown. Startup errors retain their failing phase from `core_startup`.
    pub fn run(&mut self) -> Result<(), String> {
        self.app.setup()?;
        let result = self.app.go();
        self.app.shutdown();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Recorder(Arc<Mutex<Vec<&'static str>>>);
    impl NativeComponentAdapter for Recorder {
        fn start_session(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn start_interfaces(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn next_event(&mut self) -> Result<Option<CoreEvent>, String> {
            Ok(None)
        }
        fn set_streaming(&mut self, _: bool, _: u64) -> Result<(), String> {
            Ok(())
        }
        fn stream_state(&self) -> StreamState {
            StreamState::Stopped
        }
        fn stream_bytes(&self) -> u64 {
            0
        }
        fn close_video(&mut self) {
            self.0.lock().unwrap().push("video");
        }
        fn close_input(&mut self) {
            self.0.lock().unwrap().push("input");
        }
        fn close_midi(&mut self) {
            self.0.lock().unwrap().push("midi");
        }
        fn close_audio(&mut self) {
            self.0.lock().unwrap().push("audio");
        }
        fn release_graph(&mut self) {
            self.0.lock().unwrap().push("graph");
        }
        fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
            Vec::new()
        }
        fn restore_snapshot(&mut self, _: &Snapshot) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn shutdown_quiesces_interfaces_before_releasing_graph_and_is_idempotent() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut components = NativeComponents::new(Recorder(calls.clone()));
        components.start_session().unwrap();
        components.start_interfaces().unwrap();
        components.shutdown();
        components.shutdown();
        drop(components);
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["video", "input", "midi", "audio", "graph"]
        );
    }
}
