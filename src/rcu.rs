//! Realtime thread registration for RCU-discipline synchronization.
//!
//! In the original design this module also held a full RCU implementation
//! (`Rcu<T>`, `RcuReader`, spin-wait synchronize) — none of which was ever
//! instantiated in production code.  Only the registry survived, used by
//! [`native_runtime`](crate::native_runtime) to mark the main thread as ready.

use std::sync::Mutex;

/// A simple counter-based registry for RCU-participating threads.
///
/// Threads call [`register_current`](Self::register_current) during
/// initialization; the returned slot index tracks the registration order but
/// is not currently consumed by any RCU read-side lock.
pub struct RcuRegistry {
    count: Mutex<usize>,
}

impl RcuRegistry {
    pub fn new() -> Self {
        Self {
            count: Mutex::new(0),
        }
    }

    pub fn register_current(&self) -> Result<usize, &'static str> {
        let mut count = self.count.lock().unwrap();
        *count += 1;
        Ok(*count)
    }
}

impl Default for RcuRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_current_increments() {
        let reg = RcuRegistry::new();
        let a = reg.register_current().unwrap();
        let b = reg.register_current().unwrap();
        assert_eq!(a, 1);
        assert_eq!(b, 2);
    }
}
