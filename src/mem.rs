//! Bounded deferred allocation for objects used by real-time code.
//!
//! Both callback-facing queues are fixed-capacity lock-free queues. Allocation,
//! destruction, recycling, and waiting are confined to the manager thread.

use crossbeam_queue::ArrayQueue;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const MEMMGR_UPDATE_QUEUE_SIZE: usize = 8192;
pub const PREALLOC_DEFAULT_NUM_INSTANCES: usize = 10;

pub trait Preallocated: Send {
    fn recycle(&mut self) {}
}

pub type Instance = Box<dyn Preallocated>;

/// Observable result of a deferred delete. This intentionally is not
/// `#[must_use]` for source compatibility; callback code that needs lossless
/// overload handling can recover the retained instance with `into_rejected`.
pub struct RtDeleteOutcome(Option<Instance>);

impl RtDeleteOutcome {
    pub fn accepted(&self) -> bool {
        self.0.is_none()
    }

    pub fn into_rejected(self) -> Option<Instance> {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryManagerUpdateType {
    RestockInstance,
    FreeInstance,
}

pub struct MemoryManagerUpdate {
    pub which_pt: Weak<PreallocatedTypeInner>,
    pub update_type: MemoryManagerUpdateType,
    pub update_idx: usize,
    pub tofree: Option<Instance>,
}

impl MemoryManagerUpdate {
    pub fn invalid() -> Self {
        Self {
            which_pt: Weak::new(),
            update_type: MemoryManagerUpdateType::RestockInstance,
            update_idx: 0,
            tofree: None,
        }
    }
    pub fn is_valid(&self) -> bool {
        self.which_pt.strong_count() != 0
    }
}

pub struct MemoryManager {
    queue: Arc<ArrayQueue<MemoryManagerUpdate>>,
    stopping: Arc<AtomicBool>,
    rejected: AtomicU64,
    types: Mutex<Vec<Weak<PreallocatedTypeInner>>>,
    thread: Option<JoinHandle<()>>,
}

impl MemoryManager {
    pub fn new() -> Self {
        let queue: Arc<ArrayQueue<MemoryManagerUpdate>> =
            Arc::new(ArrayQueue::new(MEMMGR_UPDATE_QUEUE_SIZE));
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_queue = Arc::clone(&queue);
        let worker_stopping = Arc::clone(&stopping);
        let thread = thread::Builder::new()
            .name("mem-mgr".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                loop {
                    if let Some(update) = worker_queue.pop() {
                        if let Some(pt) = update.which_pt.upgrade() {
                            pt.process(update);
                        }
                        continue;
                    }
                    if worker_stopping.load(Ordering::Acquire) {
                        return;
                    }
                    thread::park_timeout(Duration::from_millis(1));
                }
            })
            .expect("failed to create memory manager thread");
        Self {
            queue,
            stopping,
            rejected: AtomicU64::new(0),
            types: Mutex::new(Vec::new()),
            thread: Some(thread),
        }
    }

    pub fn add_type(&self, pt: &Arc<PreallocatedTypeInner>) {
        self.types.lock().unwrap().push(Arc::downgrade(pt));
    }

    /// Enqueue without waiting. On overflow the complete update is returned,
    /// so a callback can retain ownership instead of destroying an item there.
    pub fn wake_up(&self, update: MemoryManagerUpdate) -> Result<(), MemoryManagerUpdate> {
        match self.queue.push(update) {
            Ok(()) => {
                if let Some(worker) = &self.thread {
                    worker.thread().unpark();
                }
                Ok(())
            }
            Err(update) => {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                Err(update)
            }
        }
    }

    pub fn rejected_updates(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }

    pub fn process_queue(&self) {
        while let Some(update) = self.queue.pop() {
            if let Some(pt) = update.which_pt.upgrade() {
                pt.process(update);
            }
        }
    }
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MemoryManager {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
        // The worker drains every accepted update before observing an empty
        // queue and stopping, so no callback-owned instance is reclaimed early.
    }
}

pub struct PreallocatedTypeInner {
    factory: Box<dyn Fn() -> Instance + Send + Sync>,
    ready: ArrayQueue<Instance>,
    // C++ block-mode objects are returned to a free-block list by
    // GoPostdelete().  A later GoPreallocate() moves one back into the
    // ready list.  Keep that deliberately non-RT bookkeeping off the
    // callback-facing queues.
    recycled: Mutex<Vec<Instance>>,
    manager: Weak<MemoryManager>,
    block_mode: bool,
    block_size: usize,
    ready_overflow: AtomicU64,
}

impl PreallocatedTypeInner {
    pub fn new<F>(
        manager: &Arc<MemoryManager>,
        count: usize,
        block_mode: bool,
        factory: F,
    ) -> Arc<Self>
    where
        F: Fn() -> Instance + Send + Sync + 'static,
    {
        assert!(count > 0 && (!block_mode || count >= 3));
        let pt = Arc::new(Self {
            factory: Box::new(factory),
            // This is the C++ ready_list capacity.  It is never resized from
            // an audio callback; a successful RTNew creates one vacant slot
            // which its deferred RestockInstance update refills.
            ready: ArrayQueue::new(count),
            recycled: Mutex::new(Vec::new()),
            manager: Arc::downgrade(manager),
            block_mode,
            block_size: count,
            ready_overflow: AtomicU64::new(0),
        });
        // In C++ block mode the first element of the first array is the
        // permanent prototype/base instance, so only count - 1 instances are
        // initially consumable.  Instance mode preallocates all count items.
        let initially_ready = if block_mode { count - 1 } else { count };
        for _ in 0..initially_ready {
            pt.ready.push((pt.factory)()).ok().unwrap();
        }
        manager.add_type(&pt);
        pt
    }

