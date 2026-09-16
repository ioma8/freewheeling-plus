//! Concrete application adapter for [`crate::core::CoreServices`].
//!
//! The migrated modules deliberately stop at backend-independent boundaries.
//! This type is the glue which calls those boundaries in application order;
//! the `Components` implementation is where a platform supplies JACK/ALSA,
//! SDL/OpenGL, configuration and DSP ownership.

use crate::core::{CoreEvent, CoreServices, LoopSnapshot, Snapshot, StreamState};
use crate::core_startup::{self, StartupConfig, StartupServices};
use crate::sdlio::InputEvent;

/// The application-owned part of the migrated graph.
///
/// Implementations must perform the operation requested. In particular,
/// streaming and snapshot methods must not silently succeed without doing
/// work. Hardware-specific implementations can be generic over the backend
/// types from `audioio`, `midiio`, `sdlio`, and `videoio`.
/// The component surface a `Core` drives (§startup phases).
///
/// Errors are `String` on purpose: this is the boundary where the startup and
/// shutdown sequence reports failures to the user, and every implementation
/// (`ProductionApp`, `NativeComponentAdapter`, the test doubles) formats a
/// backend-specific error - a typed `io::Error`, `SnapshotError`,
/// `jack::JackError`, ... - into a message anyway. The typed errors stay at
/// their own layers, where callers can still match on them; introducing a
/// second error type only here would make this trait the odd one out.
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

/// What the core should do with a platform input event.
///
/// A plain `Option` would be ambiguous: `Core::go` ends the session when
/// `poll_event` reports `None`, so an adapter that forwards this mapping must
/// not be able to turn "the core has no use for this key" into a shutdown.
/// Key bindings stay data-driven (`data/*.xml`) so this mapping only covers
/// inputs the core itself owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreEventAction {
    /// The session must handle this event.
    Handle(CoreEvent),
    /// The core ignores this input; the caller keeps processing.
    Ignore,
}

