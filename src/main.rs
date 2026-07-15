//! FreeWheeling application entrypoint.
//!
//! The migrated core is generic over the platform services which own SDL,
//! audio, MIDI, and video.  This file deliberately keeps that application
//! boundary small so the process lifecycle remains testable independently of
//! those adapters.

use std::ffi::OsString;

use freewheeling_plus::application_services::{ApplicationServices, Components};
use freewheeling_plus::core::{Core, CoreEvent, CoreServices, LoopSnapshot, Snapshot, StreamState};
use freewheeling_plus::core_startup::{StartupConfig, StartupServices};
use freewheeling_plus::macos_sdlmain::LaunchArguments;
use freewheeling_plus::production_app::native_runtime::production_application;
use freewheeling_plus::production_app::{NativeComponentAdapter, ProductionApp};
use freewheeling_plus::{signal, stacktrace};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The part of the application lifecycle owned by this entrypoint.
pub trait Application {
    fn setup(&mut self) -> Result<(), String>;
    fn go(&mut self) -> Result<(), String>;
}

/// Run the process lifecycle, preserving the historical startup messages and
/// setup-before-run behavior.  `argv` is accepted because the C entrypoint
/// received it, and the program name is used by stack trace initialization.
pub fn run<A: Application>(argv: impl IntoIterator<Item = OsString>, app: &mut A) -> i32 {
    initialize_process(argv);
    run_initialized(app)
}

/// Install process-wide diagnostics before any native application object is
/// constructed.  C++ `FweelinAppMain` performs this before constructing its
/// `Fweelin flo` local, so a construction/setup failure is still covered by
/// the fatal and shutdown handlers.
fn initialize_process(argv: impl IntoIterator<Item = OsString>) {
    let mut argv = argv.into_iter();
    let program = argv
        .next()
        .unwrap_or_else(|| OsString::from("freewheeling"));
    let program = program.to_string_lossy();

    stacktrace::stack_trace_init(&program, -1);
    register_signal_handlers();
    signal::clear_shutdown_request();
}

fn run_initialized<A: Application>(app: &mut A) -> i32 {
    println!("FreeWheeling {VERSION}");
    println!("May we return to the circle.\n");

    match app.setup() {
        Ok(()) => match app.go() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("Error running FreeWheeling: {error}");
                1
            }
        },
        Err(error) => {
            eprintln!("Error starting FreeWheeling: {error}");
            1
        }
    }
}

#[cfg(not(windows))]
fn register_signal_handlers() {
    signal::register_fatal_signal_handlers();
    signal::register_info_signal_handlers();
    signal::register_shutdown_signal_handlers();
}

#[cfg(windows)]
fn register_signal_handlers() {}

impl<S: CoreServices> Application for Core<S> {
    fn setup(&mut self) -> Result<(), String> {
        Core::setup(self)
    }
    fn go(&mut self) -> Result<(), String> {
        Core::go(self)
    }
}

