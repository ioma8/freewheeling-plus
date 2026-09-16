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
    {
        // The Java activity extracts the bundled data/ assets on a worker
        // thread before the native side may read them.
        if !android_wait_for_data_assets() {
            eprintln!(
                "FreeWheeling: bundled data assets are unavailable; the application will start \
                 without its configuration"
            );
        }
        // The Java activity shows the RECORD_AUDIO runtime-permission dialog
        // before SDL starts. AAudio refuses to open the capture stream while
        // the permission is pending or denied, which used to kill the whole
        // app at audio activation; wait for the user's decision first.
        match android_wait_for_record_permission() {
            RecordPermission::Granted => {}
            RecordPermission::Denied => {
                eprintln!("FreeWheeling: RECORD_AUDIO denied; running without microphone input")
            }
            RecordPermission::TimedOut => eprintln!(
                "FreeWheeling: no RECORD_AUDIO answer; running without microphone input"
            ),
            RecordPermission::Unknown => eprintln!(
                "FreeWheeling: cannot query RECORD_AUDIO; running without microphone input"
            ),
        }
    }
    // Only the program name is used here; collecting every argument would
    // allocate the whole argv for nothing.
    let program = std::env::args_os()
        .next()
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
        let env_ref = env.as_ref().and_then(|env| env.as_ref());
        let get_vm = env_ref.map(|env| env.v1_1.GetJavaVM);
        if let (Some(get_vm), false) = (get_vm, activity.is_null()) {
            let mut vm: *mut JavaVM = std::ptr::null_mut();
            get_vm(env.cast_mut(), &mut vm);
            // SAFETY: the context is kept for the whole process and read from
            // other threads (midir's Android backend), but
            // `SDL_AndroidGetActivity` returns a *local* reference that dies
            // with this JNI frame. Promote it to a global reference, then drop
            // the local one.
            let global = match env_ref {
                Some(env_ref) => env_ref.v1_1.NewGlobalRef(env.cast_mut(), activity),
                None => std::ptr::null_mut(),
            };
            env_ref.map(|env_ref| env_ref.v1_1.DeleteLocalRef(env.cast_mut(), activity));
            if global.is_null() {
                eprintln!("FreeWheeling: SDL JNI context unavailable; MIDI will be disabled");
            } else {
                ndk_context::initialize_android_context(vm.cast(), global);
            }
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
        if activity.is_null() {
            // No activity yet: `GetObjectClass(null)` is undefined.
            return None;
        }
        let get_object_class = (*env_ref).v1_1.GetObjectClass;
        let class = get_object_class(env.cast_mut(), activity.cast::<jni_sys::_jobject>());
        // Local references are released before every return: this function is
        // polled every 100 ms while waiting for the permission dialog, and the
        // per-thread local reference table holds only 512 entries.
        let _release = LocalRefs {
            env: env.cast_mut(),
            env_ref,
            activity,
            class,
        };
        if class.is_null() {
            return None;
        }
        let name = CString::new(field_name).ok()?;
        let sig = CString::new("I").ok()?;
        let get_static_field_id = (*env_ref).v1_1.GetStaticFieldID;
        let field: jfieldID =
            get_static_field_id(env.cast_mut(), class, name.as_ptr(), sig.as_ptr());
        if field.is_null() {
            // The Java side may not have published the field yet, which raises
            // a pending `NoSuchFieldError`: clear it, or the next JNI call on
            // this thread reports it instead.
            if (*env_ref).v1_1.ExceptionCheck(env.cast_mut()) != 0 {
                (*env_ref).v1_1.ExceptionClear(env.cast_mut());
            }
            return None;
        }
        let get_static_int_field = (*env_ref).v1_1.GetStaticIntField;
        Some(get_static_int_field(env.cast_mut(), class, field) as i32)
    }
}

