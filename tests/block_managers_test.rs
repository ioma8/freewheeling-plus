use freewheeling_plus::block::AudioBlock;
use freewheeling_plus::block_managers::*;
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
    assert_eq!(out.peaks.samples, vec![3.0, 7.0, 0.0]);
    assert_eq!(out.avgs.samples, vec![1.5, 3.5, 0.0]);
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
