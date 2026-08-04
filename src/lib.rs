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
    android_init_ndk_context();
    #[cfg(target_os = "android")]
    {
        // The Java activity shows the RECORD_AUDIO runtime-permission dialog
        // before SDL starts. AAudio refuses to open the capture stream while
        // the permission is pending or denied, which used to kill the whole
        // app at audio activation; wait for the user's decision first.
        if !android_wait_for_record_permission() {
            eprintln!("FreeWheeling: RECORD_AUDIO denied; running without microphone input");
        }
    }
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
                android_log_error(&format!("run: {error}"));
                1
            }
        },
        Err(error) => {
            eprintln!("Error starting FreeWheeling: {error}");
            android_log_error(&format!("start: {error}"));
            1
        }
    }
}

/// Registers the JavaVM and activity with the `ndk-context` crate, which
/// midir's Android MIDI backend requires (it panics when the context was
/// never initialized). SDL exposes both pointers after its Java glue calls
/// `nativeSetupJNI`, which happens before `SDL_main` runs.
#[cfg(target_os = "android")]
fn android_init_ndk_context() {
    use jni_sys::{JNIEnv, JavaVM, JNINativeInterface_, jint};
    use std::ffi::c_void;
    unsafe extern "C" {
        fn SDL_AndroidGetJNIEnv() -> *mut c_void;
        fn SDL_AndroidGetActivity() -> *mut c_void;
    }
    // SAFETY: SDL's Java glue has already been set up by the time SDL_main
    // runs, so the JNI env and activity handles are valid, and
    // initialize_android_context is called exactly once.
    unsafe {
        let env: *const JNIEnv = SDL_AndroidGetJNIEnv().cast();
        let activity = SDL_AndroidGetActivity();
        let get_vm: Option<unsafe extern "system" fn(*mut JNIEnv, *mut *mut JavaVM) -> jint> = env
            .as_ref()
            .and_then(|env_ref| (*env_ref).as_ref())
            .and_then(|f: &JNINativeInterface_| Some(f.v1_1.GetJavaVM));
        if let (Some(get_vm), false) = (get_vm, activity.is_null()) {
            let mut vm: *mut JavaVM = std::ptr::null_mut();
            get_vm(env.cast_mut(), &mut vm);
            ndk_context::initialize_android_context(vm.cast(), activity);
        } else {
            eprintln!("FreeWheeling: SDL JNI context unavailable; MIDI will be disabled");
        }
    }
}

/// Read a public static `int` field from `FreeWheelingActivity` via JNI.
/// Returns `None` when the JNI glue is unavailable or the field is missing.
#[cfg(target_os = "android")]
fn android_read_static_int_field(field_name: &str) -> Option<i32> {
    use jni_sys::{jfieldID, jint, JNIEnv};
    use std::ffi::CString;
    unsafe extern "C" {
        fn SDL_AndroidGetJNIEnv() -> *mut std::ffi::c_void;
        fn SDL_AndroidGetActivity() -> *mut std::ffi::c_void;
    }
    unsafe {
        let env: *const JNIEnv = SDL_AndroidGetJNIEnv().cast();
        let activity = SDL_AndroidGetActivity();
        let env_ref = env.as_ref().and_then(|env| env.as_ref())?;
        let get_object_class = (*env_ref).v1_1.GetObjectClass;
        let class = get_object_class(env.cast_mut(), activity.cast::<jni_sys::_jobject>());
        if class.is_null() {
            return None;
        }
        let name = CString::new(field_name).ok()?;
        let sig = CString::new("I").ok()?;
        let get_static_field_id = (*env_ref).v1_1.GetStaticFieldID;
        let field: jfieldID =
            get_static_field_id(env.cast_mut(), class, name.as_ptr(), sig.as_ptr());
        if field.is_null() {
            return None;
        }
        let get_static_int_field = (*env_ref).v1_1.GetStaticIntField;
        Some(get_static_int_field(env.cast_mut(), class, field) as i32)
    }
}

/// Whether the activity holds Android's special "all files access" grant
/// (MANAGE_EXTERNAL_STORAGE), which lets stream recordings be written to the
/// shared Documents folder. Falls back to app-internal storage otherwise.
#[cfg(target_os = "android")]
pub fn android_external_storage_granted() -> bool {
    android_read_static_int_field("sExternalStorageGranted") == Some(1)
}

#[cfg(not(target_os = "android"))]
pub fn android_external_storage_granted() -> bool {
    false
}

/// Blocks until FreeWheelingActivity has resolved the RECORD_AUDIO runtime
/// permission (or 30 s elapse), then reports whether it was granted. AAudio
/// cannot open the capture stream while the permission is pending or denied,
/// so SDL_main waits for the user's dialog decision before starting audio.
/// Returns `true` on any JNI problem so a broken glue never hangs startup.
#[cfg(target_os = "android")]
fn android_wait_for_record_permission() -> bool {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match android_read_static_int_field("sRecordAudioResult") {
            Some(value) if value != 0 => return value == 1,
            Some(_) => {}
            None => return true, // JNI unavailable: do not block startup
        }
        if Instant::now() >= deadline {
            eprintln!("FreeWheeling: timed out waiting for RECORD_AUDIO permission");
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Android has no visible console; persist startup failures where the user
/// (or a debugging session) can read them back.
#[cfg(target_os = "android")]
fn android_log_error(message: &str) {
    let path =
        std::path::Path::new("/data/data/org.freewheeling.freewheeling_plus/files/startup-error.log");
    let _ = std::fs::write(path, format!("{message}\n"));
}

/// Android has no visible console; append runtime diagnostics (window/drawable
/// sizes, touch mapping state) to a file a debugging session can read back.
#[cfg(target_os = "android")]
pub fn android_diag_log(message: &str) {
    use std::io::Write;
    let path =
        std::path::Path::new("/data/data/org.freewheeling.freewheeling_plus/files/video-diag.log");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

#[cfg(not(target_os = "android"))]
pub fn android_diag_log(_message: &str) {}
