//! A bounded, one-shot guard for resources acquired during startup.
//!
//! This is the Rust equivalent of `FweelinStartupGuard`.  Rollback entries
//! are executed in reverse registration order.  Calling [`StartupGuard::release`]
//! commits the startup and discards all pending rollback actions; calling
//! [`StartupGuard::rollback`] executes each action at most once.

type RollbackFn = Box<dyn FnMut(i32) + Send>;

struct Entry {
    rollback: RollbackFn,
    tag: i32,
}

/// Tracks at most 128 startup rollback entries.
pub struct StartupGuard {
    entries: Vec<Entry>,
    released: bool,
}

impl StartupGuard {
    /// Maximum number of rollback actions retained by a guard.
    pub const MAX_ENTRIES: usize = 128;

    pub fn new() -> Self {
        Self {
            entries: Vec::with_capacity(Self::MAX_ENTRIES),
            released: false,
        }
    }

    /// Register an action to undo a successful startup step.
    ///
    /// Entries pushed after release, and entries beyond [`MAX_ENTRIES`], are
    /// ignored, matching the C++ guard.  The callback is allowed to mutate its
    /// captured state so callers can record cleanup or perform teardown.
    pub fn push<F>(&mut self, tag: i32, rollback: F)
    where
        F: FnMut(i32) + Send + 'static,
    {
        if self.released || self.entries.len() >= Self::MAX_ENTRIES {
            return;
        }

        self.entries.push(Entry {
            rollback: Box::new(rollback),
            tag,
        });
    }

    /// Commit startup and discard all pending rollback actions.
    pub fn release(&mut self) {
        self.released = true;
        self.entries.clear();
    }

    /// Undo all retained actions in LIFO order, then permanently disable the
    /// guard.  A second call has no effect.
    pub fn rollback(&mut self) {
        if self.released {
            return;
        }

        while let Some(mut entry) = self.entries.pop() {
            (entry.rollback)(entry.tag);
        }
        self.released = true;
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }

    pub fn is_released(&self) -> bool {
        self.released
    }
}

impl Default for StartupGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::StartupGuard;
    use std::sync::{Arc, Mutex};

    #[test]
    fn rollback_is_lifo_and_one_shot() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut guard = StartupGuard::new();
        for tag in [1, 2, 3] {
            let seen = Arc::clone(&seen);
            guard.push(tag, move |tag| seen.lock().unwrap().push(tag));
        }
        guard.rollback();
        guard.rollback();
        assert_eq!(*seen.lock().unwrap(), vec![3, 2, 1]);
        assert!(guard.is_released());
        assert_eq!(guard.count(), 0);
    }

    #[test]
    fn release_discards_entries_and_blocks_future_pushes() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut guard = StartupGuard::new();
        let callback_seen = Arc::clone(&seen);
        guard.push(1, move |tag| callback_seen.lock().unwrap().push(tag));
        guard.release();
        let callback_seen = Arc::clone(&seen);
        guard.push(2, move |tag| callback_seen.lock().unwrap().push(tag));
        guard.rollback();
        assert!(seen.lock().unwrap().is_empty());
        assert_eq!(guard.count(), 0);
    }

    #[test]
    fn capacity_is_bounded() {
        let mut guard = StartupGuard::new();
        for tag in 0..(StartupGuard::MAX_ENTRIES as i32 + 1) {
            guard.push(tag, |_| {});
        }
        assert_eq!(guard.count(), StartupGuard::MAX_ENTRIES);
    }
}
