//! Native resource discovery and transactional startup orchestration.
//!
//! Concrete platform backends implement [`NativeStartupAdapter`]. This module
//! owns ordering and rollback, keeping partially-created native resources out
//! of the top-level application.

use crate::core_startup::StartupServices;
use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_FILE: &str = "fweelin.xml";
pub const DATA_DIR_ENV: &str = "FWEELIN_DATADIR";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePaths {
    pub resources: PathBuf,
    pub application_support: PathBuf,
    pub config: PathBuf,
}

impl NativePaths {
    /// Resolve bundle/data resources and create the writable per-user root.
    pub fn discover(executable: &Path, home: &Path) -> Result<Self, String> {
        let resources = discover_resources(executable)?;
        let application_support = application_support_path(home);
        fs::create_dir_all(&application_support).map_err(|error| {
            format!(
                "create application support directory {}: {error}",
                application_support.display()
            )
        })?;
        let user_config = application_support.join(DEFAULT_CONFIG_FILE);
        let config = if user_config.is_file() {
            user_config
        } else {
            resources.join(DEFAULT_CONFIG_FILE)
        };
        if !config.is_file() {
            return Err(format!(
                "configuration file not found: {}",
                config.display()
            ));
        }
        Ok(Self {
            resources,
            application_support,
            config,
        })
    }

    pub fn asset(&self, relative: impl AsRef<Path>) -> Result<PathBuf, String> {
        let relative = relative.as_ref();
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(format!(
                "asset path must be relative: {}",
                relative.display()
            ));
        }
        let path = self.resources.join(relative);
        path.is_file()
            .then_some(path.clone())
            .ok_or_else(|| format!("resource not found: {}", path.display()))
    }
}

pub fn application_support_path(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library")
            .join("Application Support")
            .join("Fweelin")
    }
    #[cfg(target_os = "android")]
    {
        // Android internal storage: /data/data/<package>/
        // Matches Cargo.toml [package.metadata.bundle].identifier on Android.
        // SDL2's SDL_GetPrefPath is also available after init.
        Path::new("/data/data/org.freewheeling.freewheeling-plus/files/.fweelin").to_path_buf()
    }
    #[cfg(not(any(target_os = "macos", target_os = "android")))]
    home.join(".fweelin")
}

