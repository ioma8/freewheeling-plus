//! Bounded single-producer/single-consumer queues for real-time boundaries.
//!
//! The endpoints allocate only when the queue is constructed.  `try_send` and
//! `try_recv` are wait-free and make overload visible through shared counters.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use rtrb::{Consumer, PopError, Producer, PushError, RingBuffer};

#[derive(Debug, Default)]
pub struct QueueMetrics {
    rejected: AtomicU64,
    received: AtomicU64,
    sent: AtomicU64,
}

impl QueueMetrics {
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    pub fn rejected(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct QueueFull<T>(pub T);

pub struct RealtimeSender<T> {
    producer: Producer<T>,
    metrics: Arc<QueueMetrics>,
}

pub struct RealtimeReceiver<T> {
    consumer: Consumer<T>,
    metrics: Arc<QueueMetrics>,
}

pub fn bounded<T>(capacity: usize) -> (RealtimeSender<T>, RealtimeReceiver<T>) {
    assert!(capacity > 0, "real-time queue capacity must be non-zero");
    let (producer, consumer) = RingBuffer::new(capacity);
    let metrics = Arc::new(QueueMetrics::default());
    (
        RealtimeSender {
            producer,
            metrics: Arc::clone(&metrics),
        },
        RealtimeReceiver { consumer, metrics },
    )
}

impl<T> RealtimeSender<T> {
    /// Enqueue without waiting. The original value is returned on overflow so
    /// callers can surface a command rejection or deliberately drop a status.
    pub fn try_send(&mut self, value: T) -> Result<(), QueueFull<T>> {
        match self.producer.push(value) {
            Ok(()) => {
                self.metrics.sent.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(PushError::Full(value)) => {
                self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
                Err(QueueFull(value))
            }
        }
    }

    pub fn metrics(&self) -> &Arc<QueueMetrics> {
        &self.metrics
    }

    pub fn available_slots(&self) -> usize {
        self.producer.slots()
    }
}

impl<T> RealtimeReceiver<T> {
    pub fn try_recv(&mut self) -> Option<T> {
        match self.consumer.pop() {
            Ok(value) => {
                self.metrics.received.fetch_add(1, Ordering::Relaxed);
                Some(value)
            }
            Err(PopError::Empty) => None,
        }
    }

    pub fn metrics(&self) -> &Arc<QueueMetrics> {
        &self.metrics
    }

    pub fn available_items(&self) -> usize {
        self.consumer.slots()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_is_bounded_observable_and_returns_command() {
        let (mut sender, mut receiver) = bounded(2);
        sender.try_send(10).unwrap();
        sender.try_send(20).unwrap();
        assert_eq!(sender.try_send(30), Err(QueueFull(30)));
        assert_eq!(sender.metrics().sent(), 2);
        assert_eq!(sender.metrics().rejected(), 1);
        assert_eq!(receiver.try_recv(), Some(10));
        assert_eq!(receiver.try_recv(), Some(20));
        assert_eq!(receiver.try_recv(), None);
        assert_eq!(receiver.metrics().received(), 2);
    }

    #[test]
    #[should_panic(expected = "capacity must be non-zero")]
    fn zero_capacity_is_rejected_during_setup() {
        let _ = bounded::<u8>(0);
    }
}
