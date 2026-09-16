//! Top-level application orchestration.
//!
//! `Fweelin` used to own concrete SDL, JACK, MIDI, video, DSP, browser and
//! persistence objects.  Those implementations are being migrated separately,
//! so this module owns the application state and requires an adapter for every
//! operation it performs.  In particular, a missing subsystem is an error, not
//! an implicit no-op.

use std::collections::BTreeMap;

/// A loaded loop shown in the loop tray. Matching and ordering are by ID,
/// as in the original BrowserItem subclass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopTrayItem {
    /// Persisted C++ loop id (the `loopid` attribute written into scenes),
    /// not a runtime slot: see [`LoopSnapshot::slot`].
    pub loop_id: i32,
    pub name: String,
    pub default_name: bool,
    pub place_name: String,
    pub x: i32,
    pub y: i32,
}

impl LoopTrayItem {
    pub fn new(
        loop_id: i32,
        name: impl Into<String>,
        default_name: bool,
        place_name: impl Into<String>,
    ) -> Self {
        Self {
            loop_id,
            name: name.into(),
            default_name,
            place_name: place_name.into(),
            x: -1,
            y: -1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopStatus {
    Off,
    Recording,
    Overdubbing,
    Playing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoopSnapshot {
    /// Runtime loop slot the snapshot captured, `0..MAX_RUNTIME_LOOPS`. This is
    /// a slot index rather than the persisted loop id
    /// ([`LoopTrayItem::loop_id`]), so it is unsigned and wide enough for the
    /// whole address space (`MAX_RUNTIME_LOOPS` is 512, which u8 cannot hold).
    pub slot: u16,
    pub status: LoopStatus,
    pub loop_volume: f32,
    pub trigger_volume: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub loops: Vec<LoopSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreEvent {
    StartSession,
    ExitSession,
    ToggleDiskOutput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    Stopped,
    Writing,
}

/// Why [`Core::trigger_snapshot`] failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// No snapshot is stored at the requested index (caller error).
    Missing { index: usize },
    /// The adapter could not restore an existing snapshot (recoverable
    /// runtime/DSP failure).
    Restore { index: usize, error: String },
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { index } => write!(f, "snapshot {index} does not exist"),
            Self::Restore { index, error } => {
                write!(f, "restore snapshot {index}: {error}")
            }
        }
    }
}

impl std::error::Error for SnapshotError {}

/// Adapter for all resources which are not yet owned by this crate.
pub trait CoreServices {
    fn setup(&mut self) -> Result<(), String>;
    fn start_session(&mut self) -> Result<(), String>;
    fn start_interfaces(&mut self) -> Result<(), String>;
    fn poll_event(&mut self) -> Result<Option<CoreEvent>, String>;
    fn set_streaming(&mut self, enabled: bool, sequence: u64) -> Result<(), String>;
    fn stream_state(&self) -> StreamState;
    fn stream_bytes(&self) -> u64;
    fn close_video(&mut self);
    fn close_sdl(&mut self);
    fn close_midi(&mut self);
    fn close_audio(&mut self);
    fn shutdown(&mut self);
    /// Undo a partially completed [`CoreServices::setup`].
    ///
    /// Post-condition: the adapter is back in a clean state, so a later
    /// `setup` retry is safe. Implementations report a failure here instead of
    /// assuming the undo succeeded.
    fn rollback_setup(&mut self) -> Result<(), String>;
    fn snapshot_loops(&self) -> Vec<LoopSnapshot>;
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), String>;
}

#[derive(Debug)]
pub struct Core<S: CoreServices> {
    services: S,
    running: bool,
    setup_complete: bool,
    write_sequence: u64,
    stream_name: String,
    snapshots: BTreeMap<usize, Snapshot>,
}

impl<S: CoreServices> Core<S> {
    pub fn new(services: S) -> Self {
        Self {
            services,
            running: false,
            setup_complete: false,
            write_sequence: 0,
            stream_name: String::new(),
            snapshots: BTreeMap::new(),
        }
    }
    pub fn is_running(&self) -> bool {
        self.running
    }
    pub fn stream_name(&self) -> &str {
        &self.stream_name
    }
    pub fn services(&self) -> &S {
        &self.services
    }
    pub fn services_mut(&mut self) -> &mut S {
        &mut self.services
    }

    pub fn setup(&mut self) -> Result<(), String> {
        if let Err(error) = self.services.setup() {
            // A failed rollback leaves the adapter in an unknown state, which
            // is worth reporting next to the setup failure itself.
            return match self.services.rollback_setup() {
                Ok(()) => Err(error),
                Err(rollback) => Err(format!("{error}; rollback failed: {rollback}")),
            };
        }
        self.setup_complete = true;
        Ok(())
    }