/// Releases the JNI local references of one [`android_read_static_int_field`]
/// call, on every return path.
#[cfg(target_os = "android")]
struct LocalRefs {
    env: *mut jni_sys::JNIEnv,
    env_ref: &'static jni_sys::JNINativeInterface_,
    activity: *mut std::ffi::c_void,
    class: jni_sys::jclass,
}

#[cfg(target_os = "android")]
impl Drop for LocalRefs {
    fn drop(&mut self) {
        unsafe {
            (self.env_ref.v1_1.DeleteLocalRef)(self.env, self.class);
            (self.env_ref.v1_1.DeleteLocalRef)(self.env, self.activity.cast());
        }
    }
}

/// Wait for FreeWheelingActivity's asset extraction (or 30 s).
///
/// The activity publishes `sDataExtracted` (0 pending, 1 done, 2 failed) and
/// copies `assets/data` to `files/data` on a worker thread; the native side
/// must not read the configuration before that finishes.
/// Returns `false` when extraction failed or the flag never appeared.
#[cfg(target_os = "android")]
fn android_wait_for_data_assets() -> bool {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match android_read_static_int_field("sDataExtracted") {
            Some(1) => return true,
            Some(2) => {
                eprintln!("FreeWheeling: asset extraction failed (see logcat)");
                return false;
            }
            Some(_) => {}
            None => return true, // JNI unavailable: do not block startup
        }
        if Instant::now() >= deadline {
            eprintln!("FreeWheeling: timed out waiting for the bundled data assets");
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
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

/// Result of waiting for the RECORD_AUDIO runtime permission.
///
/// A timeout and a missing JNI bridge are not the same as a denial, and
/// reporting them as one made the startup message misleading.
#[cfg(target_os = "android")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordPermission {
    Granted,
    Denied,
    /// The user never answered within the wait window.
    TimedOut,
    /// The Java side is not reachable, so the permission state is unknown.
    Unknown,
}

/// Blocks until FreeWheelingActivity has resolved the RECORD_AUDIO runtime
/// permission (or 30 s elapse). AAudio cannot open the capture stream while
/// the permission is pending or denied, so SDL_main waits for the dialog
/// decision before starting audio.
#[cfg(target_os = "android")]
fn android_wait_for_record_permission() -> RecordPermission {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match android_read_static_int_field("sRecordAudioResult") {
            Some(1) => return RecordPermission::Granted,
            Some(0) => {}
            Some(_) => return RecordPermission::Denied,
            None => return RecordPermission::Unknown,
        }
        if Instant::now() >= deadline {
            eprintln!("FreeWheeling: timed out waiting for RECORD_AUDIO permission");
            return RecordPermission::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Android logs live next to the application's private files.
///
/// The path is built from the package name once so a rename cannot make
/// diagnostics silently disappear.
#[cfg(target_os = "android")]
fn android_log_path(file_name: &str) -> std::path::PathBuf {
    std::path::Path::new("/data/data")
        .join(crate::native_startup::ANDROID_PACKAGE_ID)
        .join("files")
        .join(file_name)
}

/// Android has no visible console; persist startup failures where the user
/// (or a debugging session) can read them back.
#[cfg(target_os = "android")]
fn android_log_error(message: &str) {
    let path = android_log_path("startup-error.log");
    if let Err(error) = std::fs::write(&path, format!("{message}\n")) {
        // Losing the log silently would hide the startup failure it exists for.
        eprintln!("FreeWheeling: cannot write {}: {error}", path.display());
    }
}

/// Android has no visible console; append runtime diagnostics (window/drawable
/// sizes, touch mapping state) to a file a debugging session can read back.
#[cfg(target_os = "android")]
pub fn android_diag_log(message: &str) {
    use std::io::Write;
    let path = android_log_path("video-diag.log");
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            if let Err(error) = writeln!(file, "{message}") {
                eprintln!("FreeWheeling: cannot write {}: {error}", path.display());
            }
        }
        Err(error) => eprintln!("FreeWheeling: cannot open {}: {error}", path.display()),
    }
}

#[cfg(not(target_os = "android"))]
pub fn android_diag_log(_message: &str) {}