/// Locate bundle resources first, then an explicit data directory, then
/// development/install layouts adjacent to the executable.
pub fn discover_resources(executable: &Path) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Some(mac_os) = executable.parent() {
        candidates.push(mac_os.join("../Resources"));
        candidates.push(mac_os.join("data"));
        if let Some(parent) = mac_os.parent() {
            candidates.push(parent.join("data"));
        }
    }
    if let Some(path) = env::var_os(DATA_DIR_ENV) {
        candidates.insert(0, PathBuf::from(path));
    }
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("data"));
        candidates.push(cwd);
    }
    for candidate in candidates {
        let normalized = candidate.canonicalize().unwrap_or(candidate);
        if normalized.join(DEFAULT_CONFIG_FILE).is_file() {
            return Ok(normalized);
        }
    }
    Err(format!(
        "could not locate {DEFAULT_CONFIG_FILE}; set {DATA_DIR_ENV} or install bundle resources"
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupPhase {
    LockMemory,
    RtThreads,
    MainThread,
    PlatformThreads,
    Sdl,
    MemoryManager,
    EventManager,
    Video,
    VideoReady,
    Audio,
    CoreGraph,
    SynthAndBuffers,
    Browsers,
    InputAndMidi,
    OscAndMixer,
    SystemVariables,
    SignalProcessing,
    StreamersAndRings,
    ProcessingElements,
}

impl fmt::Display for StartupPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Boundary implemented by the assembled native graph. `rollback` must be
/// safe after a successful `start` for the same phase.
pub trait NativeStartupAdapter {
    fn start(&mut self, phase: StartupPhase, paths: &NativePaths) -> Result<(), String>;
    fn rollback(&mut self, phase: StartupPhase);
}

pub struct NativeStartupServices<A: NativeStartupAdapter> {
    paths: NativePaths,
    adapter: A,
    completed: Vec<StartupPhase>,
}

impl<A: NativeStartupAdapter> NativeStartupServices<A> {
    pub fn new(paths: NativePaths, adapter: A) -> Self {
        Self {
            paths,
            adapter,
            completed: Vec::with_capacity(19),
        }
    }

    pub fn paths(&self) -> &NativePaths {
        &self.paths
    }
    pub fn adapter(&self) -> &A {
        &self.adapter
    }
    pub fn adapter_mut(&mut self) -> &mut A {
        &mut self.adapter
    }
}

impl<A: NativeStartupAdapter> NativeStartupServices<A> {
    fn start_phase(&mut self, phase: StartupPhase) -> Result<(), String> {
        self.adapter
            .start(phase, &self.paths)
            .map_err(|error| format!("{phase}: {error}"))?;
        self.completed.push(phase);
        Ok(())
    }
}

macro_rules! phase_methods {
    ($($method:ident => $phase:ident),+ $(,)?) => {
        $(fn $method(&mut self) -> Result<(), String> {
            self.start_phase(StartupPhase::$phase)
        })+
    };
}

impl<A: NativeStartupAdapter> StartupServices for NativeStartupServices<A> {
    phase_methods!(
        lock_memory => LockMemory,
        init_rt_threads => RtThreads,
        register_main_thread => MainThread,
        init_platform_threads => PlatformThreads,
        init_sdl => Sdl,
        init_memory_manager => MemoryManager,
        init_event_manager => EventManager,
        activate_video => Video,
        wait_for_video => VideoReady,
        init_audio => Audio,
        init_core_graph => CoreGraph,
        init_synth_and_buffers => SynthAndBuffers,
        init_loop_and_scene_browsers => Browsers,
        init_input_and_midi => InputAndMidi,
        init_osc_and_mixer => OscAndMixer,
        link_system_variables => SystemVariables,
        activate_signal_processing => SignalProcessing,
        init_streamers_and_finalize_rings => StreamersAndRings,
        add_processing_elements => ProcessingElements,
    );

    fn rollback_setup(&mut self) {
        while let Some(phase) = self.completed.pop() {
            self.adapter.rollback(phase);
        }
    }

    fn commit_setup(&mut self) {
        // `FweelinStartupGuard::Release()` discards only failure rollback
        // actions.  Normal teardown is performed by NativeComponents in the
        // same interface-before-graph order as C++ application shutdown.
        self.completed.clear();
    }
}

impl<A: NativeStartupAdapter> Drop for NativeStartupServices<A> {
    fn drop(&mut self) {
        self.rollback_setup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Recorder(Arc<Mutex<Vec<(bool, StartupPhase)>>>);
    impl NativeStartupAdapter for Recorder {
        fn start(&mut self, phase: StartupPhase, _: &NativePaths) -> Result<(), String> {
            self.0.lock().unwrap().push((true, phase));
            Ok(())
        }
        fn rollback(&mut self, phase: StartupPhase) {
            self.0.lock().unwrap().push((false, phase));
        }
    }

    fn paths() -> NativePaths {
        NativePaths {
            resources: PathBuf::from("resources"),
            application_support: PathBuf::from("support"),
            config: PathBuf::from("resources/fweelin.xml"),
        }
    }

    #[test]
    fn rollback_is_reverse_order_and_one_shot() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut startup = NativeStartupServices::new(paths(), Recorder(calls.clone()));
        startup.init_sdl().unwrap();
        startup.init_audio().unwrap();
        startup.rollback_setup();
        startup.rollback_setup();
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                (true, StartupPhase::Sdl),
                (true, StartupPhase::Audio),
                (false, StartupPhase::Audio),
                (false, StartupPhase::Sdl),
            ]
        );
    }

    #[test]
    fn committed_startup_does_not_replay_failure_rollback_on_drop() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut startup = NativeStartupServices::new(paths(), Recorder(calls.clone()));
        startup.init_sdl().unwrap();
        startup.init_audio().unwrap();
        startup.commit_setup();
        drop(startup);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![(true, StartupPhase::Sdl), (true, StartupPhase::Audio)]
        );
    }
}
