//! FreeWheeling application entrypoint.
//!
//! The migrated core is generic over the platform services which own SDL,
//! audio, MIDI, and video.  This file deliberately keeps that application
//! boundary small so the process lifecycle remains testable independently of
//! those adapters.

use std::ffi::OsString;

#[cfg(feature = "smoke-test")]
use freewheeling_plus::application_services::{ApplicationServices, Components};
use freewheeling_plus::core::{Core, CoreServices};
#[cfg(feature = "smoke-test")]
use freewheeling_plus::core::{CoreEvent, LoopSnapshot, Snapshot, StreamState};
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
pub fn run<A: Application>(argv: &[OsString], app: &mut A) -> i32 {
    initialize_process(argv);
    run_initialized(app)
}

/// Install process-wide diagnostics before any native application object is
/// constructed.  C++ `FweelinAppMain` performs this before constructing its
/// `Fweelin flo` local, so a construction/setup failure is still covered by
/// the fatal and shutdown handlers.
fn initialize_process(argv: &[OsString]) {
    let program = argv
        .first()
        .map(OsString::as_os_str)
        .unwrap_or_else(|| std::ffi::OsStr::new("freewheeling"));
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

fn register_signal_handlers() {
    signal::register_fatal_signal_handlers();
    // Android is Linux-kernel-based and the fatal handlers work there, but
    // SIGUSR1/SIGUSR2 are reserved by the Android runtime (bionic/libc uses
    // them for thread cancellation), so the info handlers stay off.
    #[cfg(not(target_os = "android"))]
    signal::register_info_signal_handlers();
    signal::register_shutdown_signal_handlers();
}


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
        let result = self.core_mut().setup();
        if result.is_err() {
            // Mirror `go`: a partial startup must be rolled back here rather
            // than relying on `Drop` of every adapter.
            self.core_mut().shutdown();
        }
        result
    }

    fn go(&mut self) -> Result<(), String> {
        let result = self.core_mut().go();
        self.core_mut().shutdown();
        result
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    /// Document paths are accepted (a Finder/Launch Services launch passes
    /// them) but not carried further: nothing in the runtime consumes them
    /// yet, and a payload nobody reads would suggest delivery that does not
    /// happen.
    Production,
    Smoke,
}

/// Split the command line into an option phase and document paths.
///
/// Options are only recognised before the first positional argument (or after
/// a `--` separator), so a document named `-weird-name.xml` is delivered
/// instead of being rejected as an unknown option.
fn invocation(args: &[OsString]) -> Result<Invocation, String> {
    // `LaunchArguments` owns its argv (it strips the macOS `-psn_*` argument),
    // so it copies once; the caller's vector is only borrowed.
    let launch = LaunchArguments::from_args(args.iter());
    let mut smoke = false;
    let mut seen_positional = false;
    let mut positional = Vec::new();

    // `launch.args()` is the command line with the macOS `-psn_*` argument
    // removed; the vector itself is owned by `launch`, so nothing is cloned.
    for arg in launch.args().iter().skip(1) {
        if !seen_positional && arg == "--" {
            // Explicit end of options: everything after it is a document.
            seen_positional = true;
        } else if !seen_positional && arg == "--smoke-test" {
            smoke = true;
        } else if !seen_positional && arg.to_string_lossy().starts_with('-') {
            // `{:?}`: a non-UTF-8 argument must not be reported as a mangled
            // string that hides what was passed.
            return Err(format!("Unknown option: {arg:?}"));
        } else {
            seen_positional = true;
            positional.push(arg.clone());
        }
    }

    if smoke && !positional.is_empty() {
        return Err("--smoke-test does not accept document arguments".into());
    }
    Ok(if smoke {
        Invocation::Smoke
    } else {
        Invocation::Production
    })
}

fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    // Process-wide diagnostics must be installed before any native object is
    // constructed, so initialization happens here rather than inside the
    // smoke arm (which used to build the core first).
    initialize_process(&args);
    let code = match invocation(&args) {
        Ok(Invocation::Smoke) => {
            #[cfg(feature = "smoke-test")]
            {
                run_initialized(&mut smoke_application())
            }
            #[cfg(not(feature = "smoke-test"))]
            {
                // Never silently substitute the real core path for a requested
                // smoke test: say that this build has no harness instead.
                eprintln!(
                    "Error: this build has no smoke-test harness; rebuild with --features smoke-test"
                );
                1
            }
        }
        Ok(Invocation::Production) => match production_application() {
            Ok(mut app) => run_initialized(&mut app),
            Err(error) => {
                eprintln!("Error starting FreeWheeling: {error}");
                1
            }
        },
        Err(error) => {
            eprintln!("{error}");
            #[cfg(feature = "smoke-test")]
            eprintln!("Usage: freewheeling-plus [--smoke-test] [document ...]");
            #[cfg(not(feature = "smoke-test"))]
            eprintln!("Usage: freewheeling-plus [document ...]");
            2
        }
    };
    // Return ExitCode instead of calling process::exit so Drop impls run
    // (audio streams, MIDI devices, video backends, memory pools). Only
    // 0/1/2 are produced today; a wider value would silently become 1, so the
    // conversion is explicit about the expectation.
    let code: u8 = code
        .try_into()
        .expect("application status codes are 0..=255");
    std::process::ExitCode::from(code)
}

/// Diagnostic core for `--smoke-test`.
///
/// Every startup phase is a no-op and the core runs no audio, video or input
/// code: the mode verifies that argument handling, process initialization and
/// the application lifecycle work end to end (it is documented in `README.md`
/// and used by `tests/macos_acceptance.rs`), and deliberately does not
/// validate any real backend.
#[cfg(feature = "smoke-test")]
#[derive(Default)]
struct SmokeConfig;
#[cfg(feature = "smoke-test")]
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
#[cfg(feature = "smoke-test")]
struct SmokeStartup;
#[cfg(feature = "smoke-test")]
macro_rules! smoke_startup_methods { ($($name:ident),+ $(,)?) => { $(fn $name(&mut self) -> Result<(), String> { Ok(()) })+ }; }
#[cfg(feature = "smoke-test")]
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
    fn rollback_setup(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn commit_setup(&mut self) {}
}

#[cfg(feature = "smoke-test")]
struct SmokeComponents {
    first_event: bool,
    state: StreamState,
}
#[cfg(feature = "smoke-test")]
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

#[cfg(feature = "smoke-test")]
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

    // These use `run_initialized`: `run` installs process-wide signal handlers
    // and prints the banner, so calling it from a test would pollute the whole
    // test process (and make the suite order-dependent).
    #[test]
    fn failed_setup_does_not_run_application() {
        let mut app = Fake {
            setup: Err("failed".into()),
            go_called: false,
        };
        assert_eq!(run_initialized(&mut app), 1);
        assert!(!app.go_called);
    }

    #[test]
    fn successful_setup_runs_application() {
        let mut app = Fake {
            setup: Ok(()),
            go_called: false,
        };
        assert_eq!(run_initialized(&mut app), 0);
        assert!(app.go_called);
    }

    #[test]
    fn normal_and_finder_document_invocations_select_production() {
        assert_eq!(
            invocation(&[OsString::from("fweelin")]).unwrap(),
            Invocation::Production
        );
        assert_eq!(
            invocation(&[
                OsString::from("fweelin"),
                OsString::from("-psn_0_42"),
                OsString::from("/tmp/session.xml"),
            ])
            .unwrap(),
            Invocation::Production
        );
    }

    #[test]
    fn smoke_and_invalid_options_are_distinguished() {
        assert_eq!(
            invocation(&[OsString::from("fweelin"), OsString::from("--smoke-test")]).unwrap(),
            Invocation::Smoke
        );
        assert_eq!(
            invocation(&[OsString::from("fweelin"), OsString::from("--wat")]).unwrap_err(),
            "Unknown option: \"--wat\""
        );
    }

    #[test]
    fn options_end_at_the_first_document_path() {
        // A document whose name starts with '-' is delivered once the option
        // phase has ended, either by a path prefix or by `--`.
        assert_eq!(
            invocation(&[
                OsString::from("fweelin"),
                OsString::from("./-session.xml"),
            ])
            .unwrap(),
            Invocation::Production
        );
        assert_eq!(
            invocation(&[
                OsString::from("fweelin"),
                OsString::from("--"),
                OsString::from("-weird-name.xml"),
            ])
            .unwrap(),
            Invocation::Production
        );
        // A `--` separator also un-arms option parsing.
        assert_eq!(
            invocation(&[
                OsString::from("fweelin"),
                OsString::from("--"),
                OsString::from("--smoke-test"),
            ])
            .unwrap(),
            Invocation::Production
        );
        // Options before the first positional are still validated.
        assert!(invocation(&[OsString::from("fweelin"), OsString::from("-x")]).is_err());
    }
}
