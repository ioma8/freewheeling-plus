//! Concrete application adapter for [`crate::core::CoreServices`].
//!
//! The migrated modules deliberately stop at backend-independent boundaries.
//! This type is the glue which calls those boundaries in application order;
//! the `Components` implementation is where a platform supplies JACK/ALSA,
//! SDL/OpenGL, configuration and DSP ownership.

use crate::core::{CoreEvent, CoreServices, LoopSnapshot, Snapshot, StreamState};
use crate::core_startup::{self, StartupConfig, StartupServices};
use crate::sdlio::{InputEvent, SdlBackend, SdlIo};

/// The application-owned part of the migrated graph.
///
/// Implementations must perform the operation requested. In particular,
/// streaming and snapshot methods must not silently succeed without doing
/// work. Hardware-specific implementations can be generic over the backend
/// types from `audioio`, `midiio`, `sdlio`, and `videoio`.
pub trait Components {
    fn start_session(&mut self) -> Result<(), String>;
    fn start_interfaces(&mut self) -> Result<(), String>;
    fn next_event(&mut self) -> Result<Option<CoreEvent>, String>;
    fn set_streaming(&mut self, enabled: bool, sequence: u64) -> Result<(), String>;
    fn stream_state(&self) -> StreamState;
    fn stream_bytes(&self) -> u64;
    fn close_video(&mut self);
    fn close_sdl(&mut self);
    fn close_midi(&mut self);
    fn close_audio(&mut self);
    fn shutdown(&mut self);
    fn snapshot_loops(&self) -> Vec<LoopSnapshot>;
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String>;
}

/// Converts platform input into core events. SDL events not understood by the
/// core are consumed and reported as no event.
pub fn core_event(event: InputEvent) -> Option<CoreEvent> {
    match event {
        InputEvent::Quit => Some(CoreEvent::ExitSession),
        _ => None,
    }
}

/// A startup/session adapter. `S` and `C` are the migrated startup contracts;
/// `P` owns the actual audio, MIDI, video, browser, event and persistence
/// components.
pub struct ApplicationServices<C, S, P> {
    config: C,
    startup: S,
    components: P,
    inputs: usize,
    last_records: usize,
    startup_active: bool,
}

impl<C, S, P> ApplicationServices<C, S, P> {
    pub fn new(config: C, startup: S, components: P, inputs: usize, last_records: usize) -> Self {
        Self {
            config,
            startup,
            components,
            inputs,
            last_records,
            startup_active: false,
        }
    }

    pub fn config(&self) -> &C {
        &self.config
    }
    pub fn config_mut(&mut self) -> &mut C {
        &mut self.config
    }
    pub fn startup(&self) -> &S {
        &self.startup
    }
    pub fn startup_mut(&mut self) -> &mut S {
        &mut self.startup
    }
    pub fn components(&self) -> &P {
        &self.components
    }
    pub fn components_mut(&mut self) -> &mut P {
        &mut self.components
    }
}

impl<C: StartupConfig, S: StartupServices, P: Components> CoreServices
    for ApplicationServices<C, S, P>
{
    fn setup(&mut self) -> Result<(), String> {
        self.startup_active = true;
        core_startup::setup(
            &mut self.config,
            &mut self.startup,
            self.inputs,
            self.last_records,
        )
        .map_err(|e| {
            // core_startup::setup already rolled the startup services back.
            self.startup_active = false;
            format!("startup {}: {}", e.phase, e.message)
        })?;
        Ok(())
    }
    fn start_session(&mut self) -> Result<(), String> {
        self.components.start_session()
    }
    fn start_interfaces(&mut self) -> Result<(), String> {
        self.components.start_interfaces()
    }
    fn poll_event(&mut self) -> Result<Option<CoreEvent>, String> {
        self.components.next_event()
    }
    fn set_streaming(&mut self, enabled: bool, sequence: u64) -> Result<(), String> {
        self.components.set_streaming(enabled, sequence)
    }
    fn stream_state(&self) -> StreamState {
        self.components.stream_state()
    }
    fn stream_bytes(&self) -> u64 {
        self.components.stream_bytes()
    }
    fn close_video(&mut self) {
        self.components.close_video()
    }
    fn close_sdl(&mut self) {
        self.components.close_sdl()
    }
    fn close_midi(&mut self) {
        self.components.close_midi()
    }
    fn close_audio(&mut self) {
        self.components.close_audio()
    }
    fn shutdown(&mut self) {
        self.components.shutdown();
        if self.startup_active {
            self.startup.rollback_setup();
            self.startup_active = false;
        }
    }
    fn rollback_setup(&mut self) {
        // Avoid a second rollback when core_startup already handled a failed
        // phase; Core calls this hook for every setup error.
        if self.startup_active {
            self.startup.rollback_setup();
            self.startup_active = false;
        }
    }
    fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
        self.components.snapshot_loops()
    }
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        self.components.restore_snapshot(snapshot)
    }
}

