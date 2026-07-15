//! Platform-owned startup rollback.
//!
//! The plain startup guard stores callbacks.  Platform startup also needs to
//! retain the handle acquired by a callback until that callback is either
//! rolled back or committed.  `OwnedResource` makes that ownership explicit.

type Cleanup = Box<dyn FnOnce(i32) + Send + 'static>;

/// A platform handle together with the operation which releases it.
pub struct OwnedResource<H> {
    handle: Option<H>,
    cleanup: Option<Box<dyn FnOnce(H, i32) + Send + 'static>>,
}

impl<H> OwnedResource<H> {
    /// The handle is retained until rollback, or dropped on release.
    pub fn new(handle: H, cleanup: impl FnOnce(H, i32) + Send + 'static) -> Self
    where
        H: Send + 'static,
    {
        Self {
            handle: Some(handle),
            cleanup: Some(Box::new(cleanup)),
        }
    }

    fn rollback(mut self, tag: i32) {
        let handle = self.handle.take().expect("resource cleanup called twice");
        (self.cleanup.take().expect("resource cleanup missing"))(handle, tag);
    }
}

struct Entry {
    tag: i32,
    cleanup: Cleanup,
}

/// A bounded, one-shot collection of platform startup cleanup actions.
pub struct PlatformStartupGuard {
    entries: Vec<Entry>,
    released: bool,
}

impl PlatformStartupGuard {
    pub const MAX_ENTRIES: usize = 128;

    pub fn new() -> Self {
        Self {
            entries: Vec::with_capacity(Self::MAX_ENTRIES),
            released: false,
        }
    }

    /// Registers a tagged cleanup callback, retaining it only if capacity is available.
    pub fn push<F>(&mut self, tag: i32, cleanup: F) -> bool
    where
        F: FnOnce(i32) + Send + 'static,
    {
        if !self.released && self.entries.len() < Self::MAX_ENTRIES {
            self.entries.push(Entry {
                tag,
                cleanup: Box::new(cleanup),
            });
            true
        } else {
            false
        }
    }

    /// Registers an owned platform handle and its tagged cleanup operation.
    pub fn push_resource<H>(&mut self, tag: i32, resource: OwnedResource<H>) -> bool
    where
        H: Send + 'static,
    {
        self.push(tag, move |tag| resource.rollback(tag))
    }

    /// Commits startup. Pending callbacks and owned handles are dropped.
    pub fn release(&mut self) {
        self.released = true;
        self.entries.clear();
    }

    /// Cleans up in reverse registration order, then permanently disables the guard.
    pub fn rollback(&mut self) {
        if self.released {
            return;
        }
        while let Some(entry) = self.entries.pop() {
            (entry.cleanup)(entry.tag);
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

impl Default for PlatformStartupGuard {
    fn default() -> Self {
        Self::new()
    }
}