    pub fn rt_new(self: &Arc<Self>) -> Option<Instance> {
        let item = self.ready.pop();
        if item.is_some()
            && let Some(m) = self.manager.upgrade()
        {
            let _ = m.wake_up(MemoryManagerUpdate {
                which_pt: Arc::downgrade(self),
                update_type: MemoryManagerUpdateType::RestockInstance,
                update_idx: 0,
                tofree: None,
            });
        }
        item
    }

    /// Non-realtime convenience API. Never call this from an audio callback.
    pub fn rt_new_with_wait(self: &Arc<Self>) -> Instance {
        loop {
            if let Some(x) = self.rt_new() {
                break x;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Defers destruction. Returns the instance unchanged if the bounded
    /// manager queue is full, making overflow observable and RT-safe.
    pub fn rt_delete(self: &Arc<Self>, instance: Instance) -> RtDeleteOutcome {
        let Some(m) = self.manager.upgrade() else {
            return RtDeleteOutcome(Some(instance));
        };
        match m.wake_up(MemoryManagerUpdate {
            which_pt: Arc::downgrade(self),
            update_type: MemoryManagerUpdateType::FreeInstance,
            update_idx: 0,
            tofree: Some(instance),
        }) {
            Ok(()) => RtDeleteOutcome(None),
            Err(mut update) => RtDeleteOutcome(Some(
                update.tofree.take().expect("free update retains instance"),
            )),
        }
    }

    fn process(&self, mut update: MemoryManagerUpdate) {
        match update.update_type {
            MemoryManagerUpdateType::FreeInstance => {
                let mut item = update
                    .tofree
                    .take()
                    .expect("free update must retain its instance");
                if self.block_mode {
                    // Match GoPostdelete(): recycle and mark free, but do
                    // not make this object callback-consumable yet.
                    item.recycle();
                    self.recycled.lock().unwrap().push(item);
                }
                // In C++ instance mode GoPostdelete() uses ::delete here.
                // Dropping `item` implements exactly that path.
            }
            MemoryManagerUpdateType::RestockInstance => {
                // Match GoPreallocate(): block-mode restocks reuse a free
                // object first, otherwise a new allocation/block is made.
                // Rust's erased factory produces one object at a time; the
                // callback-visible ready-list lifecycle is nevertheless the
                // same and its capacity remains fixed.
                let item = if self.block_mode {
                    self.recycled
                        .lock()
                        .unwrap()
                        .pop()
                        .unwrap_or_else(|| (self.factory)())
                } else {
                    (self.factory)()
                };
                if let Err(item) = self.ready.push(item) {
                    // A matching C++ ready-list state transition would be a
                    // fatal invariant violation.  Preserve callback safety by
                    // dropping only on the manager thread, and expose it for
                    // diagnostics/tests instead of silently growing memory.
                    self.ready_overflow.fetch_add(1, Ordering::Relaxed);
                    drop(item);
                }
            }
        }
    }

    pub fn get_block_size(&self) -> usize {
        self.block_size
    }

    pub fn ready_overflows(&self) -> u64 {
        self.ready_overflow.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Item(u32);
    impl Preallocated for Item {
        fn recycle(&mut self) {
            self.0 = 0;
        }
    }
    #[test]
    fn allocates_and_defers_delete() {
        let mm = Arc::new(MemoryManager::new());
        let pt = PreallocatedTypeInner::new(&mm, 1, false, || Box::new(Item(1)));
        let x = pt.rt_new().unwrap();
        assert!(pt.rt_delete(x).accepted());
        for _ in 0..100 {
            if pt.rt_new().is_some() {
                return;
            }
            // The manager deliberately parks for up to one millisecond while
            // idle; yielding alone can exhaust before it is scheduled.
            thread::sleep(Duration::from_millis(1));
        }
        panic!("manager did not replenish pool");
    }

    #[test]
    fn instance_mode_delete_does_not_recycle_the_object() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static RECYCLES: AtomicUsize = AtomicUsize::new(0);
        struct Recyclable;
        impl Preallocated for Recyclable {
            fn recycle(&mut self) {
                RECYCLES.fetch_add(1, Ordering::SeqCst);
            }
        }

        RECYCLES.store(0, Ordering::SeqCst);
        let mm = Arc::new(MemoryManager::new());
        let pt = PreallocatedTypeInner::new(&mm, 1, false, || Box::new(Recyclable));
        assert!(pt.rt_delete(pt.rt_new().unwrap()).accepted());
        for _ in 0..100 {
            mm.process_queue();
            if pt.rt_new().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(RECYCLES.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn block_mode_reserves_the_base_and_recycles_before_restock() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static RECYCLES: AtomicUsize = AtomicUsize::new(0);
        struct Recyclable;
        impl Preallocated for Recyclable {
            fn recycle(&mut self) {
                RECYCLES.fetch_add(1, Ordering::SeqCst);
            }
        }

        RECYCLES.store(0, Ordering::SeqCst);
        let mm = Arc::new(MemoryManager::new());
        let pt = PreallocatedTypeInner::new(&mm, 3, true, || Box::new(Recyclable));
        assert_eq!(pt.ready.len(), 2, "C++ block base is not consumable");
        let first = pt.rt_new().unwrap();
        let second = pt.rt_new().unwrap();
        assert!(pt.rt_delete(first).accepted());
        for _ in 0..100 {
            mm.process_queue();
            if RECYCLES.load(Ordering::SeqCst) == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(RECYCLES.load(Ordering::SeqCst), 1);
        // A queued restock will later use this recycled object before it
        // allocates again, matching GoPostdelete/GoPreallocate ordering.
        drop(second);
    }
}
