//! Core startup orchestration translated from `fweelin_core_startup.cc`.
//!
//! The concrete config, GUI, audio, MIDI, and RT-thread implementations are
//! still in the C++ application.  `StartupServices` is therefore an explicit
//! compatibility boundary: an application must supply every operation, and a
//! failed operation is propagated after rollback.  No platform operation is
//! silently skipped.

pub const BROWSE_LOOP: i32 = 0;
pub const BROWSE_SCENE: i32 = 1;
pub const BROWSE_LOOP_TRAY: i32 = 2;
pub const BROWSE_SCENE_TRAY: i32 = 3;
pub const BROWSE_PATCH: i32 = 4;

pub const STARTUP_SYSTEM_VARIABLES: &[&str] = &[
    "SYSTEM_midi_transpose",
    "SYSTEM_master_in_volume",
    "SYSTEM_master_out_volume",
    "SYSTEM_cur_pitchbend",
    "SYSTEM_bender_tune",
    "SYSTEM_cur_limiter_gain",
    "SYSTEM_audio_cpu_load",
    "SYSTEM_sync_active",
    "SYSTEM_sync_transmit",
    "SYSTEM_midisync_transmit",
    "SYSTEM_fluidsynth_enabled",
    "SYSTEM_num_midi_outs",
    "SYSTEM_num_help_pages",
    "SYSTEM_num_loops_in_map",
    "SYSTEM_num_recording_loops_in_map",
    "SYSTEM_num_patchbanks",
    "SYSTEM_cur_patchbank_tag",
    "SYSTEM_num_switchable_interfaces",
    "SYSTEM_cur_switchable_interface",
    "SYSTEM_snapshot_page_firstidx",
];

/// Names which the C++ startup links to live runtime state.
///
/// This list has the same *contents* as [`STARTUP_SYSTEM_VARIABLES`] but the
/// order carries no meaning: it is a refresh inventory, and consumers look
/// names up rather than pairing the two lists positionally. A test asserts
/// that both lists hold the same set.
pub const LIVE_SYSTEM_VARIABLES: &[&str] = &[
    "SYSTEM_num_midi_outs",
    "SYSTEM_midi_transpose",
    "SYSTEM_master_in_volume",
    "SYSTEM_master_out_volume",
    "SYSTEM_cur_pitchbend",
    "SYSTEM_bender_tune",
    "SYSTEM_audio_cpu_load",
    "SYSTEM_sync_active",
    "SYSTEM_sync_transmit",
    "SYSTEM_midisync_transmit",
    "SYSTEM_fluidsynth_enabled",
    "SYSTEM_num_help_pages",
    "SYSTEM_num_loops_in_map",
    "SYSTEM_num_recording_loops_in_map",
    "SYSTEM_num_patchbanks",
    "SYSTEM_cur_patchbank_tag",
    "SYSTEM_num_switchable_interfaces",
    "SYSTEM_cur_switchable_interface",
    "SYSTEM_snapshot_page_firstidx",
    "SYSTEM_cur_limiter_gain",
];

/// The part of FloConfig needed before the rest of the application exists.
pub trait StartupConfig {
    fn add_int_constant(&mut self, name: &str, value: i32);
    fn add_empty_variable(&mut self, name: &str);
    fn parse(&mut self) -> Result<(), String>;
    fn start(&mut self) -> Result<(), String>;

