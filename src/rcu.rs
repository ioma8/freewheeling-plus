//! Realtime read-copy-update support.
//!
//! This is the Rust counterpart of `fweelin_rcu.h`.  The objects referenced by
//! an RCU slot are deliberately not owned here: callers may reclaim the value
//! returned by `AtomicPtr::swap` after [`Rcu::synchronize`] completes.

use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering, fence};
use std::thread::ThreadId;
use std::time::Duration;

pub const MAX_RW_THREADS: usize = 50;

/// Stable pre-registered reader slot. Keeping this token on the callback-owned
/// processor makes the RCU read side entirely atomic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RcuReader(usize);

/// The process-wide reader/writer registration boundary used by an [`Rcu`].
/// Register threads before they call `read_lock`; registration is permanent,
/// matching the lifetime assumptions of the original RT_RWThreads class.
pub struct RcuRegistry {
    ids: Mutex<Vec<ThreadId>>,
}

impl RcuRegistry {
    pub fn new() -> Self {
        Self {
            ids: Mutex::new(Vec::new()),
        }
    }

    pub fn register_current(&self) -> Result<usize, &'static str> {
        self.register(std::thread::current().id())
    }

    pub fn register_current_reader(&self) -> Result<RcuReader, &'static str> {
        self.register_current().map(RcuReader)
    }

    pub fn register(&self, id: ThreadId) -> Result<usize, &'static str> {
        let mut ids = self.ids.lock().expect("RCU registry mutex poisoned");
        if let Some(i) = ids.iter().position(|known| *known == id) {
            return Ok(i);
        }
        if ids.len() == MAX_RW_THREADS {
            return Err("too many RCU reader threads");
        }
        ids.push(id);
        Ok(ids.len() - 1)
    }

    fn index(&self, id: ThreadId) -> Option<usize> {
        self.ids
            .lock()
            .expect("RCU registry mutex poisoned")
            .iter()
            .position(|known| *known == id)
    }

    fn len(&self) -> usize {
        self.ids.lock().expect("RCU registry mutex poisoned").len()
    }
}

impl Default for RcuRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Lock-free read side, with serialized registration only.
pub struct Rcu<T> {
    registry: RcuRegistry,
    readers: Box<[AtomicU32; MAX_RW_THREADS]>,
    /// C++ captures `RT_RWThreads::num_rw_threads` when the RCU is created
    /// and rejects any later registration change.
    num_readers: usize,
    global_time: AtomicU32,
    last_update: AtomicU32,
    _marker: std::marker::PhantomData<*mut T>,
}

impl<T> Rcu<T> {
    pub fn new(registry: RcuRegistry) -> Self {
        Self {
            num_readers: registry.len(),
            registry,
            readers: Box::new(std::array::from_fn(|_| AtomicU32::new(0))),
            global_time: AtomicU32::new(1),
            last_update: AtomicU32::new(0),
            _marker: std::marker::PhantomData,
        }
    }

    pub fn registry(&self) -> &RcuRegistry {
        &self.registry
    }

    fn readers_are_frozen(&self) -> Result<(), &'static str> {
        (self.registry.len() == self.num_readers)
            .then_some(())
            .ok_or("RCU reader registration changed after initialization")
    }

    pub fn read_lock(&self) -> Result<(), &'static str> {
        self.readers_are_frozen()?;
        let i = self
            .registry
            .index(std::thread::current().id())
            .ok_or("RCU read lock from unregistered thread")?;
        let count = self.global_time.fetch_add(1, Ordering::SeqCst);
        self.readers[i].store(count, Ordering::Release);
        fence(Ordering::SeqCst);
        Ok(())
    }

    /// Lock-free callback API. `reader` must have been obtained from this
    /// registry during non-realtime setup and used by only that reader thread.
    pub fn read_lock_registered(&self, reader: RcuReader) -> Result<(), &'static str> {
        self.readers_are_frozen()?;
        let Some(slot) = self
            .readers
            .get(reader.0)
            .filter(|_| reader.0 < self.num_readers)
        else {
            return Err("invalid RCU reader slot");
        };
        let count = self.global_time.fetch_add(1, Ordering::SeqCst);
        slot.store(count, Ordering::Release);
        fence(Ordering::SeqCst);
        Ok(())
    }

    pub fn read_unlock(&self) -> Result<(), &'static str> {
        self.readers_are_frozen()?;
        let i = self
            .registry
            .index(std::thread::current().id())
            .ok_or("RCU read unlock from unregistered thread")?;
        self.readers[i].store(0, Ordering::Release);
        fence(Ordering::SeqCst);
        Ok(())
    }

    pub fn read_unlock_registered(&self, reader: RcuReader) -> Result<(), &'static str> {
        self.readers_are_frozen()?;
        let Some(slot) = self
            .readers
            .get(reader.0)
            .filter(|_| reader.0 < self.num_readers)
        else {
            return Err("invalid RCU reader slot");
        };
        slot.store(0, Ordering::Release);
        fence(Ordering::SeqCst);
        Ok(())
    }

    /// Publish `new_ptr` and return the previously published pointer.
    ///
    /// # Safety
    ///
    /// `new_ptr` must remain valid until a later update and grace period. The
    /// returned pointer may only be reclaimed after `synchronize` completes.
    pub unsafe fn update(&self, slot: &AtomicPtr<T>, new_ptr: *mut T) -> *mut T {
        self.readers_are_frozen()
            .expect("RCU reader registration changed after initialization");
        let old = slot.swap(new_ptr, Ordering::Release);
        fence(Ordering::SeqCst);
        let update = self.global_time.fetch_add(1, Ordering::SeqCst);
        self.last_update.store(update, Ordering::Release);
        old
    }

    pub fn synchronize(&self, sleep_time: Duration) {
        self.readers_are_frozen()
            .expect("RCU reader registration changed after initialization");
        let update = self.last_update.load(Ordering::Acquire);
        loop {
            let active = (0..self.num_readers).any(|i| {
                let lock = self.readers[i].load(Ordering::Acquire);
                lock != 0 && lock < update
            });
            if !active {
                return;
            }
            std::thread::sleep(sleep_time);
        }
    }
}

unsafe impl<T: Send> Send for Rcu<T> {}
unsafe impl<T: Send> Sync for Rcu<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn rejects_unregistered_readers() {
        let rcu = Rcu::<u32>::new(RcuRegistry::new());
        assert!(rcu.read_lock().is_err());
    }

    #[test]
    fn publishes_and_reclaims_after_unlock() {
        let registry = RcuRegistry::new();
        registry.register_current().unwrap();
        let rcu = Arc::new(Rcu::<u32>::new(registry));
        let slot = AtomicPtr::new(Box::into_raw(Box::new(1)));
        rcu.read_lock().unwrap();
        let old = unsafe { rcu.update(&slot, Box::into_raw(Box::new(2))) };
        rcu.read_unlock().unwrap();
        rcu.synchronize(Duration::from_micros(1));
        unsafe {
            drop(Box::from_raw(old));
            drop(Box::from_raw(slot.load(Ordering::Acquire)));
        }
    }

    #[test]
    fn rejects_reader_registration_after_rcu_initialization() {
        let registry = RcuRegistry::new();
        registry.register_current().unwrap();
        let rcu = Rcu::<u32>::new(registry);
        let registered_elsewhere = std::thread::scope(|scope| {
            scope
                .spawn(|| rcu.registry().register_current())
                .join()
                .unwrap()
        });
        assert!(registered_elsewhere.is_ok());
        assert!(rcu.read_lock().is_err());
    }
}