    /// Run the main-thread loop.  SDL polling remains in the adapter, matching
    /// the C++ requirement that window operations happen on the main thread.
    pub fn go(&mut self) -> Result<(), String> {
        if !self.setup_complete {
            return Err("core is not set up".into());
        }
        self.running = true;
        // The run sequence is captured so shutdown always happens: leaving the
        // session, interfaces or stream open after a mid-loop error would leak
        // them (and `is_running` would keep reporting `true`).
        let result = (|| -> Result<(), String> {
            self.services.start_session()?;
            self.services.start_interfaces()?;
            while self.running {
                match self.services.poll_event()? {
                    Some(event) => self.handle_event(event)?,
                    None => break,
                }
            }
            Ok(())
        })();
        self.shutdown();
        result
    }

    pub fn handle_event(&mut self, event: CoreEvent) -> Result<(), String> {
        match event {
            CoreEvent::StartSession => self.services.start_session(),
            CoreEvent::ExitSession => {
                self.running = false;
                Ok(())
            }
            CoreEvent::ToggleDiskOutput => self.toggle_disk_output(),
        }
    }

    pub fn toggle_disk_output(&mut self) -> Result<(), String> {
        let writing = self.services.stream_state() == StreamState::Writing;
        if writing {
            self.services.set_streaming(false, self.write_sequence)?;
            self.stream_name.clear();
            self.write_sequence += 1;
        } else {
            // Publish the name only after the adapter accepted the stream,
            // otherwise `stream_name` would advertise a stream that is not
            // being written.
            self.services.set_streaming(true, self.write_sequence)?;
            self.stream_name = format!("freewheeling-{:04}", self.write_sequence);
        }
        Ok(())
    }

    /// Store a snapshot at `index`, returning the snapshot it replaced.
    ///
    /// The slot is overwritten, matching the C++ behaviour; the previous value
    /// is returned so callers can detect the loss.
    pub fn create_snapshot(&mut self, index: usize, name: impl Into<String>) -> Option<Snapshot> {
        self.snapshots.insert(
            index,
            Snapshot {
                name: name.into(),
                loops: self.services.snapshot_loops(),
            },
        )
    }
    pub fn trigger_snapshot(&mut self, index: usize) -> Result<(), SnapshotError> {
        let snapshot = self
            .snapshots
            .get(&index)
            .ok_or(SnapshotError::Missing { index })?;
        self.services
            .restore_snapshot(snapshot)
            .map_err(|error| SnapshotError::Restore { index, error })
    }
    pub fn snapshot(&self, index: usize) -> Option<&Snapshot> {
        self.snapshots.get(&index)
    }

    pub fn stream_stats(&self) -> (StreamState, u64) {
        (self.services.stream_state(), self.services.stream_bytes())
    }

    pub fn shutdown(&mut self) {
        if !self.setup_complete {
            return;
        }
        self.running = false;
        self.services.close_video();
        self.services.close_sdl();
        self.services.close_midi();
        self.services.close_audio();
        self.services.shutdown();
        self.setup_complete = false;
    }
}

impl<S: CoreServices> Drop for Core<S> {
    fn drop(&mut self) {
        // Safe to call even if the user already called shutdown() —
        // shutdown() guards on setup_complete and each close_* method
        // is idempotent (NativeComponents tracks per-resource state).
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        state: StreamState,
        events: Vec<Option<CoreEvent>>,
        starts: usize,
        closes: usize,
    }
    impl CoreServices for Fake {
        fn setup(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn start_session(&mut self) -> Result<(), String> {
            self.starts += 1;
            Ok(())
        }
        fn start_interfaces(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn poll_event(&mut self) -> Result<Option<CoreEvent>, String> {
            Ok(self.events.pop().unwrap_or(None))
        }
        fn set_streaming(&mut self, e: bool, _: u64) -> Result<(), String> {
            self.state = if e {
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
            7
        }
        fn close_video(&mut self) {}
        fn close_sdl(&mut self) {}
        fn close_midi(&mut self) {}
        fn close_audio(&mut self) {}
        fn shutdown(&mut self) {
            self.closes += 1
        }
        fn rollback_setup(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn snapshot_loops(&self) -> Vec<LoopSnapshot> {
            vec![]
        }
        fn restore_snapshot(&mut self, _: &Snapshot) -> Result<(), String> {
            Ok(())
        }
    }
    #[test]
    fn lifecycle_and_stream_toggle() {
        let f = Fake {
            state: StreamState::Stopped,
            events: vec![Some(CoreEvent::ExitSession)],
            starts: 0,
            closes: 0,
        };
        let mut c = Core::new(f);
        c.setup().unwrap();
        c.go().unwrap();
        assert_eq!(c.services().starts, 1);
        assert_eq!(c.services().closes, 1);
    }
    #[test]
    fn snapshots_are_stateful() {
        let f = Fake {
            state: StreamState::Stopped,
            events: vec![],
            starts: 0,
            closes: 0,
        };
        let mut c = Core::new(f);
        c.setup().unwrap();
        c.create_snapshot(2, "scene");
        assert_eq!(c.snapshot(2).unwrap().name, "scene");
    }
}
