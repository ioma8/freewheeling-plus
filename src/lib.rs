pub mod amixer;
pub mod application_services;
pub mod audio_native_cpal;
pub mod audioio;
pub mod audioio_platform;
pub mod block;
pub mod block_managers;
pub mod browser;
pub mod browser_types;
pub mod config;
pub mod core;
pub mod core_dsp;
pub mod core_dsp_audio_buffers;
pub mod core_persistence;
pub mod core_persistence_parse;
pub mod core_persistence_runtime;
pub mod core_startup;
pub mod datatypes;
pub mod event;
pub mod file_codecs;
pub mod fluidsynth;

#[cfg(all(
    feature = "jack",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub mod jack;
pub mod logo;
pub mod looplibrary;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub mod macos_audio_unit;
pub mod macos_sdlmain;
pub mod mem;
pub mod midiio;
pub mod midiio_platform;
pub mod native_dsp_graph;
pub mod file_streamer;
pub mod native_event_bridge;
pub mod native_loop_selection;
pub mod native_patch_browser;
pub mod native_rename;
pub mod native_startup;
pub mod native_ui_state;
pub mod osc;
pub mod paramset;
pub mod processor_queue;
pub mod production_app;

pub mod realtime_guard;
pub mod realtime_queue;
pub mod runtime_event_actions;
pub mod sdlio;
pub mod sdlkey_compat;
pub mod signal;
pub mod stacktrace;
pub mod string_utils;
pub mod surface_primitives;
pub mod video_layout;
pub mod microui;
pub mod videoio;
pub mod videoio_displays;
pub mod videoio_platform;

/// Android's `NativeActivity` loads the Rust cdylib and SDL2's Java glue
/// calls this symbol to hand control to the application.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "C" fn SDL_main(_argc: i32, _argv: *const *const i8) -> i32 {
    let args: Vec<_> = std::env::args_os().collect();
    let program = args
        .first()
        .cloned()
        .unwrap_or_else(|| std::ffi::OsString::from("freewheeling"));
    let program = program.to_string_lossy();

    stacktrace::stack_trace_init(&program, -1);
    signal::register_fatal_signal_handlers();
    signal::register_shutdown_signal_handlers();
    signal::clear_shutdown_request();

    println!("FreeWheeling {}", env!("CARGO_PKG_VERSION"));
    println!("May we return to the circle.\n");

    match production_app::native_runtime::production_application() {
        Ok(mut app) => match app.run() {
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