/// Converts platform input into a core action.
pub fn core_event(event: InputEvent) -> CoreEventAction {
    match event {
        InputEvent::Quit => CoreEventAction::Handle(CoreEvent::ExitSession),
        _ => CoreEventAction::Ignore,
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
        .map_err(|error| {
            // core_startup::setup already rolled the startup services back.
            self.startup_active = false;
            // Use the structured error's own Display so the formats cannot
            // drift apart.
            error.to_string()
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
        // Rollback belongs to core_startup::setup (failed phases) and
        // `rollback_setup` below; a committed setup must not be replayed here.
        // Normal teardown is `Components::shutdown`.
        self.components.shutdown();
    }
    fn rollback_setup(&mut self) -> Result<(), String> {
        // Avoid a second rollback when core_startup already handled a failed
        // phase; Core calls this hook for every setup error.
        if !self.startup_active {
            return Ok(());
        }
        self.startup_active = false;
        self.startup.rollback_setup()
    }
    fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
        self.components.snapshot_loops()
    }
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        self.components.restore_snapshot(snapshot)
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
    impl StartupServices for Startup {
        fn lock_memory(&mut self) -> Result<(), String> { Ok(()) }
        fn init_rt_threads(&mut self) -> Result<(), String> { Ok(()) }
        fn register_main_thread(&mut self) -> Result<(), String> { Ok(()) }
        fn init_platform_threads(&mut self) -> Result<(), String> { Ok(()) }
        fn init_sdl(&mut self) -> Result<(), String> { Ok(()) }
        fn init_memory_manager(&mut self) -> Result<(), String> { Ok(()) }
        fn init_event_manager(&mut self) -> Result<(), String> { Ok(()) }
        fn activate_video(&mut self) -> Result<(), String> { Ok(()) }
        fn wait_for_video(&mut self) -> Result<(), String> { Ok(()) }
        fn init_audio(&mut self) -> Result<(), String> { Ok(()) }
        fn init_core_graph(&mut self) -> Result<(), String> { Ok(()) }
        fn init_synth_and_buffers(&mut self) -> Result<(), String> { Ok(()) }
        fn init_loop_and_scene_browsers(&mut self) -> Result<(), String> { Ok(()) }
        fn init_input_and_midi(&mut self) -> Result<(), String> { Ok(()) }
        fn init_osc_and_mixer(&mut self) -> Result<(), String> { Ok(()) }
        fn link_system_variables(&mut self) -> Result<(), String> { Ok(()) }
        fn activate_signal_processing(&mut self) -> Result<(), String> { Ok(()) }
        fn init_streamers_and_finalize_rings(&mut self) -> Result<(), String> { Ok(()) }
        fn add_processing_elements(&mut self) -> Result<(), String> { Ok(()) }
        fn rollback_setup(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn commit_setup(&mut self) {}
    }

    /// Startup double whose `init_sdl` fails and which counts rollbacks.
    struct FailingStartup {
        rollbacks: std::rc::Rc<std::cell::Cell<u32>>,
    }
    macro_rules! startup_ok {
        ($($name:ident),* $(,)?) => {
            $(fn $name(&mut self) -> Result<(), String> {
                Ok(())
            })*
        };
    }
    impl StartupServices for FailingStartup {
        startup_ok!(
            lock_memory,
            init_rt_threads,
            register_main_thread,
            init_platform_threads,
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
            add_processing_elements,
        );
        fn init_sdl(&mut self) -> Result<(), String> {
            Err("no display".into())
        }
        fn rollback_setup(&mut self) -> Result<(), String> {
            self.rollbacks.set(self.rollbacks.get() + 1);
            Ok(())
        }
        fn commit_setup(&mut self) {}
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
    fn a_failed_setup_rolls_back_exactly_once() {
        let rollbacks = std::rc::Rc::new(std::cell::Cell::new(0));
        let services = ApplicationServices::new(
            Config,
            FailingStartup {
                rollbacks: std::rc::Rc::clone(&rollbacks),
            },
            TestComponents::new(),
            0,
            0,
        );
        let mut core = Core::new(services);
        let error = core.setup().unwrap_err();
        assert!(error.contains("init_sdl"), "{error}");
        // `core_startup::setup` rolled the failed phase back; `Core::setup`'s
        // own hook must not replay the stack a second time.
        assert_eq!(rollbacks.get(), 1);
    }

    #[test]
    fn a_failing_run_still_shuts_down() {
        struct FailingComponents(TestComponents);
        impl Components for FailingComponents {
            fn start_session(&mut self) -> Result<(), String> {
                Err("no session".into())
            }
            fn start_interfaces(&mut self) -> Result<(), String> {
                self.0.start_interfaces()
            }
            fn next_event(&mut self) -> Result<Option<CoreEvent>, String> {
                self.0.next_event()
            }
            fn set_streaming(&mut self, enabled: bool, sequence: u64) -> Result<(), String> {
                self.0.set_streaming(enabled, sequence)
            }
            fn stream_state(&self) -> StreamState {
                self.0.stream_state()
            }
            fn stream_bytes(&self) -> u64 {
                self.0.stream_bytes()
            }
            fn close_video(&mut self) {
                self.0.close_video()
            }
            fn close_sdl(&mut self) {
                self.0.close_sdl()
            }
            fn close_midi(&mut self) {
                self.0.close_midi()
            }
            fn close_audio(&mut self) {
                self.0.close_audio()
            }
            fn shutdown(&mut self) {
                self.0.shutdown()
            }
            fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
                self.0.snapshot_loops()
            }
            fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String> {
                self.0.restore_snapshot(snapshot)
            }
        }

        let services = ApplicationServices::new(
            Config,
            Startup,
            FailingComponents(TestComponents::new()),
            0,
            0,
        );
        let mut core = Core::new(services);
        core.setup().unwrap();
        assert_eq!(core.go().unwrap_err(), "no session");
        // The failed run must still release the components it may have opened.
        assert_eq!(core.services().components().0.closes, 1);
        assert!(!core.is_running());
    }

    #[test]
    fn quit_is_a_real_core_exit_event_and_other_input_is_ignored() {
        assert_eq!(
            core_event(InputEvent::Quit),
            CoreEventAction::Handle(CoreEvent::ExitSession)
        );
        // Any other platform input must not look like "the session ended".
        assert_eq!(
            core_event(InputEvent::Key {
                keysym: 32,
                down: true,
                unicode: 32,
            }),
            CoreEventAction::Ignore
        );
    }
}
