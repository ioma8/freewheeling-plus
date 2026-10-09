//! Commands exchanged between the processor-management and processing threads.
//!
//! This is the Rust counterpart of `fweelin_processor_queue.{h,cc}`.  Processor
//! objects are owned by the processor graph; this queue only carries their
//! addresses and never dereferences or drops them.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::realtime_guard::InstrumentedMutex;

/// Opaque processor handle.  The concrete processor implementation lives in a
/// later migration unit.
#[repr(C)]
pub struct Processor {
    _private: [u8; 0],
}

/// Opaque processor-item handle.  Ownership remains with the processor graph.
#[repr(C)]
pub struct ProcessorItem {
    _private: [u8; 0],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum ProcessorCommandType {
    Add,
    RequestDelete,
}

/// One queued processor operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct ProcessorCommand {
    pub command_type: ProcessorCommandType,
    pub item: *mut ProcessorItem,
    pub processor: *mut Processor,
}

impl Default for ProcessorCommand {
    fn default() -> Self {
        Self {
            command_type: ProcessorCommandType::Add,
            item: std::ptr::null_mut(),
            processor: std::ptr::null_mut(),
        }
    }
}

// Raw pointers are handles only (ZST [u8; 0] — never dereferenced). The
// queue merely stores and returns them; every access happens on the owning
// thread after read_next transfers ownership of the command. C++ compat.
//
// SAFETY (Send): the handles are never dereferenced through the Send boundary
// — they are only accessed by the owning thread after the command is consumed.
//
// Lifetime invariant (not expressible in the type system, relied upon by
// every consumer that dereferences a popped handle): the `Processor` /
// `ProcessorItem` a command points at is owned by the processor graph and MUST
// outlive the queue and stay valid until the command is consumed and the
// referenced object is released.
unsafe impl Send for ProcessorCommand {}

pub struct ProcessorCommandQueue {
    // This is intentionally a mutex rather than a lock-free queue.  The C++
    // `ReadNext` uses `pthread_mutex_trylock`: the realtime thread must never
    // wait for a producer, and treats a producer-held mutex exactly like an
    // empty queue for that callback.  A lock-free queue changes that
    // externally observable timing by allowing the consumer to receive an
    // item while an enqueue is in progress. The wrapping type is the
    // instrumented mutex so a future change to a blocking realtime read
    // becomes an acceptance-counter failure instead of silent waiting.
    commands: InstrumentedMutex<VecDeque<ProcessorCommand>>,
    rejected: AtomicU64,
}

impl ProcessorCommandQueue {
    pub const MAX_COMMANDS: usize = 256;

    pub fn new() -> Self {
        Self {
            commands: InstrumentedMutex::new(VecDeque::with_capacity(Self::MAX_COMMANDS)),
            rejected: AtomicU64::new(0),
        }
    }

    pub fn enqueue_add(&self, item: *mut ProcessorItem) -> bool {
        self.enqueue(ProcessorCommand {
            command_type: ProcessorCommandType::Add,
            item,
            processor: std::ptr::null_mut(),
        })
    }

    pub fn enqueue_delete(&self, processor: *mut Processor) -> bool {
        self.enqueue(ProcessorCommand {
            command_type: ProcessorCommandType::RequestDelete,
            item: std::ptr::null_mut(),
            processor,
        })
    }

    fn enqueue(&self, command: ProcessorCommand) -> bool {
        // C++'s producer path waits for its mutex, then refuses only once the
        // fixed 256-entry ring is full.  Recover a poisoned mutex because C++
        // has no poison state and the queued command objects are plain data.
        let mut commands = self
            .commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if commands.len() == Self::MAX_COMMANDS {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            false
        } else {
            commands.push_back(command);
            true
        }
    }

    /// Attempts to read one command without waiting.  This preserves C++
    /// `ReadNext`'s `pthread_mutex_trylock` behavior: contention produces an
    /// immediate false result and leaves the FIFO unchanged for a later audio
    /// callback.
    ///
    /// On `false` the FIFO is untouched and `command` is NOT written; callers
    /// must check the return value before using it, or they may re-process a
    /// stale command.
    pub fn read_next(&self, command: &mut ProcessorCommand) -> bool {
        let mut commands = match self.commands.try_lock() {
            Ok(guard) => guard,
            // A panic while a producer held the lock must not silence the
            // realtime reader forever; recover exactly like `enqueue` and
            // `pending_count`, since C++ has no poison state.
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        if let Some(next) = commands.pop_front() {
            *command = next;
            true
        } else {
            false
        }
    }

    pub fn pending_count(&self) -> usize {
        self.commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn rejected_count(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }
}

impl Default for ProcessorCommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_fifo_and_rejects_overflow() {
        let queue = ProcessorCommandQueue::new();
        let item = std::ptr::NonNull::<ProcessorItem>::dangling().as_ptr();
        let processor = std::ptr::NonNull::<Processor>::dangling().as_ptr();
        assert!(queue.enqueue_add(item));
        assert!(queue.enqueue_delete(processor));
        assert_eq!(queue.pending_count(), 2);
        let mut command = ProcessorCommand::default();
        assert!(queue.read_next(&mut command));
        assert_eq!(command.command_type, ProcessorCommandType::Add);
        assert_eq!(command.item, item);
        assert!(queue.read_next(&mut command));
        assert_eq!(command.command_type, ProcessorCommandType::RequestDelete);
        assert_eq!(command.processor, processor);
        assert!(!queue.read_next(&mut command));

        for _ in 0..ProcessorCommandQueue::MAX_COMMANDS {
            // ZST never dereferenced; null is fine for capacity testing.
            assert!(queue.enqueue_add(std::ptr::NonNull::<ProcessorItem>::dangling().as_ptr()));
        }
        assert!(!queue.enqueue_add(std::ptr::NonNull::<ProcessorItem>::dangling().as_ptr()));
        assert_eq!(queue.rejected_count(), 1);
    }

    #[test]
    fn a_poisoned_mutex_still_drains_the_queue() {
        let queue = std::sync::Arc::new(ProcessorCommandQueue::new());
        assert!(queue.enqueue_add(std::ptr::NonNull::<ProcessorItem>::dangling().as_ptr()));
        let poisoner = std::sync::Arc::clone(&queue);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.commands.lock().unwrap();
            panic!("poison the queue");
        })
        .join();
        // The realtime reader must keep draining instead of reporting false
        // for the rest of the session.
        let mut command = ProcessorCommand::default();
        assert!(queue.read_next(&mut command));
        assert_eq!(command.command_type, ProcessorCommandType::Add);
        assert!(!queue.read_next(&mut command));
    }

    #[test]
    fn read_next_skips_a_callback_when_a_producer_holds_the_cpp_mutex() {
        let queue = ProcessorCommandQueue::new();
        // ZST never dereferenced; NonNull::dangling is fine for this test.
        let item = std::ptr::NonNull::<ProcessorItem>::dangling().as_ptr();
        assert!(queue.enqueue_add(item));
        let held_lock = queue.commands.lock().unwrap();
        let mut command = ProcessorCommand::default();

        // `ProcessorCommandQueue::ReadNext` returns zero when
        // `pthread_mutex_trylock` cannot acquire the producer mutex.
        assert!(!queue.read_next(&mut command));
        assert_eq!(held_lock.len(), 1);
        drop(held_lock);

        assert!(queue.read_next(&mut command));
        assert_eq!(command.command_type, ProcessorCommandType::Add);
    }
}
