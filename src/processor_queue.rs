//! Commands exchanged between the processor-management and processing threads.
//!
//! This is the Rust counterpart of `fweelin_processor_queue.{h,cc}`.  Processor
//! objects are owned by the processor graph; this queue only carries their
//! addresses and never dereferences or drops them.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

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

// Raw pointers are handles only. The queue never dereferences them, and the
// C++ implementation likewise permits commands to cross thread boundaries.
// SAFETY: ProcessorCommand holds raw pointers that are never dereferenced
// through the Send boundary — they are only accessed by the owning thread.
unsafe impl Send for ProcessorCommand {}

pub struct ProcessorCommandQueue {
    // This is intentionally a mutex rather than a lock-free queue.  The C++
    // `ReadNext` uses `pthread_mutex_trylock`: the realtime thread must never
    // wait for a producer, and treats a producer-held mutex exactly like an
    // empty queue for that callback.  A lock-free queue changes that
    // externally observable timing by allowing the consumer to receive an
    // item while an enqueue is in progress.
    commands: Mutex<VecDeque<ProcessorCommand>>,
    rejected: AtomicU64,
}

impl ProcessorCommandQueue {
    pub const MAX_COMMANDS: usize = 256;

    pub fn new() -> Self {
        Self {
            commands: Mutex::new(VecDeque::with_capacity(Self::MAX_COMMANDS)),
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
    pub fn read_next(&self, command: &mut ProcessorCommand) -> bool {
        let Ok(mut commands) = self.commands.try_lock() else {
            return false;
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
            assert!(queue.enqueue_add(std::ptr::null_mut()));
        }
        assert!(!queue.enqueue_add(std::ptr::null_mut()));
        assert_eq!(queue.rejected_count(), 1);
    }

    #[test]
    fn read_next_skips_a_callback_when_a_producer_holds_the_cpp_mutex() {
        let queue = ProcessorCommandQueue::new();
        assert!(queue.enqueue_add(std::ptr::null_mut()));
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
