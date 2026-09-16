//! Deferred block-chain maintenance.  The C++ version stores intrusive pointers;
//! Rust keeps the same state machine while making ownership explicit.
use crate::block::{AudioBlock, MarkerPoints, PeaksAvgs, TimeMarker};
use crate::mem::Preallocated;
use std::sync::{Arc, Mutex, RwLock};

pub type SharedBlock = Arc<RwLock<AudioBlock>>;

/// Application callbacks used by the automatic block managers.
///
/// These mirror the C++ callbacks, but pass the Rust-owned chain handle rather
/// than borrowing an intrusive pointer.  The manager keeps its own handle
/// until the operation completes (or the chain is explicitly deleted).
pub trait AutoWriteControl {
    fn get_write_block(&mut self) -> Option<(SharedBlock, usize)>;
}

pub trait AutoReadControl {
    fn get_read_block(&mut self) -> Option<(Vec<f32>, bool)>;
    fn read_complete(&mut self, block: Option<SharedBlock>);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedChainType {
    None,
    GrowChain,
    PeaksAvgs,
    BlockRead,
    BlockWrite,
    HiPri,
    StripeBlock,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedChainStatus {
    Running,
    PendingDelete,
    /// Slot released by [`BlockManager::collect`]; reused by the next
    /// [`BlockManager::add`]. The slot keeps its index so previously handed out
    /// handles stay valid.
    Deleted,
}

pub struct ManagedChain {
    pub block: Option<SharedBlock>,
    pub cursor: usize,
    pub status: ManagedChainStatus,
    /// Countdown shared with [`HiPriManagedChain::run_deferred`].
    deferred_cycles: usize,
}
impl ManagedChain {
    pub fn new(block: Option<SharedBlock>) -> Self {
        Self {
            block,
            cursor: 0,
            status: ManagedChainStatus::Running,
            deferred_cycles: 0,
        }
    }
    /// Base maintenance hook.
    ///
    /// `ManagedChain` owns no work state of its own, so this is inert; every
    /// concrete manager performs its own maintenance in its own `manage`.
    pub fn manage(&mut self) -> bool {
        false
    }
    /// Chain type of the bare handle. Concrete managers report their own type.
    pub fn kind(&self) -> ManagedChainType {
        ManagedChainType::None
    }
    pub fn ref_deleted(&mut self, block: &SharedBlock) -> bool {
        if self.block.as_ref().is_some_and(|b| Arc::ptr_eq(b, block)) {
            self.status = ManagedChainStatus::PendingDelete;
            true
        } else {
            false
        }
    }
}
impl Preallocated for ManagedChain {
    fn recycle(&mut self) {
        self.block = None;
        self.cursor = 0;
        self.status = ManagedChainStatus::Running;
        self.deferred_cycles = 0;
    }
}

pub struct GrowChainManager {
    pub base: ManagedChain,
    pub block_len: usize,
}
impl GrowChainManager {
    pub fn new(block: SharedBlock, block_len: usize) -> Self {
        Self {
            base: ManagedChain::new(Some(block)),
            block_len,
        }
    }
    pub fn kind(&self) -> ManagedChainType {
        ManagedChainType::GrowChain
    }
    /// Append the next link when the chain has grown to its end.
    ///
    /// Returns `false` when there is nothing to do (also for a recycled or
    /// poisoned chain, instead of panicking).
    pub fn manage(&mut self) -> bool {
        let Some(block) = self.base.block.as_ref() else {
            return false;
        };
        let Ok(mut x) = block.write() else {
            return false;
        };
        if x.next.is_none() {
            x.next = Some(Box::new(AudioBlock::new(self.block_len)));
        }
        false
    }
}

impl Preallocated for GrowChainManager {
    fn recycle(&mut self) {
        self.base.recycle();
    }
}

pub struct PeaksAvgsManager {
    pub base: ManagedChain,
    pub chunk_size: usize,
    pub output: Arc<RwLock<PeaksAvgs>>,
    pub grow: bool,
}
impl PeaksAvgsManager {
    pub fn new(block: SharedBlock, chunk_size: usize, grow: bool) -> Self {
        Self {
            base: ManagedChain::new(Some(block)),
            chunk_size: chunk_size.max(1),
            output: Arc::new(RwLock::new(PeaksAvgs {
                peaks: AudioBlock::new(0),
                avgs: AudioBlock::new(0),
                chunk_size: chunk_size.max(1),
            })),
            grow,
        }
    }
    pub fn kind(&self) -> ManagedChainType {
        ManagedChainType::PeaksAvgs
    }
    /// Recompute the display peaks/averages for the whole chain.
    ///
    /// Layout per chunk matches `PeaksAvgsProcessor` (and the C++ manager):
    /// two `peaks` samples (maximum, then minimum) and one `avgs` sample (mean
    /// absolute amplitude). Lock order is chain first, then output; no other
    /// path takes both.
    pub fn manage(&mut self) -> bool {
        let Some(block) = self.base.block.as_ref() else {
            return false;
        };
        let Ok(b) = block.read() else {
            return false;
        };
        let chunk_size = self.chunk_size.max(1);
        let n = b.total_len();
        let chunks = n.div_ceil(chunk_size);
        let Ok(mut o) = self.output.write() else {
            return false;
        };
        o.chunk_size = chunk_size;
        o.peaks.samples.clear();
        o.peaks.samples.reserve(chunks * 2);
        o.avgs.samples.clear();
        o.avgs.samples.reserve(chunks);
        let mut samples = b.samples_iter();
        let mut pos = 0;
        while pos < n {
            let end = (pos + chunk_size).min(n);
            let (mut lo, mut hi, mut sum) = (f32::INFINITY, f32::NEG_INFINITY, 0.0);
            for _ in pos..end {
                let value = samples.next().unwrap_or(0.0);
                lo = lo.min(value);
                hi = hi.max(value);
                sum += value.abs();
            }
            o.peaks.samples.push(hi);
            o.peaks.samples.push(lo);
            o.avgs.samples.push(sum / (end - pos) as f32);
            pos = end;
        }
        false
    }
    pub fn end(&mut self) {
        self.base.status = ManagedChainStatus::PendingDelete
    }
}
impl Preallocated for PeaksAvgsManager {
    fn recycle(&mut self) {
        self.base.recycle();
    }
}

pub struct BlockReadManager {
    pub base: ManagedChain,
    pub input: Vec<f32>,
    pub done: bool,
    pub smooth_end: bool,
}
impl BlockReadManager {
    pub fn new_auto() -> Self {
        Self {
            base: ManagedChain::new(None),
            input: Vec::new(),
            done: true,
            smooth_end: false,
        }
    }
    pub fn new(block: SharedBlock) -> Self {
        Self {
            base: ManagedChain::new(Some(block)),
            input: Vec::new(),
            done: false,
            smooth_end: false,
        }
    }
    pub fn kind(&self) -> ManagedChainType {
        ManagedChainType::BlockRead
    }
    pub fn start(&mut self, samples: Vec<f32>) {
        self.input = samples;
        self.done = false;
        self.smooth_end = false;
        self.base.cursor = 0
    }
    /// Move the pending input into the chain.
    ///
    /// Returns `false` (instead of panicking) when the chain handle is missing
    /// or its lock is poisoned; the input is moved rather than copied so the
    /// audio path does not duplicate whole buffers.
    pub fn manage(&mut self) -> bool {
        let Some(block) = self.base.block.as_ref() else {
            return false;
        };
        let Ok(mut x) = block.write() else {
            return false;
        };
        if !self.done {
            x.samples = std::mem::take(&mut self.input);
            self.done = true;
        }
        false
    }

    pub fn manage_auto<C: AutoReadControl>(&mut self, control: &mut C) -> bool {
        if self.done {
            let Some((samples, smooth_end)) = control.get_read_block() else {
                return true;
            };
            self.base.block = Some(Arc::new(RwLock::new(AudioBlock::new(samples.len()))));
            self.start(samples);
            self.smooth_end = smooth_end;
        } else if self.base.block.is_none() {
            return true;
        }
        self.manage();
        let block = self.base.block.clone();
        control.read_complete(block);
        self.base.block = None;
        self.done = true;
        false
    }
}
impl Preallocated for BlockReadManager {
    fn recycle(&mut self) {
        self.base.recycle();
        self.input.clear();
        self.done = true;
        self.smooth_end = false;
    }
}
pub struct BlockWriteManager {
    pub base: ManagedChain,
    pub output: Vec<f32>,
    pub done: bool,
    pub write_len: Option<usize>,
}
impl BlockWriteManager {
    pub fn new_auto() -> Self {
        Self {
            base: ManagedChain::new(None),
            output: Vec::new(),
            done: true,
            write_len: None,
        }
    }
    pub fn new(block: SharedBlock) -> Self {
        Self {
            base: ManagedChain::new(Some(block)),
            output: Vec::new(),
            done: false,
            write_len: None,
        }
    }
    pub fn kind(&self) -> ManagedChainType {
        ManagedChainType::BlockWrite
    }
    /// Copy the chain into `output`, honoring the requested length.
    ///
    /// `output` always has exactly `len` samples (a sample missing from an
    /// inconsistent chain reads as silence) and keeps its capacity between
    /// calls.
    pub fn manage(&mut self) -> bool {
        let Some(block) = self.base.block.as_ref() else {
            return false;
        };
        let Ok(b) = block.read() else {
            return false;
        };
        let total = b.total_len();
        let len = self.write_len.unwrap_or(total).min(total);
        self.output.clear();
        self.output
            .extend((0..len).map(|i| b.sample(i).unwrap_or(0.0)));
        self.done = true;
        false
    }

    pub fn manage_auto<C: AutoWriteControl>(&mut self, control: &mut C) -> bool {
        if self.base.block.is_none() {
            let Some((block, len)) = control.get_write_block() else {
                return true;
            };
            self.base.block = Some(block);
            self.write_len = Some(len);
            self.done = false;
        } else if self.done {
            return match control.get_write_block() {
                Some((block, len)) => {
                    self.base.block = Some(block);
                    self.write_len = Some(len);
                    self.done = false;
                    self.manage();
                    false
                }
                None => true,
            };
        }
        self.manage();
        self.base.block = None;
        false
    }
}
impl Preallocated for BlockWriteManager {
    fn recycle(&mut self) {
        self.base.recycle();
        self.output.clear();
        // A recycled instance must match `new_auto()`: no chain, nothing left
        // to write, so `manage()` cannot act on a missing block.
        self.done = true;
        self.write_len = None;
    }
}

pub struct HiPriManagedChain {
    pub base: ManagedChain,
    /// Deferred-maintenance interval in cycles: [`Self::run_deferred`] runs the
    /// maintenance when the countdown reaches this value, so `0` means every
    /// call.
    pub trigger: usize,
}
impl HiPriManagedChain {
    pub fn new(trigger: usize, block: SharedBlock) -> Self {
        Self {
            base: ManagedChain::new(Some(block)),
            trigger,
        }
    }
    pub fn kind(&self) -> ManagedChainType {
        ManagedChainType::HiPri
    }
    /// Run the deferred high-priority maintenance for this cycle.
    ///
    /// Returns `true` when the maintenance ran, so callers can tick this every
    /// cycle without re-running it.
    pub fn run_deferred(&mut self) -> bool {
        if self.base.deferred_cycles < self.trigger {
            self.base.deferred_cycles += 1;
            return false;
        }
        self.base.deferred_cycles = 0;
        self.base.manage();
        true
    }
}

/// Upper bound on recorded stripe boundaries: a striped loop never needs more
/// boundaries than the block it marks, and the manager is pooled and reused for
/// the lifetime of the session.
pub const MAX_STRIPE_MARKERS: usize = 4096;

pub struct StripeBlockManager {
    pub base: HiPriManagedChain,
    pub markers: Arc<Mutex<MarkerPoints>>,
}
impl StripeBlockManager {
    pub fn new(trigger: usize, block: SharedBlock) -> Self {
        Self {
            base: HiPriManagedChain::new(trigger, block),
            markers: Arc::new(Mutex::new(MarkerPoints::default())),
        }
    }
    pub fn kind(&self) -> ManagedChainType {
        ManagedChainType::StripeBlock
    }
    /// Record the stripe boundary at the current cursor and advance it.
    ///
    /// The marker list is bounded: managing the same offset again refreshes
    /// that boundary, and once [`MAX_STRIPE_MARKERS`] boundaries exist the
    /// oldest is dropped.
    pub fn manage(&mut self) {
        let cursor = self.base.base.cursor;
        self.base.base.cursor = cursor + 1;
        let mut markers = self
            .markers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = markers
            .markers
            .iter_mut()
            .find(|marker| marker.offset == cursor)
        {
            existing.data = 0;
            return;
        }
        if markers.markers.len() >= MAX_STRIPE_MARKERS {
            markers.markers.remove(0);
        }
        markers.markers.push(TimeMarker {
            offset: cursor,
            data: 0,
        });
    }
}

pub struct BlockManager {
    /// Slot table whose indices are stable handles: released slots are marked
    /// [`ManagedChainStatus::Deleted`] and reused instead of shifting the
    /// remaining entries.
    managers: Mutex<Vec<ManagedChainStatus>>,
}
impl Default for BlockManager {
    fn default() -> Self {
        Self::new()
    }
}
impl BlockManager {
    pub fn new() -> Self {
        Self {
            managers: Mutex::new(Vec::new()),
        }
    }
    /// Add a manager and return a stable handle to it.
    pub fn add(&self) -> usize {
        let mut managers = self
            .managers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match managers
            .iter()
            .position(|status| *status == ManagedChainStatus::Deleted)
        {
            Some(slot) => {
                managers[slot] = ManagedChainStatus::Running;
                slot
            }
            None => {
                managers.push(ManagedChainStatus::Running);
                managers.len() - 1
            }
        }
    }
    /// Mark the manager at `handle` for deletion.
    pub fn remove(&self, handle: usize) {
        if let Some(status) = self
            .managers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get_mut(handle)
        {
            *status = ManagedChainStatus::PendingDelete
        }
    }
    /// Complete pending deletions. Handles stay valid across collection.
    pub fn collect(&self) {
        let mut managers = self
            .managers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for status in managers.iter_mut() {
            if *status == ManagedChainStatus::PendingDelete {
                *status = ManagedChainStatus::Deleted;
            }
        }
    }
    /// Status of the manager behind `handle`.
    pub fn status(&self, handle: usize) -> Option<ManagedChainStatus> {
        self.managers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(handle)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Reader {
        requests: Vec<Vec<f32>>,
        completed: Vec<SharedBlock>,
    }
    impl AutoReadControl for Reader {
        fn get_read_block(&mut self) -> Option<(Vec<f32>, bool)> {
            self.requests.pop().map(|samples| (samples, false))
        }
        fn read_complete(&mut self, block: Option<SharedBlock>) {
            self.completed
                .push(block.expect("read must retain its chain"));
        }
    }

    struct Writer {
        block: Option<SharedBlock>,
        len: usize,
    }
    impl AutoWriteControl for Writer {
        fn get_write_block(&mut self) -> Option<(SharedBlock, usize)> {
            self.block.take().map(|b| (b, self.len))
        }
    }

    #[test]
    fn auto_read_completes_with_owned_managed_chain() {
        let mut manager = BlockReadManager::new_auto();
        let mut control = Reader {
            requests: vec![vec![1.0, -2.0]],
            completed: Vec::new(),
        };
        assert!(!manager.manage_auto(&mut control));
        assert_eq!(
            control.completed[0].read().unwrap().samples,
            vec![1.0, -2.0]
        );
        assert!(manager.manage_auto(&mut control));
    }

    #[test]
    fn auto_write_honors_length_and_releases_chain_between_requests() {
        let block = Arc::new(RwLock::new(AudioBlock::new(3)));
        block
            .write()
            .unwrap()
            .samples
            .copy_from_slice(&[1.0, 2.0, 3.0]);
        let mut control = Writer {
            block: Some(block.clone()),
            len: 2,
        };
        let mut manager = BlockWriteManager::new_auto();
        assert!(!manager.manage_auto(&mut control));
        assert_eq!(manager.output, vec![1.0, 2.0]);
        assert!(manager.manage_auto(&mut control));
        assert_eq!(Arc::strong_count(&block), 1);
    }
}
