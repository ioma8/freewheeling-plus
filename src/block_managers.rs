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
}

pub struct ManagedChain {
    pub block: Option<SharedBlock>,
    pub cursor: usize,
    pub status: ManagedChainStatus,
}
impl ManagedChain {
    pub fn new(block: Option<SharedBlock>) -> Self {
        Self {
            block,
            cursor: 0,
            status: ManagedChainStatus::Running,
        }
    }
    pub fn manage(&mut self) -> bool {
        false
    }
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
    pub fn manage(&mut self) -> bool {
        let b = self.base.block.as_ref().unwrap();
        let mut x = b.write().unwrap();
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
    pub fn manage(&mut self) -> bool {
        let b = self.base.block.as_ref().unwrap().read().unwrap();
        let n = b.total_len();
        let mut o = self.output.write().unwrap();
        o.peaks.samples.clear();
        o.avgs.samples.clear();
        for p in (0..n).step_by(self.chunk_size) {
            let e = (p + self.chunk_size).min(n);
            let v = (p..e)
                .map(|i| b.sample(i).unwrap_or(0.0))
                .collect::<Vec<_>>();
            let lo = v.iter().fold(f32::INFINITY, |a, &x| a.min(x));
            let hi = v.iter().fold(f32::NEG_INFINITY, |a, &x| a.max(x));
            o.peaks.samples.push(hi - lo);
            o.avgs
                .samples
                .push(v.iter().map(|x| x.abs()).sum::<f32>() / v.len() as f32);
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
    pub fn start(&mut self, samples: Vec<f32>) {
        self.input = samples;
        self.done = false;
        self.smooth_end = false;
        self.base.cursor = 0
    }
    pub fn manage(&mut self) -> bool {
        let b = self.base.block.as_ref().unwrap();
        let mut x = b.write().unwrap();
        if !self.done {
            x.samples = self.input.clone();
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
    pub fn manage(&mut self) -> bool {
        let b = self.base.block.as_ref().unwrap().read().unwrap();
        let len = self
            .write_len
            .unwrap_or_else(|| b.total_len())
            .min(b.total_len());
        self.output = (0..len).filter_map(|i| b.sample(i)).collect();
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
        self.done = false;
        self.write_len = None;
    }
}

pub struct HiPriManagedChain {
    pub base: ManagedChain,
    pub trigger: usize,
}
impl HiPriManagedChain {
    pub fn new(trigger: usize, block: SharedBlock) -> Self {
        Self {
            base: ManagedChain::new(Some(block)),
            trigger,
        }
    }
    pub fn trigger(&mut self) {
        self.base.manage();
    }
}
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
    pub fn manage(&mut self) {
        let p = self.base.base.cursor;
        let mut m = self.markers.lock().unwrap();
        m.markers.push(TimeMarker { offset: p, data: 0 });
        self.base.base.cursor = p + 1;
    }
}

pub struct BlockManager {
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
    pub fn add(&self) -> usize {
        let mut m = self.managers.lock().unwrap();
        m.push(ManagedChainStatus::Running);
        m.len() - 1
    }
    pub fn remove(&self, n: usize) {
        if let Some(x) = self.managers.lock().unwrap().get_mut(n) {
            *x = ManagedChainStatus::PendingDelete
        }
    }
    pub fn collect(&self) {
        self.managers
            .lock()
            .unwrap()
            .retain(|x| *x == ManagedChainStatus::Running)
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