/// Small helper for the common SDL-backed event source.
pub struct SdlEvents<B: SdlBackend> {
    pub io: SdlIo<B>,
}
impl<B: SdlBackend> SdlEvents<B> {
    pub fn new(io: SdlIo<B>) -> Self {
        Self { io }
    }
    pub fn poll(&mut self) -> Option<CoreEvent> {
        self.io.poll().and_then(core_event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Core;

    #[derive(Default)]
    struct Config;
    impl StartupConfig for Config {
        fn add_int_constant(&mut self, _: &str, _: i32) {}
        fn add_empty_variable(&mut self, _: &str) {}
        fn parse(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn start(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Startup;
    macro_rules! startup_methods { ($($name:ident),+ $(,)?) => { $(fn $name(&mut self) -> Result<(), String> { Ok(()) })+ }; }
    impl StartupServices for Startup {
        startup_methods!(
            lock_memory,
            init_rt_threads,
            register_main_thread,
            init_platform_threads,
            init_sdl,
            init_memory_manager,
            init_event_manager,
            activate_video,
            wait_for_video,
            init_audio,
            init_core_graph,
            init_synth_and_buffers,
            init_loop_and_scene_browsers,
            init_input_and_midi,
            init_osc_and_mixer,
            link_system_variables,
            activate_signal_processing,
            init_streamers_and_finalize_rings,
            add_processing_elements
        );
        fn rollback_setup(&mut self) {}
    }

    struct TestComponents {
        events: Vec<Option<CoreEvent>>,
        state: StreamState,
        starts: usize,
        closes: usize,
    }
    impl TestComponents {
        fn new() -> Self {
            Self {
                events: vec![Some(CoreEvent::ExitSession)],
                state: StreamState::Stopped,
                starts: 0,
                closes: 0,
            }
        }
    }
    impl Components for TestComponents {
        fn start_session(&mut self) -> Result<(), String> {
            self.starts += 1;
            Ok(())
        }
        fn start_interfaces(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn next_event(&mut self) -> Result<Option<CoreEvent>, String> {
            Ok(self.events.pop().flatten())
        }
        fn set_streaming(&mut self, enabled: bool, _: u64) -> Result<(), String> {
            self.state = if enabled {
                StreamState::Writing
            } else {
                StreamState::Stopped
            };
            Ok(())
        }
        fn stream_state(&self) -> StreamState {
            self.state
        }
        fn stream_bytes(&self) -> u64 {
            12
        }
        fn close_video(&mut self) {}
        fn close_sdl(&mut self) {}
        fn close_midi(&mut self) {}
        fn close_audio(&mut self) {}
        fn shutdown(&mut self) {
            self.closes += 1;
        }
        fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
            Vec::new()
        }
        fn restore_snapshot(&mut self, _: &Snapshot) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn core_lifecycle_reaches_components_and_shuts_down() {
        let services = ApplicationServices::new(Config, Startup, TestComponents::new(), 0, 0);
        let mut core = Core::new(services);
        core.setup().unwrap();
        core.go().unwrap();
        assert_eq!(core.services().components().starts, 1);
        assert_eq!(core.services().components().closes, 1);
    }

    #[test]
    fn quit_is_a_real_core_exit_event() {
        assert_eq!(core_event(InputEvent::Quit), Some(CoreEvent::ExitSession));
    }
}