    /// Refresh values backed by runtime objects after those objects have been
    /// linked. Implementations may leave this as a no-op when their runtime
    /// updates variables continuously.
    fn refresh_system_variables(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// Compatibility interface for services not yet migrated to Rust.
pub trait StartupServices {
    fn lock_memory(&mut self) -> Result<(), String>;
    fn init_rt_threads(&mut self) -> Result<(), String>;
    fn register_main_thread(&mut self) -> Result<(), String>;
    fn init_platform_threads(&mut self) -> Result<(), String>;
    fn init_sdl(&mut self) -> Result<(), String>;
    fn init_memory_manager(&mut self) -> Result<(), String>;
    fn init_event_manager(&mut self) -> Result<(), String>;
    fn activate_video(&mut self) -> Result<(), String>;
    fn wait_for_video(&mut self) -> Result<(), String>;
    fn init_audio(&mut self) -> Result<(), String>;
    fn init_core_graph(&mut self) -> Result<(), String>;
    fn init_synth_and_buffers(&mut self) -> Result<(), String>;
    fn init_loop_and_scene_browsers(&mut self) -> Result<(), String>;
    fn init_input_and_midi(&mut self) -> Result<(), String>;
    fn init_osc_and_mixer(&mut self) -> Result<(), String>;
    fn link_system_variables(&mut self) -> Result<(), String>;
    fn activate_signal_processing(&mut self) -> Result<(), String>;
    fn init_streamers_and_finalize_rings(&mut self) -> Result<(), String>;
    fn add_processing_elements(&mut self) -> Result<(), String>;
    /// Undo a partially completed startup, leaving the adapter clean enough
    /// for a retry.
    fn rollback_setup(&mut self) -> Result<(), String>;

    /// Commit a completely initialized graph.  C++ calls
    /// `FweelinStartupGuard::Release()` at this point, so the setup-only
    /// rollback stack must not be replayed during ordinary application
    /// shutdown (which has its own ordered cleanup path).
    ///
    /// Required rather than defaulted: an implementation that keeps a rollback
    /// stack must clear (or otherwise retire) it here, and the compiler cannot
    /// tell the difference for a defaulted no-op body.
    fn commit_setup(&mut self);

    /// Give the native graph a deterministic point at which to publish its
    /// current values to the configuration system.
    fn refresh_system_variables(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct StartupError {
    pub phase: &'static str,
    pub message: String,
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "startup {}: {}", self.phase, self.message)
    }
}

impl std::error::Error for StartupError {}

fn fail(phase: &'static str, message: String) -> StartupError {
    StartupError { phase, message }
}

/// Register variables in the same order and with the same names as C++.
pub fn install_startup_variables<C: StartupConfig>(
    cfg: &mut C,
    inputs: usize,
    last_records: usize,
) {
    for (name, value) in [
        ("BROWSE_loop", BROWSE_LOOP),
        ("BROWSE_scene", BROWSE_SCENE),
        ("BROWSE_loop_tray", BROWSE_LOOP_TRAY),
        ("BROWSE_scene_tray", BROWSE_SCENE_TRAY),
        ("BROWSE_patch", BROWSE_PATCH),
    ] {
        cfg.add_int_constant(name, value);
    }
    for name in STARTUP_SYSTEM_VARIABLES {
        cfg.add_empty_variable(name);
    }
    for i in 0..inputs {
        let n = i + 1;
        cfg.add_empty_variable(&format!("SYSTEM_in_{n}_volume"));
        cfg.add_empty_variable(&format!("SYSTEM_in_{n}_peak"));
        cfg.add_empty_variable(&format!("SYSTEM_in_{n}_record"));
    }
    for i in 0..last_records {
        cfg.add_empty_variable(&format!("SYSTEM_loopid_lastrecord_{i}"));
    }
}

/// Execute startup in the C++ order.  Rollback is attempted exactly once for
/// every failure after service initialization begins.
pub fn setup<C: StartupConfig, S: StartupServices>(
    cfg: &mut C,
    services: &mut S,
    inputs: usize,
    last_records: usize,
) -> Result<(), StartupError> {
    // The receiver is passed in explicitly: the macro must not depend on the
    // names in scope at its (only) expansion site.
    macro_rules! step {
        ($services:expr, $name:literal, $expr:expr) => {
            match $expr {
                Ok(value) => value,
                Err(error) => {
                    return Err(match $services.rollback_setup() {
                        Ok(()) => fail($name, error),
                        Err(rollback) => {
                            fail($name, format!("{error}; rollback failed: {rollback}"))
                        }
                    });
                }
            }
        };
    }
    step!(services, "lock_memory", services.lock_memory());
    step!(services, "init_rt_threads", services.init_rt_threads());
    step!(
        services,
        "register_main_thread",
        services.register_main_thread()
    );
    step!(
        services,
        "init_platform_threads",
        services.init_platform_threads()
    );
    step!(services, "init_sdl", services.init_sdl());
    step!(
        services,
        "init_memory_manager",
        services.init_memory_manager()
    );
    install_startup_variables(cfg, inputs, last_records);
    step!(services, "parse_config", cfg.parse());
    step!(
        services,
        "init_event_manager",
        services.init_event_manager()
    );
    step!(services, "activate_video", services.activate_video());
    step!(services, "wait_for_video", services.wait_for_video());
    step!(services, "init_audio", services.init_audio());
    step!(services, "init_core_graph", services.init_core_graph());
    step!(
        services,
        "init_synth_and_buffers",
        services.init_synth_and_buffers()
    );
    step!(
        services,
        "init_loop_and_scene_browsers",
        services.init_loop_and_scene_browsers()
    );
    step!(
        services,
        "init_input_and_midi",
        services.init_input_and_midi()
    );
    step!(
        services,
        "init_osc_and_mixer",
        services.init_osc_and_mixer()
    );
    step!(
        services,
        "link_system_variables",
        services.link_system_variables()
    );
    step!(
        services,
        "refresh_system_variables",
        services.refresh_system_variables()
    );
    step!(
        services,
        "refresh_config_system_variables",
        cfg.refresh_system_variables()
    );
    step!(services, "start_config", cfg.start());
    step!(
        services,
        "activate_signal_processing",
        services.activate_signal_processing()
    );
    step!(
        services,
        "init_streamers_and_finalize_rings",
        services.init_streamers_and_finalize_rings()
    );
    step!(
        services,
        "add_processing_elements",
        services.add_processing_elements()
    );
    services.commit_setup();
    Ok(())
}
