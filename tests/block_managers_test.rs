use freewheeling_plus::block::AudioBlock;
use freewheeling_plus::block_managers::*;
use freewheeling_plus::mem::Preallocated;
use std::sync::{Arc, RwLock};

#[test]
fn grow_recycles_and_peaks_are_incremental_safe() {
    let b = Arc::new(RwLock::new(AudioBlock::new(4)));
    b.write()
        .unwrap()
        .samples
        .copy_from_slice(&[-1.0, 2.0, 3.0, -4.0]);
    let mut grow = GrowChainManager::new(b.clone(), 2);
    grow.manage();
    assert_eq!(b.read().unwrap().total_len(), 6);
    let mut peaks = PeaksAvgsManager::new(b, 2, false);
    peaks.manage();
    let out = peaks.output.read().unwrap();
    // Two peak samples (maximum, then minimum) per chunk, plus the C++
    // manager's mean-absolute average.
    assert_eq!(out.peaks.samples, vec![2.0, -1.0, 3.0, -4.0, 0.0, 0.0]);
    assert_eq!(out.avgs.samples, vec![1.5, 3.5, 0.0]);
}

#[test]
fn peaks_are_recomputed_and_chunk_size_zero_terminates() {
    let b = Arc::new(RwLock::new(AudioBlock::new(2)));
    b.write().unwrap().samples.copy_from_slice(&[1.0, -1.0]);
    let mut peaks = PeaksAvgsManager::new(b.clone(), 2, false);
    peaks.manage();
    assert_eq!(peaks.output.read().unwrap().peaks.samples.len(), 2);
    // A second pass replaces the previous result instead of appending to it.
    peaks.manage();
    assert_eq!(peaks.output.read().unwrap().peaks.samples.len(), 2);

    let mut zero_chunk = PeaksAvgsManager::new(b, 0, false);
    zero_chunk.manage();
    let out = zero_chunk.output.read().unwrap();
    // A zero chunk size is clamped to one sample per chunk, so the loop
    // terminates instead of dividing by zero.
    assert_eq!(out.chunk_size, 1);
    assert_eq!(out.peaks.samples.len(), 4);
}

#[test]
fn recycled_write_manager_does_not_act_on_a_missing_chain() {
    let b = Arc::new(RwLock::new(AudioBlock::new(2)));
    let mut write = BlockWriteManager::new(b);
    write.recycle();
    assert!(write.done);
    write.manage();
    assert!(write.output.is_empty());
}

#[test]
fn manager_handles_survive_collection() {
    let manager = BlockManager::new();
    let first = manager.add();
    let second = manager.add();
    manager.remove(first);
    manager.collect();
    assert_eq!(manager.status(first), Some(ManagedChainStatus::Deleted));
    assert_eq!(manager.status(second), Some(ManagedChainStatus::Running));
    // The released slot is reused instead of shifting the surviving handle.
    assert_eq!(manager.add(), first);
    assert_eq!(manager.status(second), Some(ManagedChainStatus::Running));
}

#[test]
fn stripe_and_io_preserve_lifecycle() {
    let b = Arc::new(RwLock::new(AudioBlock::new(2)));
    let mut read = BlockReadManager::new(b.clone());
    read.start(vec![1.0, -2.0]);
    read.manage();
    let mut write = BlockWriteManager::new(b.clone());
    write.manage();
    assert_eq!(write.output, vec![1.0, -2.0]);
    let mut stripe = StripeBlockManager::new(7, b);
    stripe.manage();
    stripe.manage();
    assert_eq!(stripe.markers.lock().unwrap().count(), 2);
}