impl<C, S, A> Application for ProductionApp<C, S, A>
where
    C: StartupConfig,
    S: StartupServices,
    A: NativeComponentAdapter,
{
    fn setup(&mut self) -> Result<(), String> {
        self.app_mut().setup()
    }

    fn go(&mut self) -> Result<(), String> {
        let result = self.app_mut().go();
        self.app_mut().shutdown();
        result
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    Production(Vec<OsString>),
    Smoke(Vec<OsString>),
}

fn invocation(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, String> {
    let launch = LaunchArguments::from_args(args);
    let args = launch.args().to_vec();
    let mut smoke = false;

    for arg in args.iter().skip(1) {
        if arg == "--smoke-test" {
            smoke = true;
        } else if arg.to_string_lossy().starts_with('-') {
            return Err(format!("Unknown option: {}", arg.to_string_lossy()));
        }
    }

    if smoke && args.len() != 2 {
        return Err("--smoke-test does not accept document arguments".into());
    }
    Ok(if smoke {
        Invocation::Smoke(args)
    } else {
        // Positional paths are retained for Finder/Launch Services document
        // delivery even though document loading is owned by the runtime.
        Invocation::Production(args)
    })
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let code = match invocation(args.clone()) {
        Ok(Invocation::Smoke(args)) => run(args, &mut smoke_application()),
        Ok(Invocation::Production(_)) => {
            initialize_process(args);
            match production_application() {
                Ok(mut app) => run_initialized(&mut app),
                Err(error) => {
                    eprintln!("Error starting FreeWheeling: {error}");
                    1
                }
            }
        }
        Err(error) => {
            eprintln!("{error}");
            eprintln!("Usage: freewheeling-plus [--smoke-test] [document ...]");
            2
        }
    };
    std::process::exit(code);
}

#[derive(Default)]
struct SmokeConfig;
impl StartupConfig for SmokeConfig {
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
struct SmokeStartup;
macro_rules! smoke_startup_methods { ($($name:ident),+ $(,)?) => { $(fn $name(&mut self) -> Result<(), String> { Ok(()) })+ }; }
impl StartupServices for SmokeStartup {
    smoke_startup_methods!(
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

struct SmokeComponents {
    first_event: bool,
    state: StreamState,
}
impl Components for SmokeComponents {
    fn start_session(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn start_interfaces(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn next_event(&mut self) -> Result<Option<CoreEvent>, String> {
        Ok(self.first_event.then(|| {
            self.first_event = false;
            CoreEvent::ExitSession
        }))
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
        0
    }
    fn close_video(&mut self) {}
    fn close_sdl(&mut self) {}
    fn close_midi(&mut self) {}
    fn close_audio(&mut self) {}
    fn shutdown(&mut self) {}
    fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
        Vec::new()
    }
    fn restore_snapshot(&mut self, _: &Snapshot) -> Result<(), String> {
        Ok(())
    }
}

fn smoke_application() -> Core<ApplicationServices<SmokeConfig, SmokeStartup, SmokeComponents>> {
    Core::new(ApplicationServices::new(
        SmokeConfig,
        SmokeStartup,
        SmokeComponents {
            first_event: true,
            state: StreamState::Stopped,
        },
        0,
        0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        setup: Result<(), String>,
        go_called: bool,
    }

    impl Application for Fake {
        fn setup(&mut self) -> Result<(), String> {
            self.setup.clone()
        }

        fn go(&mut self) -> Result<(), String> {
            self.go_called = true;
            Ok(())
        }
    }

    #[test]
    fn failed_setup_does_not_run_application() {
        let mut app = Fake {
            setup: Err("failed".into()),
            go_called: false,
        };
        assert_eq!(run([OsString::from("test")], &mut app), 1);
        assert!(!app.go_called);
    }

    #[test]
    fn successful_setup_runs_application() {
        let mut app = Fake {
            setup: Ok(()),
            go_called: false,
        };
        assert_eq!(run([OsString::from("test")], &mut app), 0);
        assert!(app.go_called);
    }

    #[test]
    fn normal_and_finder_document_invocations_select_production() {
        assert_eq!(
            invocation([OsString::from("fweelin")]).unwrap(),
            Invocation::Production(vec![OsString::from("fweelin")])
        );
        assert_eq!(
            invocation([
                OsString::from("fweelin"),
                OsString::from("-psn_0_42"),
                OsString::from("/tmp/session.xml"),
            ])
            .unwrap(),
            Invocation::Production(vec![
                OsString::from("fweelin"),
                OsString::from("/tmp/session.xml"),
            ])
        );
    }

    #[test]
    fn smoke_and_invalid_options_are_distinguished() {
        assert!(matches!(
            invocation([OsString::from("fweelin"), OsString::from("--smoke-test")]),
            Ok(Invocation::Smoke(_))
        ));
        assert_eq!(
            invocation([OsString::from("fweelin"), OsString::from("--wat")]).unwrap_err(),
            "Unknown option: --wat"
        );
    }
}
