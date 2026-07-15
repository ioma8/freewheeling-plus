//! Preallocated production DSP graph for native audio callbacks.

use crate::audioio::{AudioCallback, AudioProcessor};
use crate::fluidsynth::{FluidLiteBackend, FluidSynthBackend, PITCH_BEND_CENTER};
use crate::realtime_queue::{RealtimeReceiver, RealtimeSender, bounded};
use std::cell::UnsafeCell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Legacy pckeyboard addresses are zero-based and extend through 322.
pub const MAX_RUNTIME_LOOPS: usize = 323;
pub const DEFAULT_COMMAND_CAPACITY: usize = 256;
pub const DEFAULT_STATUS_CAPACITY: usize = 64;
pub const DEFAULT_TRANSFER_SLOTS: usize = 8;
/// Number of simultaneously recordable loops.  Loop IDs are metadata slots;
/// sample storage is leased from this bounded pool instead of being reserved
/// for every ID.
pub const DEFAULT_RECORDING_BUFFERS: usize = 8;
/// `AudioBlock::AUDIOBLOCK_DEFAULT_LEN` in the C++ engine.
const AUDIO_BLOCK_FRAMES: usize = 20_000;
/// `FloConfig::NUM_PREALLOCATED_AUDIO_BLOCKS` in the C++ engine. Blocks are
/// the initial realtime-ready set, rather than a permanent aggregate limit.
/// `PreallocatedType::RTNew` asks `MemoryManager` to replenish this many
/// ready `AudioBlock`s on its non-realtime thread as blocks are consumed.
const DEFAULT_AUDIO_BLOCKS: usize = 40;
/// Initial frames represented by the C++ realtime-ready audio-block set.
/// This remains the import/transfer capacity for now; it is deliberately not
/// a lifetime cap for native recordings.
pub const CPP_AUDIO_POOL_FRAMES: usize = AUDIO_BLOCK_FRAMES * DEFAULT_AUDIO_BLOCKS;
/// The C++ memory-manager update ring is independent from the audio callback.
/// Keep enough returned-block slots for all native recording buffers to be
/// erased in one command burst without allocating on the callback thread.
const STORAGE_RETURN_CAPACITY: usize = DEFAULT_AUDIO_BLOCKS * DEFAULT_RECORDING_BUFFERS;
/// Fixed, callback-safe preview resolution for a loop scope in the UI.
pub const WAVEFORM_SAMPLES: usize = 8;
/// Width of the legacy temporary loop-scope strip before it is mapped into a
/// circular display (`lscopewidth` in the C++ video backend).
pub const LOOP_SCOPE_COLUMNS: usize = 320;
/// `FloConfig::loop_peaksavgs_chunksize`.
const PEAK_AVG_CHUNK_FRAMES: usize = 500;
/// The original `PeaksAvgsManager` runs from BlockManager's non-audio
/// management thread.  Until scope scanning has its own Rust worker, keep the
/// compatibility scan strictly bounded so it cannot dominate a 16-frame audio
/// callback.  At 48 kHz this finishes one 500-frame peak chunk in at most
/// eight 64-frame callbacks (about 2.7 ms with a 16-frame device period).
const SCOPE_REFRESH_SAMPLES_PER_CALLBACK: usize = 64;
/// The callback→UI snapshot is fixed-size. It carries the first 320 C++
/// chunks without allocating; a future preallocated variable-length snapshot
/// channel can retain the full 1,600-entry block-pool scope.
pub const MAX_LOOP_SCOPE_CHUNKS: usize = LOOP_SCOPE_COLUMNS;
/// Maximum stereo frames copied for a loop export in one audio callback.
/// This bounds save-related callback work independently of loop duration.
pub const EXPORT_COPY_FRAMES_PER_CALLBACK: usize = 4096;
/// `AudioBlock::Smooth` and `Processor::DEFAULT_SMOOTH_LENGTH` in the C++
/// engine both use this endpoint / restart crossfade length.
const LOOP_SMOOTH_FRAMES: usize = 64;
/// `RecordProcessor::REC_TAIL_LEN`: a synchronised recording that is ended
/// in the second half of a beat continues this far past the next downbeat so
/// PlayProcessor can crossfade its restart without truncating the tail.
const REC_TAIL_FRAMES: usize = 1024;
/// `Loop::MIN_VOL`, used before multiplying a rising loop volume delta.
const LOOP_MIN_GAIN: f32 = 0.01;

// Match C++ AutoLimitProcessor and the shipped basics.xml defaults. This is
// the final shared limiter after all loop, monitor and synth sources mix.
const LIMITER_ATTACK_LENGTH: f32 = 1024.0;
const LIMITER_ADJUST_PERIOD: usize = 64;
const LIMITER_THRESHOLD: f32 = 0.75;
const LIMITER_RELEASE_RATE: f32 = 0.000_020;
const LIMITER_MAX_GAIN: f32 = 1.0;
const METRONOME_HIT_LEN: usize = 800;
const METRONOME_TONE_LEN: usize = 4_400;

/// Startup-owned counterparts of the C++ `FloConfig` limiter/play settings.
/// They are copied into the callback graph before activation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DspSettings {
    pub max_play_volume: f32,
    pub max_limiter_gain: f32,
    pub limiter_threshold: f32,
    pub limiter_release_rate: f32,
}

impl Default for DspSettings {
    fn default() -> Self {
        Self {
            max_play_volume: 0.0,
            max_limiter_gain: LIMITER_MAX_GAIN,
            limiter_threshold: LIMITER_THRESHOLD,
            limiter_release_rate: LIMITER_RELEASE_RATE,
        }
    }
}

fn gcd_u32(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left.max(1)
}

/// `PlayProcessor` starts every pulse-synchronised loop at
/// `(Pulse::GetLongCount_Cur() % Loop::nbeats) * pulse_length + pulse_pos`.
/// Keep the tail retained by a recorded block-chain outside this musical
/// coordinate system: it is used only by the restart fade.
fn pulse_synced_loop_position(
    pulse_frames: u32,
    pulse_position: u32,
    pulse_long_count: u32,
    pulse_beats: u32,
    loop_len: usize,
) -> usize {
    if loop_len == 0 || pulse_beats == 0 {
        return 0;
    }
    let beat = pulse_long_count % pulse_beats;
    ((beat as usize * pulse_frames.max(1) as usize) + pulse_position as usize) % loop_len
}

const TRANSFER_FREE: u8 = 0;
const TRANSFER_CONTROL: u8 = 1;
const TRANSFER_QUEUED: u8 = 2;
const TRANSFER_CALLBACK: u8 = 3;
const TRANSFER_EXPORTED: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmTransferHandle {
    index: u16,
    generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopTransferMetadata {
    pub frames: u32,
    pub position: u32,
    pub mode: LoopMode,
    pub gain: f32,
    pub pulse_frames: u32,
    pub beats: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmTransferError {
    PoolExhausted,
    InvalidHandle,
    PcmTooLong,
    ChannelLengthMismatch,
    CommandQueueFull,
    TransferBusy,
    RecordingStorageExhausted,
}

struct StereoTransfer {
    left: Vec<f32>,
    right: Vec<f32>,
    len: usize,
}

/// Control-thread-only equivalent of the completed C++ `PeaksAvgsManager`
/// output carried alongside imported PCM. Keeping it in the transfer slot
/// prevents a large imported loop from being scanned by the audio callback.
struct TransferScopeCache {
    peaks: [f32; MAX_LOOP_SCOPE_CHUNKS],
    averages: [f32; MAX_LOOP_SCOPE_CHUNKS],
    columns: usize,
}

impl Default for TransferScopeCache {
    fn default() -> Self {
        Self {
            peaks: [0.0; MAX_LOOP_SCOPE_CHUNKS],
            averages: [0.0; MAX_LOOP_SCOPE_CHUNKS],
            columns: 0,
        }
    }
}

impl TransferScopeCache {
    fn compute(&mut self, left: &[f32], right: &[f32]) {
        *self = Self::default();
        for (chunk, (left, right)) in left
            .chunks_exact(PEAK_AVG_CHUNK_FRAMES)
            .zip(right.chunks_exact(PEAK_AVG_CHUNK_FRAMES))
            .take(MAX_LOOP_SCOPE_CHUNKS)
            .enumerate()
        {
            let mut maximum = 0.0_f32;
            let mut minimum = 0.0_f32;
            let mut tally = 0.0_f32;
            for (&left, &right) in left.iter().zip(right) {
                maximum = maximum.max(left).max(right);
                minimum = minimum.min(left).min(right);
                tally += (left.abs() + right.abs()) * 0.5;
            }
            self.peaks[chunk] = maximum - minimum;
            self.averages[chunk] = tally / PEAK_AVG_CHUNK_FRAMES as f32;
            self.columns = chunk + 1;
        }
    }
}

/// One native-recording `AudioBlock`. C++ links these nodes instead of
/// growing a callback-owned vector, which lets a recording grow for as long
/// as the non-realtime memory manager can provide blocks.
struct LoopStorageBlock {
    storage: StereoTransfer,
    next: Option<Box<LoopStorageBlock>>,
}

/// Callback-safe counterpart to C++'s `AudioBlock::first`/`next` chain.
/// Linking/unlinking an already allocated node performs no allocation.
struct LoopBlockChain {
    first: Option<Box<LoopStorageBlock>>,
    /// Stable because every node is individually heap allocated. This mirrors
    /// `AudioBlockIterator`/`AudioBlock::Link`'s O(1) chain extension rather
    /// than scanning the existing recording on the audio callback.
    tail: Option<usize>,
    count: usize,
}

impl LoopBlockChain {
    fn is_empty(&self) -> bool {
        self.first.is_none()
    }

    fn len(&self) -> usize {
        self.count
    }

    fn append(&mut self, mut block: Box<LoopStorageBlock>) {
        block.next = None;
        let tail = (&mut *block as *mut LoopStorageBlock) as usize;
        if let Some(previous) = self.tail {
            // SAFETY: `previous` points to a node owned exclusively by this
            // chain. Nodes are boxed and never moved; `&mut self` prevents
            // concurrent mutation while the tail link is written.
            unsafe { (*(previous as *mut LoopStorageBlock)).next = Some(block) };
        } else {
            self.first = Some(block);
        }
        self.tail = Some(tail);
        self.count += 1;
    }

    fn pop_first(&mut self) -> Option<Box<LoopStorageBlock>> {
        let mut block = self.first.take()?;
        self.first = block.next.take();
        self.count -= 1;
        if self.count == 0 {
            self.tail = None;
        }
        Some(block)
    }

    fn block_at(&self, index: usize) -> &LoopStorageBlock {
        let mut current = self.first.as_deref();
        for _ in 0..index {
            current = current.and_then(|block| block.next.as_deref());
        }
        current.expect("recording block chain shorter than loop length")
    }

    fn block_at_mut(&mut self, index: usize) -> &mut LoopStorageBlock {
        let mut current = self.first.as_deref_mut();
        for _ in 0..index {
            current = current.and_then(|block| block.next.as_deref_mut());
        }
        current.expect("recording block chain shorter than loop length")
    }
}

impl Default for LoopBlockChain {
    fn default() -> Self {
        Self {
            first: None,
            tail: None,
            count: 0,
        }
    }
}

struct TransferSlot {
    state: AtomicU8,
    generation: AtomicU32,
    pcm: UnsafeCell<StereoTransfer>,
    scope: UnsafeCell<TransferScopeCache>,
}

// State transitions give one side exclusive access to each UnsafeCell: control
// owns CONTROL/EXPORTED and the audio thread owns QUEUED/CALLBACK.
unsafe impl Sync for TransferSlot {}

struct TransferPool {
    slots: Box<[TransferSlot]>,
    capacity: usize,
}

impl TransferPool {
    fn new(count: usize, capacity: usize) -> Self {
        let slots = (0..count)
            .map(|_| TransferSlot {
                state: AtomicU8::new(TRANSFER_FREE),
                generation: AtomicU32::new(0),
                pcm: UnsafeCell::new(StereoTransfer {
                    left: vec![0.0; capacity],
                    right: vec![0.0; capacity],
                    len: 0,
                }),
                scope: UnsafeCell::new(TransferScopeCache::default()),
            })
            .collect();
        Self { slots, capacity }
    }

    fn slot(&self, handle: PcmTransferHandle) -> Option<&TransferSlot> {
        let slot = self.slots.get(handle.index as usize)?;
        (slot.generation.load(Ordering::Acquire) == handle.generation).then_some(slot)
    }

    fn acquire(&self) -> Result<PcmTransferHandle, PcmTransferError> {
        for (index, slot) in self.slots.iter().enumerate() {
            if slot
                .state
                .compare_exchange(
                    TRANSFER_FREE,
                    TRANSFER_CONTROL,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                let generation = slot
                    .generation
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1);
                return Ok(PcmTransferHandle {
                    index: index as u16,
                    generation,
                });
            }
        }
        Err(PcmTransferError::PoolExhausted)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RuntimeCommand {
    Record {
        slot: u8,
    },
    Overdub {
        slot: u8,
        feedback: f32,
        gain: f32,
    },
    StopRecord,
    Trigger {
        slot: u8,
        gain: f32,
    },
    SetTriggerGain {
        slot: u8,
        gain: f32,
    },
    SetLoopGain {
        slot: u8,
        gain: f32,
    },
    AdjustLoopGain {
        slot: u8,
        factor: f32,
    },
    AdjustLoopGainDelta {
        slot: u8,
        amount: f32,
    },
    ResetLoopGainDeltas,
    MoveLoop {
        from: u8,
        to: u8,
    },
    Mute {
        slot: u8,
        muted: bool,
    },
    Erase {
        slot: u8,
    },
    SetInputMonitor(f32),
    AdjustInputMonitor(f32),
    SetMasterGain(f32),
    AdjustMasterGain(f32),
    SetPulse {
        frames: u32,
    },
    /// C++ `LoopManager::SetSubdivide`: affects the next pulse created from
    /// a loop, while leaving an already-created pulse untouched.
    SetPulseSubdivide {
        beats: u32,
    },
    SetPulseFromLoop {
        slot: u8,
    },
    ClearPulse,
    SetMetronome {
        enabled: bool,
        gain: f32,
    },
    SynthNote {
        note: u8,
        velocity: u8,
    },
    SynthOff,
    SetSynthEnabled(bool),
    /// Configured `FloConfig::fschannel`, used by C++ for synth MIDI input.
    SetSynthChannel(u8),
    /// Configured `FloConfig::fsstereo`; mono folds synth R into L and does
    /// not contribute a right external-input channel.
    SetSynthStereo(bool),
    SynthController {
        channel: u8,
        control: u8,
        value: u8,
    },
    SynthPitchBend {
        channel: u8,
        value: u16,
    },
    SynthPatch {
        channel: u8,
        soundfont_id: i32,
        bank: i32,
        program: i32,
    },
    SynthTuning {
        cents: f32,
    },
    ImportLoop {
        slot: u8,
        handle: PcmTransferHandle,
        position: u32,
        mode: LoopMode,
        gain: f32,
    },
    RequestLoopExport {
        slot: u8,
        replacement: PcmTransferHandle,
    },
    RequestSnapshot,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopMode {
    #[default]
    Empty,
    Recording,
    Overdubbing,
    Playing,
    Muted,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuntimeLoopSnapshot {
    pub mode: LoopMode,
    pub frames: u32,
    pub position: u32,
    pub gain: f32,
    pub trigger_gain: f32,
    pub gain_delta: f32,
    pub waveform: [f32; WAVEFORM_SAMPLES],
}

impl Default for RuntimeLoopSnapshot {
    fn default() -> Self {
        Self {
            mode: LoopMode::Empty,
            frames: 0,
            position: 0,
            gain: 0.0,
            trigger_gain: 0.0,
            gain_delta: 1.0,
            waveform: [0.0; WAVEFORM_SAMPLES],
        }
    }
}

/// Quantized C++ `PeaksAvgsManager` columns for one active loop scope. Peak
/// is the stereo sample range (`max - min`); average is mean absolute stereo
/// amplitude. The renderer consumes those two signals before bending its flat
/// strip into a ring. Eight entries cover the visible scope snapshot without
/// making every one of the 323 address slots carry video memory.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuntimeScopePreview {
    pub loop_id: u8,
    pub position_column: u16,
    pub chunk_count: u16,
    pub current_peak: f32,
    pub peaks: [f32; MAX_LOOP_SCOPE_CHUNKS],
    pub averages: [f32; MAX_LOOP_SCOPE_CHUNKS],
}

impl Default for RuntimeScopePreview {
    fn default() -> Self {
        Self {
            loop_id: u8::MAX,
            position_column: 0,
            chunk_count: 0,
            current_peak: 0.0,
            peaks: [0.0; MAX_LOOP_SCOPE_CHUNKS],
            averages: [0.0; MAX_LOOP_SCOPE_CHUNKS],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuntimeSnapshot {
    pub sequence: u64,
    pub sample_clock: u64,
    pub pulse_position: u32,
    pub pulse_frames: u32,
    pub pulse_long_count: u32,
    pub pulse_long_length: u32,
    pub recording_slot: i16,
    pub input_peak: [f32; 2],
    pub output_peak: [f32; 2],
    pub monitor_gain: f32,
    pub master_gain: f32,
    pub limiter_gain: f32,
    pub scope_count: u8,
    pub scopes: [RuntimeScopePreview; DEFAULT_RECORDING_BUFFERS],
    pub loops: [RuntimeLoopSnapshot; MAX_RUNTIME_LOOPS],
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            sequence: 0,
            sample_clock: 0,
            pulse_position: 0,
            pulse_frames: 1,
            pulse_long_count: 0,
            pulse_long_length: 1,
            recording_slot: -1,
            input_peak: [0.0; 2],
            output_peak: [0.0; 2],
            monitor_gain: 0.0,
            master_gain: 1.0,
            limiter_gain: 1.0,
            scope_count: 0,
            scopes: [RuntimeScopePreview::default(); DEFAULT_RECORDING_BUFFERS],
            loops: [RuntimeLoopSnapshot::default(); MAX_RUNTIME_LOOPS],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
// Keeping the snapshot inline makes status publication allocation-free. The
// bounded ring pays this storage cost once during construction.
#[allow(clippy::large_enum_variant)]
pub enum RuntimeStatus {
    Snapshot(RuntimeSnapshot),
    CommandRejected(RuntimeCommand),
    RecordingFull {
        slot: u8,
    },
    LoopCompleted {
        slot: u8,
    },
    LoopImported {
        slot: u8,
        handle: PcmTransferHandle,
    },
    LoopExported {
        slot: u8,
        handle: PcmTransferHandle,
        metadata: LoopTransferMetadata,
    },
    TransferError {
        slot: u8,
        handle: PcmTransferHandle,
        error: PcmTransferError,
    },
    ShutdownComplete,
}

pub struct RuntimeControls {
    commands: RealtimeSender<RuntimeCommand>,
    statuses: RealtimeReceiver<RuntimeStatus>,
    transfers: Arc<TransferPool>,
    loop_storage_refiller: LoopStorageRefiller,
}

impl RuntimeControls {
    pub fn try_command(&mut self, command: RuntimeCommand) -> Result<(), RuntimeCommand> {
        self.commands.try_send(command).map_err(|full| full.0)
    }

    pub fn try_status(&mut self) -> Option<RuntimeStatus> {
        self.statuses.try_recv()
    }

    pub fn rejected_commands(&self) -> u64 {
        self.commands.metrics().rejected()
    }

    /// Nudge the dedicated non-realtime block manager. The worker normally
    /// wakes directly from the callback; this lets the application loop make
    /// an immediate progress request without ever allocating on audio.
    pub fn service_loop_storage(&mut self) {
        self.loop_storage_refiller.service();
    }

    pub fn try_acquire_transfer(&self) -> Result<PcmTransferHandle, PcmTransferError> {
        self.transfers.acquire()
    }

    pub fn write_transfer(
        &self,
        handle: PcmTransferHandle,
        left: &[f32],
        right: &[f32],
    ) -> Result<(), PcmTransferError> {
        if left.len() != right.len() {
            return Err(PcmTransferError::ChannelLengthMismatch);
        }
        let slot = self
            .transfers
            .slot(handle)
            .filter(|slot| slot.state.load(Ordering::Acquire) == TRANSFER_CONTROL)
            .ok_or(PcmTransferError::InvalidHandle)?;
        // SAFETY: CONTROL is exclusively owned by this endpoint.
        let pcm = unsafe { &mut *slot.pcm.get() };
        if left.len() > pcm.left.len() {
            return Err(PcmTransferError::PcmTooLong);
        }
        pcm.left[..left.len()].copy_from_slice(left);
        pcm.right[..right.len()].copy_from_slice(right);
        pcm.len = left.len();
        Ok(())
    }

    pub fn try_import_loop(
        &mut self,
        slot: u8,
        handle: PcmTransferHandle,
        position: u32,
        mode: LoopMode,
        gain: f32,
    ) -> Result<(), PcmTransferError> {
        let transfer = self
            .transfers
            .slot(handle)
            .ok_or(PcmTransferError::InvalidHandle)?;
        let state = transfer.state.load(Ordering::Acquire);
        if state != TRANSFER_CONTROL && state != TRANSFER_EXPORTED {
            return Err(PcmTransferError::InvalidHandle);
        }
        // SAFETY: CONTROL/EXPORTED grants this endpoint exclusive PCM and
        // scope ownership. This is the C++ BlockManager-side peak pass, kept
        // off the audio callback before queueing the import.
        let pcm = unsafe { &*transfer.pcm.get() };
        let scope = unsafe { &mut *transfer.scope.get() };
        scope.compute(&pcm.left[..pcm.len], &pcm.right[..pcm.len]);
        transfer
            .state
            .compare_exchange(state, TRANSFER_QUEUED, Ordering::AcqRel, Ordering::Relaxed)
            .map_err(|_| PcmTransferError::InvalidHandle)?;
        let command = RuntimeCommand::ImportLoop {
            slot,
            handle,
            position,
            mode,
            gain,
        };
        if self.commands.try_send(command).is_err() {
            transfer.state.store(TRANSFER_CONTROL, Ordering::Release);
            return Err(PcmTransferError::CommandQueueFull);
        }
        Ok(())
    }

    pub fn try_request_loop_export(
        &mut self,
        slot: u8,
    ) -> Result<PcmTransferHandle, PcmTransferError> {
        let replacement = self.transfers.acquire()?;
        let transfer = self.transfers.slot(replacement).unwrap();
        transfer.state.store(TRANSFER_QUEUED, Ordering::Release);
        if self
            .commands
            .try_send(RuntimeCommand::RequestLoopExport { slot, replacement })
            .is_err()
        {
            transfer.state.store(TRANSFER_FREE, Ordering::Release);
            return Err(PcmTransferError::CommandQueueFull);
        }
        Ok(replacement)
    }

    pub fn with_exported_pcm<R>(
        &self,
        handle: PcmTransferHandle,
        read: impl FnOnce(&[f32], &[f32]) -> R,
    ) -> Result<R, PcmTransferError> {
        let slot = self
            .transfers
            .slot(handle)
            .filter(|slot| slot.state.load(Ordering::Acquire) == TRANSFER_EXPORTED)
            .ok_or(PcmTransferError::InvalidHandle)?;
        // SAFETY: EXPORTED is exclusively read by control until release.
        let pcm = unsafe { &*slot.pcm.get() };
        Ok(read(&pcm.left[..pcm.len], &pcm.right[..pcm.len]))
    }

    pub fn release_transfer(&self, handle: PcmTransferHandle) -> Result<(), PcmTransferError> {
        let slot = self
            .transfers
            .slot(handle)
            .ok_or(PcmTransferError::InvalidHandle)?;
        let state = slot.state.load(Ordering::Acquire);
        if state != TRANSFER_CONTROL && state != TRANSFER_EXPORTED {
            return Err(PcmTransferError::InvalidHandle);
        }
        // Import transfers lend their contiguous vectors to the live loop in
        // the audio callback. Rebuild an empty returned slot here, on the
        // control thread, before making it acquirable again. Otherwise the
        // next export can select a zero-capacity slot and panic in the
        // callback.
        // SAFETY: CONTROL/EXPORTED is exclusively owned by this endpoint.
        let pcm = unsafe { &mut *slot.pcm.get() };
        if pcm.left.len() < self.transfers.capacity {
            pcm.left.resize(self.transfers.capacity, 0.0);
        }
        if pcm.right.len() < self.transfers.capacity {
            pcm.right.resize(self.transfers.capacity, 0.0);
        }
        pcm.len = 0;
        slot.state
            .compare_exchange(state, TRANSFER_FREE, Ordering::AcqRel, Ordering::Relaxed)
            .map(|_| ())
            .map_err(|_| PcmTransferError::InvalidHandle)
    }
}

struct LoopSlot {
    left: Vec<f32>,
    right: Vec<f32>,
    /// C++ records into a chain of globally preallocated `AudioBlock`s. Rust
    /// imports retain their contiguous transfer buffer in `left`/`right`, but
    /// native recordings use this callback-safe shared block chain.
    blocks: LoopBlockChain,
    /// Logical beginning of this loop within its retained storage. C++
    /// `AudioBlock::Smooth(1)` advances `first->buf` by 64 samples instead
    /// of copying the whole chain; retain the same O(1) logical trim.
    data_offset: usize,
    len: usize,
    position: usize,
    mode: LoopMode,
    gain: f32,
    trigger_gain: f32,
    gain_delta: f32,
    feedback: f32,
    /// A loop recorded against the selected pulse uses PlayProcessor's
    /// restart crossfade instead of permanently smoothing its endpoint.
    pulse_synced: bool,
    /// C++ `Loop::nbeats`, retained separately because a synchronised
    /// recording can carry a post-downbeat crossfade tail beyond its musical
    /// period.
    pulse_beats: u32,
    /// Offset within the 64-frame C++ `fadepreandcurrent` equivalent after a
    /// pulse-synchronised loop wraps. `None` means no restart fade is active.
    boundary_fade_position: Option<usize>,
    /// C++ `RecordProcessor::End()` keeps an overdub processor alive for one
    /// final callback and fades input out while returning the feedback path to
    /// unity. `(progress, total)` is established on that callback.
    overdub_fade_out: Option<(usize, usize)>,
    /// `RecordProcessor::Jump` preserves the previous fragment, then on the
    /// next callback fades that old location out while fading the new pulse
    /// location in. This fixed cache is established before audio starts and
    /// is never allocated or resized on the callback.
    overdub_jump: Box<OverdubJumpCache>,
    recent_peak: f32,
    /// Incrementally maintained equivalent of the C++ PeaksAvgsManager
    /// display strip. Keeping this state in the loop makes Snapshot a bounded
    /// metadata copy rather than a long-loop scan on the audio callback.
    scope: Box<LoopScopeCache>,
}

#[derive(Clone, Copy)]
struct ExportJob {
    slot: u8,
    handle: PcmTransferHandle,
    cursor: usize,
    metadata: LoopTransferMetadata,
}

impl LoopSlot {
    fn new() -> Self {
        Self {
            left: Vec::new(),
            right: Vec::new(),
            blocks: LoopBlockChain::default(),
            data_offset: 0,
            len: 0,
            position: 0,
            mode: LoopMode::Empty,
            gain: 1.0,
            trigger_gain: 1.0,
            gain_delta: 1.0,
            feedback: 0.5,
            pulse_synced: false,
            pulse_beats: 0,
            boundary_fade_position: None,
            overdub_fade_out: None,
            overdub_jump: Box::default(),
            recent_peak: 0.0,
            scope: Box::default(),
        }
    }

    fn uses_blocks(&self) -> bool {
        !self.blocks.is_empty()
    }

    fn capacity(&self) -> usize {
        if self.uses_blocks() {
            self.blocks.len() * AUDIO_BLOCK_FRAMES
        } else {
            self.left.len()
        }
        .saturating_sub(self.data_offset)
    }

    fn sample_at(&self, frame: usize) -> (f32, f32) {
        let frame = frame + self.data_offset;
        if self.uses_blocks() {
            let block = self.blocks.block_at(frame / AUDIO_BLOCK_FRAMES);
            let offset = frame % AUDIO_BLOCK_FRAMES;
            (block.storage.left[offset], block.storage.right[offset])
        } else {
            (self.left[frame], self.right[frame])
        }
    }

    fn set_sample(&mut self, frame: usize, left: f32, right: f32) {
        let frame = frame + self.data_offset;
        if self.uses_blocks() {
            let block = self.blocks.block_at_mut(frame / AUDIO_BLOCK_FRAMES);
            let offset = frame % AUDIO_BLOCK_FRAMES;
            block.storage.left[offset] = left;
            block.storage.right[offset] = right;
        } else {
            self.left[frame] = left;
            self.right[frame] = right;
        }
    }

    fn copy_range_to(&self, start: usize, left: &mut [f32], right: &mut [f32]) {
        for (offset, (out_left, out_right)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            (*out_left, *out_right) = self.sample_at(start + offset);
        }
    }

    /// Write-side counterpart to C++ `PeaksAvgsManager::Manage`. Recording
    /// already visits each sample, so this keeps complete 500-frame scope
    /// chunks without a second callback-time scan.
    fn record_scope_sample(&mut self, left: f32, right: f32) {
        let scope = &mut self.scope;
        if scope.complete || scope.column >= MAX_LOOP_SCOPE_CHUNKS {
            return;
        }
        scope.maximum = scope.maximum.max(left).max(right);
        scope.minimum = scope.minimum.min(left).min(right);
        scope.absolute_tally += (left.abs() + right.abs()) * 0.5;
        scope.sample += 1;
        if scope.sample == PEAK_AVG_CHUNK_FRAMES {
            scope.peaks[scope.column] = scope.maximum - scope.minimum;
            scope.averages[scope.column] = scope.absolute_tally / PEAK_AVG_CHUNK_FRAMES as f32;
            scope.column += 1;
            scope.sample = 0;
            scope.maximum = 0.0;
            scope.minimum = 0.0;
            scope.absolute_tally = 0.0;
        }
    }
}

#[derive(Clone)]
struct OverdubJumpCache {
    positions: [usize; LOOP_SMOOTH_FRAMES],
    left: [f32; LOOP_SMOOTH_FRAMES],
    right: [f32; LOOP_SMOOTH_FRAMES],
    fade_positions: [usize; LOOP_SMOOTH_FRAMES],
    fade_left: [f32; LOOP_SMOOTH_FRAMES],
    fade_right: [f32; LOOP_SMOOTH_FRAMES],
    count: usize,
    next: usize,
    /// `Some(progress)` after a quantised `RecordProcessor::Jump`.
    fade_position: Option<usize>,
}

impl Default for OverdubJumpCache {
    fn default() -> Self {
        Self {
            positions: [0; LOOP_SMOOTH_FRAMES],
            left: [0.0; LOOP_SMOOTH_FRAMES],
            right: [0.0; LOOP_SMOOTH_FRAMES],
            fade_positions: [0; LOOP_SMOOTH_FRAMES],
            fade_left: [0.0; LOOP_SMOOTH_FRAMES],
            fade_right: [0.0; LOOP_SMOOTH_FRAMES],
            count: 0,
            next: 0,
            fade_position: None,
        }
    }
}

impl OverdubJumpCache {
    fn reset(&mut self) {
        self.count = 0;
        self.next = 0;
        self.fade_position = None;
    }

    fn push(&mut self, position: usize, left: f32, right: f32) {
        self.positions[self.next] = position;
        self.left[self.next] = left;
        self.right[self.next] = right;
        self.next = (self.next + 1) % LOOP_SMOOTH_FRAMES;
        self.count = (self.count + 1).min(LOOP_SMOOTH_FRAMES);
    }

    fn ordered_index(&self, offset: usize) -> usize {
        debug_assert!(offset < self.count);
        (self.next + LOOP_SMOOTH_FRAMES - self.count + offset) % LOOP_SMOOTH_FRAMES
    }

    fn begin_fade(&mut self) {
        for offset in 0..self.count {
            let source = self.ordered_index(offset);
            self.fade_positions[offset] = self.positions[source];
            self.fade_left[offset] = self.left[source];
            self.fade_right[offset] = self.right[source];
        }
        self.fade_position = (self.count != 0).then_some(0);
    }
}

/// Startup-owned scope state. Boxing it keeps the fixed 323-loop realtime
/// processor compact enough for normal audio/test thread stacks.
struct LoopScopeCache {
    peaks: [f32; MAX_LOOP_SCOPE_CHUNKS],
    averages: [f32; MAX_LOOP_SCOPE_CHUNKS],
    column: usize,
    sample: usize,
    maximum: f32,
    minimum: f32,
    absolute_tally: f32,
    complete: bool,
}

impl Default for LoopScopeCache {
    fn default() -> Self {
        Self {
            peaks: [0.0; MAX_LOOP_SCOPE_CHUNKS],
            averages: [0.0; MAX_LOOP_SCOPE_CHUNKS],
            column: 0,
            sample: 0,
            maximum: 0.0,
            minimum: 0.0,
            absolute_tally: 0.0,
            complete: false,
        }
    }
}

impl LoopScopeCache {
    /// Reset the cache in place. Replacing its `Box` while importing PCM
    /// would allocate in the audio callback.
    fn reset(&mut self) {
        self.peaks.fill(0.0);
        self.averages.fill(0.0);
        self.column = 0;
        self.sample = 0;
        self.maximum = 0.0;
        self.minimum = 0.0;
        self.absolute_tally = 0.0;
        self.complete = false;
    }
}

/// Callback-owned end of the C++ `PreallocatedType<AudioBlock>` path.  Its
/// vectors/rings have fixed capacity before activation; consuming or returning
/// a block only moves an already allocated `StereoTransfer`.
struct LoopStoragePool {
    free: Vec<Box<LoopStorageBlock>>,
    refills: RealtimeReceiver<Box<LoopStorageBlock>>,
    returned: RealtimeSender<Box<LoopStorageBlock>>,
    requests: Arc<AtomicUsize>,
    worker: thread::Thread,
}

/// Dedicated non-realtime `MemoryManager` counterpart. The C++ implementation
/// starts with 40 ready instances and creates replacements asynchronously
/// whenever `RTNew` consumes one. This worker has the same ownership boundary:
/// it alone may allocate `AudioBlock` storage, while the callback merely moves
/// it through bounded SPSC rings.
struct LoopStorageRefiller {
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl LoopStoragePool {
    fn new() -> (Self, LoopStorageRefiller) {
        let (refill_tx, refill_rx) = bounded(DEFAULT_AUDIO_BLOCKS);
        let (return_tx, return_rx) = bounded(STORAGE_RETURN_CAPACITY);
        let requests = Arc::new(AtomicUsize::new(0));
        let worker_requests = Arc::clone(&requests);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let worker = thread::Builder::new()
            .name("loop-storage".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let mut refills = refill_tx;
                let mut returned = return_rx;
                let mut recycled = Vec::new();
                while !worker_stopping.load(Ordering::Acquire) {
                    refill_loop_storage(
                        &mut refills,
                        &mut returned,
                        &mut recycled,
                        &worker_requests,
                    );
                    thread::park_timeout(Duration::from_millis(1));
                }
                refill_loop_storage(&mut refills, &mut returned, &mut recycled, &worker_requests);
            })
            .expect("failed to create loop storage manager");
        let worker_thread = worker.thread().clone();
        (
            Self {
                free: (0..DEFAULT_AUDIO_BLOCKS)
                    .map(|_| {
                        Box::new(LoopStorageBlock {
                            storage: StereoTransfer {
                                left: vec![0.0; AUDIO_BLOCK_FRAMES],
                                right: vec![0.0; AUDIO_BLOCK_FRAMES],
                                len: 0,
                            },
                            next: None,
                        })
                    })
                    .collect(),
                refills: refill_rx,
                returned: return_tx,
                requests,
                worker: worker_thread,
            },
            LoopStorageRefiller {
                stopping,
                worker: Some(worker),
            },
        )
    }

    fn collect_refills(&mut self) {
        // Pull all currently ready blocks in one bounded pass. `free` starts
        // with 40 entries and can hold the 40-entry refill ring without a
        // callback-time reallocation.
        while self.free.len() < self.free.capacity() {
            let Some(storage) = self.refills.try_recv() else {
                break;
            };
            self.free.push(storage);
        }
    }

    fn add_block(&mut self, slot: &mut LoopSlot) -> bool {
        self.collect_refills();
        let Some(storage) = self.free.pop() else {
            return false;
        };
        self.requests.fetch_add(1, Ordering::Release);
        self.worker.unpark();
        slot.blocks.append(storage);
        true
    }

    fn release_blocks(&mut self, slot: &mut LoopSlot) {
        while let Some(storage) = slot.blocks.pop_first() {
            // The return ring is intentionally much larger than C++'s
            // realtime-ready set, so an erase/re-record command burst stays
            // allocation-free and keeps ownership off the callback thread.
            if let Err(full) = self.returned.try_send(storage) {
                // This is equivalent to C++ exhausting MemoryManager's update
                // queue: retaining the block locally is safer than freeing or
                // allocating from audio. The next recording may be refused
                // until the control loop catches up.
                slot.blocks.append(full.0);
                break;
            }
            self.worker.unpark();
        }
    }
}

impl LoopStorageRefiller {
    fn service(&mut self) {
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
}

impl Drop for LoopStorageRefiller {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn refill_loop_storage(
    refills: &mut RealtimeSender<Box<LoopStorageBlock>>,
    returned: &mut RealtimeReceiver<Box<LoopStorageBlock>>,
    recycled: &mut Vec<Box<LoopStorageBlock>>,
    requests: &AtomicUsize,
) {
    while let Some(storage) = returned.try_recv() {
        recycled.push(storage);
    }
    let requested = requests.swap(0, Ordering::AcqRel);
    for remaining in (0..requested).rev() {
        let storage = recycled.pop().unwrap_or_else(|| {
            Box::new(LoopStorageBlock {
                storage: StereoTransfer {
                    left: vec![0.0; AUDIO_BLOCK_FRAMES],
                    right: vec![0.0; AUDIO_BLOCK_FRAMES],
                    len: 0,
                },
                next: None,
            })
        });
        if let Err(full) = refills.try_send(storage) {
            recycled.push(full.0);
            requests.fetch_add(remaining + 1, Ordering::Release);
            break;
        }
    }
}

/// Port of `AudioBlock::Smooth(smoothtype = 1)`, used by the C++ recorder for
/// a loop that was not tied to a Pulse. It blends the beginning into the final
/// 64 frames in place, so every later wrap is continuous without per-callback
/// work.
fn smooth_unsynchronised_loop_endpoints(slot: &mut LoopSlot) {
    if slot.len < LOOP_SMOOTH_FRAMES {
        return;
    }
    let last_start = slot.len - LOOP_SMOOTH_FRAMES;
    for offset in 0..LOOP_SMOOTH_FRAMES {
        let mix = offset as f32 / LOOP_SMOOTH_FRAMES as f32;
        let (start_left, start_right) = slot.sample_at(offset);
        let (end_left, end_right) = slot.sample_at(last_start + offset);
        slot.set_sample(
            last_start + offset,
            mix * start_left + (1.0 - mix) * end_left,
            mix * start_right + (1.0 - mix) * end_right,
        );
    }
    // `AudioBlock::Smooth(1)` advances its first buffer pointer and shortens
    // the logical chain by the head that was blended into the tail.
    slot.data_offset += LOOP_SMOOTH_FRAMES;
    slot.len -= LOOP_SMOOTH_FRAMES;
}

/// Port of `Loop::UpdateVolume`.  It runs once per active PlayProcessor or
/// overdubbing RecordProcessor callback, before that processor renders.
fn update_loop_gain(slot: &mut LoopSlot) {
    if slot.gain_delta != 1.0 {
        if slot.gain_delta > 1.0 && slot.gain < LOOP_MIN_GAIN {
            slot.gain = LOOP_MIN_GAIN;
        }
        slot.gain *= slot.gain_delta;
    }
}

fn capped_loop_gain(slot: &LoopSlot, max_play_volume: f32) -> f32 {
    let gain = slot.gain * slot.trigger_gain;
    if max_play_volume > 0.0 {
        gain.min(max_play_volume)
    } else {
        gain.max(0.0)
    }
}

/// Final, stereo-linked master limiter.
///
/// This intentionally follows `AutoLimitProcessor` in the C++ engine rather
/// than using a per-sample soft clip.  In particular, the peak detector sees
/// the unscaled mix, both channels share one gain envelope, and gain reduction
/// attacks over 1024 frames before the very last 0.99 safety guard.
struct MasterLimiter {
    current_gain: f32,
    target_gain: f32,
    gain_delta: f32,
    maximum_observed: f32,
    frozen: bool,
}

impl MasterLimiter {
    fn with_settings(settings: DspSettings) -> Self {
        Self {
            current_gain: settings.max_limiter_gain,
            target_gain: settings.max_limiter_gain,
            gain_delta: settings.limiter_release_rate,
            maximum_observed: settings.limiter_threshold,
            frozen: false,
        }
    }

    /// Process a complete callback in place without allocating.  The callback
    /// size is deliberately part of the adjustment cadence, matching the C++
    /// `l + 1 == len || l % LIMITER_ADJUST_PERIOD == 0` condition.
    fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32], settings: DspSettings) {
        debug_assert_eq!(left.len(), right.len());
        let frames = left.len();
        let mut local_maximum = 0.0_f32;
        let mut clip_count = 0_u32;

        for (frame, (left, right)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let source_left = left.abs();
            let source_right = right.abs();
            *left *= self.current_gain;
            *right *= self.current_gain;
            let limited_left = left.abs();
            let limited_right = right.abs();

            if !self.frozen {
                local_maximum = local_maximum.max(source_left).max(source_right);
                clip_count += u32::from(limited_left > settings.limiter_threshold);
                clip_count += u32::from(limited_right > settings.limiter_threshold);
                self.current_gain += self.gain_delta;
            }

            // Preserve the original final format-safety ceiling.  This is
            // only a guard while the 1024-frame gain attack catches up.
            *left = left.clamp(-0.99, 0.99);
            *right = right.clamp(-0.99, 0.99);

            if frame + 1 == frames || frame % LIMITER_ADJUST_PERIOD == 0 {
                if (clip_count > 0 || local_maximum > self.maximum_observed) && local_maximum > 0.0
                {
                    clip_count = 0;
                    self.target_gain = settings.limiter_threshold / local_maximum;
                    self.gain_delta =
                        (self.target_gain - self.current_gain) / LIMITER_ATTACK_LENGTH;
                    self.maximum_observed = local_maximum;
                }

                if self.gain_delta < 0.0 && self.current_gain <= self.target_gain {
                    self.gain_delta = settings.limiter_release_rate;
                }
                if self.gain_delta > 0.0 && self.current_gain > settings.max_limiter_gain {
                    self.gain_delta = 0.0;
                }
            }
        }
    }
}

pub struct RuntimeAudioProcessor<B: FluidSynthBackend = FluidLiteBackend> {
    loops: [LoopSlot; MAX_RUNTIME_LOOPS],
    loop_storage: LoopStoragePool,
    commands: RealtimeReceiver<RuntimeCommand>,
    statuses: RealtimeSender<RuntimeStatus>,
    sample_clock: u64,
    pulse_frames: u32,
    pulse_position: u32,
    pulse_long_count: u32,
    pulse_long_length: u32,
    pulse_subdivide: u32,
    pulse_sync_active: bool,
    metro_enabled: bool,
    metro_gain: f32,
    metro_noise: Vec<f32>,
    metro_hi: Vec<f32>,
    metro_lo: Vec<f32>,
    metro_noise_offset: usize,
    metro_hi_offset: usize,
    metro_lo_offset: usize,
    monitor_gain: f32,
    master_gain: f32,
    master_limiter: MasterLimiter,
    dsp_settings: DspSettings,
    input_peak: [f32; 2],
    output_peak: [f32; 2],
    recording: Option<usize>,
    recording_waiting_start: bool,
    recording_waiting_stop: bool,
    recording_tail_remaining: Option<usize>,
    recording_end_justify: bool,
    /// C++ `RecordProcessor::nbeats`. This deliberately does not derive from
    /// PCM length: a sync recording retains `REC_TAIL_LEN` samples after its
    /// final downbeat, while its musical period ends at that downbeat.
    recording_pulse_beats: u32,
    /// C++ extends the selected pulse at `LoopManager::Deactivate`, before a
    /// second-half recording writes its delayed crossfade tail.
    recording_pulse_extension_applied: bool,
    input_history_left: Vec<f32>,
    input_history_right: Vec<f32>,
    input_history_position: usize,
    input_history_len: usize,
    synth: B,
    synth_enabled: bool,
    synth_channel: u8,
    synth_stereo: bool,
    synth_left: Vec<f32>,
    synth_right: Vec<f32>,
    sequence: u64,
    running: bool,
    transfers: Arc<TransferPool>,
    export_job: Option<ExportJob>,
    scope_refresh_slot: usize,
}

/// Constructs the concrete processor and its non-realtime control endpoint.
/// All loop memory and queue storage is allocated here, before activation.
pub fn production_audio_processor(
    synth: FluidLiteBackend,
    sample_rate: u32,
    max_loop_frames: usize,
    max_callback_frames: usize,
) -> (RuntimeAudioProcessor, RuntimeControls) {
    production_audio_processor_with_settings(
        synth,
        sample_rate,
        max_loop_frames,
        max_callback_frames,
        DspSettings::default(),
    )
}

pub fn production_audio_processor_with_settings(
    synth: FluidLiteBackend,
    sample_rate: u32,
    max_loop_frames: usize,
    max_callback_frames: usize,
    settings: DspSettings,
) -> (RuntimeAudioProcessor, RuntimeControls) {
    runtime_audio_processor_with_backend_settings(
        synth,
        sample_rate,
        max_loop_frames,
        max_callback_frames,
        settings,
    )
}

/// Test and adapter constructor. The backend and all render storage are owned
/// by the processor before it can be transferred to the audio callback.
pub fn runtime_audio_processor_with_backend<B: FluidSynthBackend>(
    synth: B,
    sample_rate: u32,
    max_loop_frames: usize,
    max_callback_frames: usize,
) -> (RuntimeAudioProcessor<B>, RuntimeControls) {
    runtime_audio_processor_with_backend_settings(
        synth,
        sample_rate,
        max_loop_frames,
        max_callback_frames,
        DspSettings::default(),
    )
}

pub fn runtime_audio_processor_with_backend_settings<B: FluidSynthBackend>(
    synth: B,
    sample_rate: u32,
    max_loop_frames: usize,
    max_callback_frames: usize,
    dsp_settings: DspSettings,
) -> (RuntimeAudioProcessor<B>, RuntimeControls) {
    assert!(sample_rate > 0, "sample rate must be non-zero");
    assert!(max_loop_frames > 0, "loop capacity must be non-zero");
    assert!(
        max_callback_frames > 0,
        "callback capacity must be non-zero"
    );
    let (command_tx, command_rx) = bounded(DEFAULT_COMMAND_CAPACITY);
    let (status_tx, status_rx) = bounded(DEFAULT_STATUS_CAPACITY);
    let transfers = Arc::new(TransferPool::new(DEFAULT_TRANSFER_SLOTS, max_loop_frames));
    let (loop_storage, loop_storage_refiller) = LoopStoragePool::new();
    // Port `Pulse`'s precomputed metronome material. The original uses the C
    // process-wide PRNG; keep Rust deterministic while preserving its shape,
    // frequencies, amplitudes, and decay exactly.
    let mut seed = 0x9e37_79b9_u32;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32 - 0.5
    };
    let metro_noise = (0..METRONOME_HIT_LEN)
        .map(|index| noise() * (1.0 - index as f32 / METRONOME_HIT_LEN as f32))
        .collect();
    let metro_hi = (0..METRONOME_TONE_LEN)
        .map(|index| {
            1.5 * (880.0 * index as f64 * 2.0 * std::f64::consts::PI / sample_rate as f64).sin()
                as f32
                * (1.0 - index as f32 / METRONOME_TONE_LEN as f32)
        })
        .collect();
    let metro_lo = (0..METRONOME_TONE_LEN)
        .map(|index| {
            (440.0 * index as f64 * 2.0 * std::f64::consts::PI / sample_rate as f64).sin() as f32
                * (1.0 - index as f32 / METRONOME_TONE_LEN as f32)
        })
        .collect();
    (
        RuntimeAudioProcessor {
            loops: std::array::from_fn(|_| LoopSlot::new()),
            loop_storage,
            commands: command_rx,
            statuses: status_tx,
            sample_clock: 0,
            pulse_frames: sample_rate / 2,
            pulse_position: 0,
            pulse_long_count: 0,
            pulse_long_length: 1,
            pulse_subdivide: 1,
            pulse_sync_active: false,
            metro_enabled: false,
            metro_gain: 0.1,
            metro_noise,
            metro_hi,
            metro_lo,
            // Match Pulse's constructor: no metronome hit plays merely
            // because the DSP graph begins at pulse position zero.
            metro_noise_offset: METRONOME_HIT_LEN,
            metro_hi_offset: METRONOME_TONE_LEN,
            metro_lo_offset: METRONOME_TONE_LEN,
            monitor_gain: 0.0,
            master_gain: 1.0,
            master_limiter: MasterLimiter::with_settings(dsp_settings),
            dsp_settings,
            input_peak: [0.0; 2],
            output_peak: [0.0; 2],
            recording: None,
            recording_waiting_start: false,
            recording_waiting_stop: false,
            recording_tail_remaining: None,
            recording_end_justify: false,
            recording_pulse_beats: 0,
            recording_pulse_extension_applied: false,
            // `audiomem` is the C++ engine's 10-second rolling input history;
            // it is independent of the recording block-chain capacity.
            input_history_left: vec![0.0; (sample_rate as usize * 10).min(max_loop_frames)],
            input_history_right: vec![0.0; (sample_rate as usize * 10).min(max_loop_frames)],
            input_history_position: 0,
            input_history_len: 0,
            synth,
            synth_enabled: true,
            synth_channel: 0,
            synth_stereo: true,
            synth_left: vec![0.0; max_callback_frames],
            synth_right: vec![0.0; max_callback_frames],
            sequence: 0,
            running: true,
            transfers: Arc::clone(&transfers),
            export_job: None,
            scope_refresh_slot: 0,
        },
        RuntimeControls {
            commands: command_tx,
            statuses: status_rx,
            transfers,
            loop_storage_refiller,
        },
    )
}

impl<B: FluidSynthBackend> RuntimeAudioProcessor<B> {
    fn send_status(&mut self, status: RuntimeStatus) {
        let _ = self.statuses.try_send(status);
    }

    fn snapshot(&mut self) {
        self.sequence = self.sequence.wrapping_add(1);
        let mut snapshot = RuntimeSnapshot {
            sequence: self.sequence,
            sample_clock: self.sample_clock,
            pulse_position: self.pulse_position,
            pulse_frames: self.pulse_frames,
            pulse_long_count: self.pulse_long_count,
            pulse_long_length: self.pulse_long_length,
            recording_slot: self.recording.map_or(-1, |slot| slot as i16),
            input_peak: self.input_peak,
            output_peak: self.output_peak,
            monitor_gain: self.monitor_gain,
            master_gain: self.master_gain,
            limiter_gain: self.master_limiter.current_gain,
            ..RuntimeSnapshot::default()
        };
        for (out, slot) in snapshot.loops.iter_mut().zip(&self.loops) {
            *out = RuntimeLoopSnapshot {
                mode: slot.mode,
                frames: slot.len as u32,
                position: slot.position as u32,
                gain: slot.gain,
                trigger_gain: slot.trigger_gain,
                gain_delta: slot.gain_delta,
                waveform: std::array::from_fn(|point| {
                    if slot.len == 0 {
                        return 0.0;
                    }
                    let index = point.saturating_mul(slot.len.saturating_sub(1))
                        / WAVEFORM_SAMPLES.saturating_sub(1);
                    let (left, right) = slot.sample_at(index);
                    (left + right) * 0.5
                }),
            };
        }
        for (loop_id, slot) in self.loops.iter_mut().enumerate() {
            if slot.len == 0 || snapshot.scope_count as usize >= DEFAULT_RECORDING_BUFFERS {
                continue;
            }
            let mut preview = RuntimeScopePreview {
                loop_id: loop_id as u8,
                position_column: (slot.position / PEAK_AVG_CHUNK_FRAMES) as u16,
                chunk_count: (slot.len / PEAK_AVG_CHUNK_FRAMES).min(MAX_LOOP_SCOPE_CHUNKS) as u16,
                current_peak: slot.recent_peak,
                ..RuntimeScopePreview::default()
            };
            preview.peaks.copy_from_slice(&slot.scope.peaks);
            preview.averages.copy_from_slice(&slot.scope.averages);
            snapshot.scopes[snapshot.scope_count as usize] = preview;
            snapshot.scope_count += 1;
            slot.recent_peak = 0.0;
        }
        self.send_status(RuntimeStatus::Snapshot(snapshot));
    }

    /// Exact stateful counterpart to `Pulse::ExtendLongCount`.  C++ grows
    /// this cycle when a loop is activated; deleting or muting a loop does
    /// not shorten it again.
    fn extend_pulse_long_count(&mut self, beats: u32, end_justify: bool) {
        if beats == 0 || !self.pulse_sync_active {
            return;
        }
        let old_length = self.pulse_long_length.max(1);
        let divisor = gcd_u32(old_length, beats);
        let new_length = old_length
            .saturating_div(divisor)
            .saturating_mul(beats)
            .max(1);
        if end_justify && new_length > old_length {
            let end_delta = old_length.saturating_sub(self.pulse_long_count % old_length);
            self.pulse_long_count = new_length.saturating_sub(end_delta);
        }
        self.pulse_long_length = new_length;
    }

    fn stop_recording(&mut self, notify: bool) {
        self.recording_waiting_start = false;
        self.recording_waiting_stop = false;
        self.recording_tail_remaining = None;
        if let Some(index) = self.recording.take() {
            let extension = {
                let slot = &mut self.loops[index];
                let is_new_recording = matches!(slot.mode, LoopMode::Recording);
                let is_overdub = matches!(slot.mode, LoopMode::Overdubbing);
                let completed = slot.len != 0
                    && matches!(slot.mode, LoopMode::Recording | LoopMode::Overdubbing);
                if is_new_recording && !slot.pulse_synced {
                    smooth_unsynchronised_loop_endpoints(slot);
                }
                // C++ `LoopManager::Deactivate` copies
                // `RecordProcessor::nbeats` into the Loop before
                // `RecordProcessor::End` records its post-downbeat tail.
                // Never infer this from PCM length: that would treat the
                // tail as hundreds of extra musical beats at small pulses.
                let extension =
                    (is_new_recording && slot.pulse_synced && slot.len != 0).then(|| {
                        slot.pulse_beats = slot.pulse_beats.max(1);
                        slot.pulse_beats
                    });
                slot.boundary_fade_position = None;
                slot.overdub_fade_out = None;
                // C++ PlayProcessor aligns a pulse-synchronised loop to the
                // current pulse position (including its long beat count) when
                // recording finishes. Starting at zero here was only correct at
                // a downbeat; stopping in the first half of a beat restarted the
                // new loop early by `pulse_position` frames.
                slot.position = if is_overdub {
                    slot.position
                } else if self.pulse_sync_active && slot.len != 0 {
                    pulse_synced_loop_position(
                        self.pulse_frames,
                        self.pulse_position,
                        self.pulse_long_count,
                        slot.pulse_beats,
                        slot.len,
                    )
                } else {
                    0
                };
                slot.mode = if slot.len == 0 {
                    LoopMode::Empty
                } else {
                    LoopMode::Playing
                };
                (extension, completed)
            };
            if let Some(beats) = extension
                .0
                .filter(|_| !self.recording_pulse_extension_applied)
            {
                self.extend_pulse_long_count(beats, self.recording_end_justify);
            }
            self.recording_pulse_extension_applied = false;
            if notify && extension.1 {
                self.send_status(RuntimeStatus::LoopCompleted { slot: index as u8 });
            }
        }
    }

    fn prefill_recording_from_history(&mut self, index: usize, requested: usize) {
        let capacity = self.input_history_left.len();
        let total = requested.min(self.loops[index].capacity());
        if total == 0 || capacity == 0 {
            return;
        }
        let available = total.min(self.input_history_len);
        let missing = total - available;
        let start = (self.input_history_position + capacity - available) % capacity;
        let target = &mut self.loops[index];
        for offset in 0..missing {
            target.set_sample(offset, 0.0, 0.0);
            target.record_scope_sample(0.0, 0.0);
        }
        for offset in 0..available {
            let source = (start + offset) % capacity;
            let left = self.input_history_left[source];
            let right = self.input_history_right[source];
            target.set_sample(missing + offset, left, right);
            target.record_scope_sample(left, right);
        }
        target.len = total;
    }

    fn request_stop_recording(&mut self, notify: bool) {
        if let Some(index) = self.recording
            && matches!(self.loops[index].mode, LoopMode::Overdubbing)
        {
            // C++ `RecordProcessor::End()` does not stop an overdub
            // immediately: its next process call fades input down over the
            // processor's prebuffer, then calls `EndNow()`.
            self.loops[index].overdub_fade_out = Some((0, 0));
            return;
        }
        if !self.pulse_sync_active || self.recording.is_none() {
            self.stop_recording(notify);
            return;
        }
        let index = self.recording.expect("recording checked above");
        let pulse = self.pulse_frames.max(1) as usize;
        let position = self.pulse_position as usize % pulse;
        // Match RecordProcessor::End exactly: `GetPct() >= 0.5 ||
        // GetPos() < REC_TAIL_LEN` schedules the delayed end-sync. The
        // second term matters just after a downbeat, even though that phase
        // is in the first half.
        if position * 2 < pulse && position >= REC_TAIL_FRAMES {
            self.recording_end_justify = false;
            // `LoopManager::Deactivate` preserves the current completed
            // callback count, but never creates a zero-beat loop.
            self.loops[index].pulse_beats = self.recording_pulse_beats.max(1);
            // `RecordProcessor::EndNow` ends immediately here.  The source
            // contains a proposed `HackTotalLengthBy` crop, but it is
            // commented out; preserve the recorded partial beat exactly.
            self.stop_recording(notify);
        } else {
            // In the second half, continue through the upcoming downbeat.
            // C++ commits the extra upcoming beat to the new Loop before
            // requesting the delayed `RecordProcessor::End` callback.
            self.loops[index].pulse_beats = self.recording_pulse_beats.saturating_add(1).max(1);
            self.extend_pulse_long_count(self.loops[index].pulse_beats, true);
            self.recording_pulse_extension_applied = true;
            self.recording_end_justify = true;
            self.recording_waiting_stop = true;
        }
    }

    fn apply_command(&mut self, command: RuntimeCommand) {
        if let Some(job) = self.export_job
            && command.mutates_loop(job.slot)
        {
            self.send_status(RuntimeStatus::CommandRejected(command));
            return;
        }
        match command {
            RuntimeCommand::Record { slot } => {
                self.stop_recording(true);
                let index = slot as usize;
                if index < self.loops.len() {
                    // Re-recording a C++ loop returns its whole AudioBlock
                    // chain to the shared pool before taking one fresh block.
                    if self.loops[index].uses_blocks() {
                        self.loop_storage.release_blocks(&mut self.loops[index]);
                    }
                    if self.loops[index].left.is_empty()
                        && !self.loop_storage.add_block(&mut self.loops[index])
                    {
                        self.send_status(RuntimeStatus::TransferError {
                            slot,
                            handle: PcmTransferHandle {
                                index: u16::MAX,
                                generation: 0,
                            },
                            error: PcmTransferError::RecordingStorageExhausted,
                        });
                        return;
                    }
                    let target = &mut self.loops[index];
                    target.data_offset = 0;
                    target.len = 0;
                    target.position = 0;
                    target.gain = 1.0;
                    target.trigger_gain = 1.0;
                    target.gain_delta = 1.0;
                    target.pulse_synced = self.pulse_sync_active;
                    target.pulse_beats = 0;
                    target.boundary_fade_position = None;
                    target.recent_peak = 0.0;
                    target.overdub_jump.reset();
                    // The scope cache is deliberately boxed at graph setup;
                    // reset it in place so a queued record command cannot
                    // allocate or free on the audio callback.
                    target.scope.reset();
                    target.mode = LoopMode::Recording;
                    self.recording = Some(index);
                    self.recording_end_justify = false;
                    self.recording_pulse_beats = 0;
                    self.recording_pulse_extension_applied = false;
                    if self.pulse_sync_active {
                        // C++ compares `GetPct() >= 0.5`; use a widened
                        // integer comparison so odd-length pulses make the
                        // same boundary decision without floating-point
                        // rounding.
                        if u64::from(self.pulse_position) * 2 >= u64::from(self.pulse_frames) {
                            self.recording_waiting_start = true;
                        } else {
                            let requested = self.pulse_position as usize;
                            while self.loops[index].uses_blocks()
                                && self.loops[index].capacity() < requested
                                && self.loop_storage.add_block(&mut self.loops[index])
                            {
                            }
                            self.prefill_recording_from_history(index, requested);
                        }
                    }
                } else {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                }
            }
            RuntimeCommand::Overdub {
                slot,
                feedback,
                gain,
            } => {
                self.stop_recording(true);
                let beats = self
                    .loops
                    .get(slot as usize)
                    .filter(|s| s.pulse_synced && s.len > 0)
                    .map(|s| s.pulse_beats.max(1));
                if let Some(target) = self.loops.get_mut(slot as usize).filter(|s| s.len > 0) {
                    target.position = if target.pulse_synced && self.pulse_sync_active {
                        pulse_synced_loop_position(
                            self.pulse_frames,
                            self.pulse_position,
                            self.pulse_long_count,
                            target.pulse_beats,
                            target.len,
                        )
                    } else {
                        0
                    };
                    target.feedback = feedback.clamp(0.0, 1.0);
                    target.trigger_gain = gain.max(0.0);
                    target.overdub_fade_out = None;
                    target.overdub_jump.reset();
                    target.mode = LoopMode::Overdubbing;
                    self.recording = Some(slot as usize);
                    if let Some(beats) = beats {
                        self.extend_pulse_long_count(beats, true);
                    }
                } else {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                }
            }
            RuntimeCommand::StopRecord => self.request_stop_recording(true),
            RuntimeCommand::Trigger { slot, gain } => {
                let beats = self
                    .loops
                    .get(slot as usize)
                    .filter(|s| s.pulse_synced && s.len > 0)
                    .map(|s| s.pulse_beats.max(1));
                if let Some(target) = self.loops.get_mut(slot as usize).filter(|s| s.len > 0) {
                    target.position = if target.pulse_synced && self.pulse_sync_active {
                        pulse_synced_loop_position(
                            self.pulse_frames,
                            self.pulse_position,
                            self.pulse_long_count,
                            target.pulse_beats,
                            target.len,
                        )
                    } else {
                        0
                    };
                    target.trigger_gain = gain.max(0.0);
                    target.mode = LoopMode::Playing;
                    if let Some(beats) = beats {
                        self.extend_pulse_long_count(beats, true);
                    }
                } else {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                }
            }
            RuntimeCommand::SetTriggerGain { slot, gain } => {
                if let Some(target) = self.loops.get_mut(slot as usize).filter(|s| s.len > 0) {
                    target.trigger_gain = gain.max(0.0);
                } else {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                }
            }
            RuntimeCommand::SetLoopGain { slot, gain } => {
                if let Some(target) = self.loops.get_mut(slot as usize).filter(|s| s.len > 0) {
                    target.gain = gain.max(0.0);
                } else {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                }
            }
            RuntimeCommand::AdjustLoopGain { slot, factor } => {
                if let Some(target) = self.loops.get_mut(slot as usize).filter(|s| s.len > 0) {
                    target.gain = (target.gain * factor).max(0.0);
                } else {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                }
            }
            RuntimeCommand::AdjustLoopGainDelta { slot, amount } => {
                if let Some(target) = self.loops.get_mut(slot as usize).filter(|s| s.len > 0) {
                    target.gain_delta = (target.gain_delta + amount).max(0.0);
                } else {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                }
            }
            RuntimeCommand::ResetLoopGainDeltas => {
                for target in &mut self.loops {
                    target.gain_delta = 1.0;
                }
            }
            RuntimeCommand::MoveLoop { from, to } => {
                let from = from as usize;
                let to = to as usize;
                if from >= self.loops.len()
                    || to >= self.loops.len()
                    || from == to
                    || self.loops[from].len == 0
                    || self.loops[to].len != 0
                {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                } else {
                    self.loops.swap(from, to);
                    if self.recording == Some(from) {
                        self.recording = Some(to);
                    } else if self.recording == Some(to) {
                        self.recording = Some(from);
                    }
                }
            }
            RuntimeCommand::Mute { slot, muted } => {
                if let Some(target) = self.loops.get_mut(slot as usize).filter(|s| s.len > 0) {
                    if muted {
                        target.position = 0;
                    }
                    target.mode = if muted {
                        LoopMode::Muted
                    } else {
                        LoopMode::Playing
                    };
                } else {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                }
            }
            RuntimeCommand::Erase { slot } => {
                if self.recording == Some(slot as usize) {
                    self.recording = None;
                }
                if let Some(target) = self.loops.get_mut(slot as usize) {
                    self.loop_storage.release_blocks(target);
                    target.data_offset = 0;
                    target.len = 0;
                    target.position = 0;
                    target.mode = LoopMode::Empty;
                    target.gain = 1.0;
                    target.trigger_gain = 1.0;
                    target.gain_delta = 1.0;
                    target.pulse_synced = false;
                    target.pulse_beats = 0;
                    target.boundary_fade_position = None;
                    target.recent_peak = 0.0;
                    target.overdub_jump.reset();
                    target.scope.reset();
                } else {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                }
            }
            RuntimeCommand::SetInputMonitor(gain) => self.monitor_gain = gain.max(0.0),
            RuntimeCommand::AdjustInputMonitor(amount) => {
                self.monitor_gain = (self.monitor_gain + amount).max(0.0)
            }
            RuntimeCommand::SetMasterGain(gain) => self.master_gain = gain.max(0.0),
            RuntimeCommand::AdjustMasterGain(amount) => {
                self.master_gain = (self.master_gain + amount).max(0.0)
            }
            RuntimeCommand::SetPulse { frames } => {
                self.pulse_frames = frames.max(1);
                self.pulse_position %= self.pulse_frames;
                self.pulse_long_count = 0;
                self.pulse_long_length = 1;
                self.pulse_sync_active = true;
            }
            RuntimeCommand::SetPulseSubdivide { beats } => {
                self.pulse_subdivide = beats.max(1);
            }
            RuntimeCommand::SetPulseFromLoop { slot } => {
                // C++ `LoopManager::SelectPulse` reselects an existing pulse
                // when F1 is pressed again; it does not recreate the pulse
                // from the latest loop or reset its phase.  Rebuilding it
                // here could move every already-synced loop and make later
                // recordings appear to have lost synchronization.
                if self.pulse_sync_active {
                    return;
                }
                let Some(loop_state) = self.loops.get(slot as usize) else {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                    return;
                };
                if loop_state.len == 0 {
                    self.send_status(RuntimeStatus::CommandRejected(command));
                    return;
                }
                // `LoopManager::CreatePulse(index, pulseindex, sub)` uses
                // integer division and stores `sub` as `Loop::nbeats`.  The
                // subdivision is deliberately persistent state set by
                // Shift+F1..F10, not a property inferred from old loops.
                let beats = self.pulse_subdivide.max(1);
                self.pulse_frames = (loop_state.len / beats as usize)
                    .max(1)
                    .min(u32::MAX as usize) as u32;
                self.pulse_position = (loop_state.position % self.pulse_frames as usize) as u32;
                self.pulse_long_count = 0;
                self.pulse_long_length = 1;
                self.pulse_sync_active = true;
                self.loops[slot as usize].pulse_synced = true;
                self.loops[slot as usize].pulse_beats = beats;
                self.loops[slot as usize].boundary_fade_position = None;
            }
            RuntimeCommand::ClearPulse => {
                self.pulse_sync_active = false;
                self.pulse_long_count = 0;
                self.pulse_long_length = 1;
            }
            RuntimeCommand::SetMetronome { enabled, gain } => {
                self.metro_enabled = enabled;
                self.metro_gain = gain.max(0.0);
            }
            RuntimeCommand::SynthNote { note, velocity } => {
                self.synth
                    .note_on(self.synth_channel, note.into(), velocity);
            }
            RuntimeCommand::SynthOff => self.synth.controller(0, 123, 0),
            RuntimeCommand::SetSynthEnabled(enabled) => {
                self.synth_enabled = enabled;
                if !enabled {
                    self.synth.controller(0, 123, 0);
                }
            }
            RuntimeCommand::SetSynthChannel(channel) => self.synth_channel = channel.min(15),
            RuntimeCommand::SetSynthStereo(stereo) => self.synth_stereo = stereo,
            RuntimeCommand::SynthController {
                channel: _,
                control,
                value,
            } => self.synth.controller(self.synth_channel, control, value),
            // C++ `FluidSynthProcessor` adds the FluidSynth pitch-bend
            // centre to the legacy event value at its synth boundary.
            RuntimeCommand::SynthPitchBend { channel: _, value } => self.synth.pitch_bend(
                self.synth_channel,
                i32::from(value).saturating_add(PITCH_BEND_CENTER),
            ),
            RuntimeCommand::SynthPatch {
                channel,
                soundfont_id,
                bank,
                program,
            } => self
                .synth
                .program_select(channel.min(15), soundfont_id, bank, program),
            RuntimeCommand::SynthTuning { cents } => self.synth.set_tuning(cents.into()),
            RuntimeCommand::ImportLoop {
                slot,
                handle,
                position,
                mode,
                gain,
            } => {
                let Some(target) = self.loops.get_mut(slot as usize) else {
                    if let Some(transfer) = self.transfers.slot(handle) {
                        transfer.state.store(TRANSFER_EXPORTED, Ordering::Release);
                    }
                    self.send_status(RuntimeStatus::TransferError {
                        slot,
                        handle,
                        error: PcmTransferError::InvalidHandle,
                    });
                    return;
                };
                let Some(transfer) = self
                    .transfers
                    .slot(handle)
                    .filter(|item| item.state.load(Ordering::Acquire) == TRANSFER_QUEUED)
                else {
                    self.send_status(RuntimeStatus::TransferError {
                        slot,
                        handle,
                        error: PcmTransferError::InvalidHandle,
                    });
                    return;
                };
                transfer.state.store(TRANSFER_CALLBACK, Ordering::Release);
                // SAFETY: CALLBACK grants this audio thread exclusive access.
                let pcm = unsafe { &mut *transfer.pcm.get() };
                // SAFETY: CALLBACK grants the same exclusive transfer owner
                // for its control-thread-precomputed scope cache.
                let imported_scope = unsafe { &*transfer.scope.get() };
                // Imported PCM is already prepared by the control thread.
                // Return any C++-style recording blocks before adopting it;
                // neither operation allocates or frees on this callback.
                self.loop_storage.release_blocks(target);
                std::mem::swap(&mut target.left, &mut pcm.left);
                std::mem::swap(&mut target.right, &mut pcm.right);
                std::mem::swap(&mut target.len, &mut pcm.len);
                target.data_offset = 0;
                target.position = if target.len == 0 {
                    0
                } else {
                    position as usize % target.len
                };
                target.mode = if target.len == 0 {
                    LoopMode::Empty
                } else {
                    match mode {
                        LoopMode::Empty | LoopMode::Recording => LoopMode::Playing,
                        other => other,
                    }
                };
                target.gain = gain.max(0.0);
                target.trigger_gain = 1.0;
                target.gain_delta = 1.0;
                target.pulse_synced = false;
                target.boundary_fade_position = None;
                target.recent_peak = 0.0;
                target.scope.reset();
                target.scope.peaks.copy_from_slice(&imported_scope.peaks);
                target
                    .scope
                    .averages
                    .copy_from_slice(&imported_scope.averages);
                target.scope.column = imported_scope.columns;
                // C++ PeaksAvgsManager ends at import completion; partial
                // chunks intentionally remain absent rather than being
                // continued on the DSP callback.
                target.scope.complete = true;
                transfer.state.store(TRANSFER_EXPORTED, Ordering::Release);
                self.send_status(RuntimeStatus::LoopImported { slot, handle });
            }
            RuntimeCommand::RequestLoopExport { slot, replacement } => {
                if self.export_job.is_some() {
                    if let Some(transfer) = self.transfers.slot(replacement) {
                        transfer.state.store(TRANSFER_EXPORTED, Ordering::Release);
                    }
                    self.send_status(RuntimeStatus::TransferError {
                        slot,
                        handle: replacement,
                        error: PcmTransferError::TransferBusy,
                    });
                    return;
                }
                let Some(target) = self.loops.get(slot as usize).filter(|item| {
                    item.len > 0
                        && !matches!(item.mode, LoopMode::Recording | LoopMode::Overdubbing)
                }) else {
                    if let Some(transfer) = self.transfers.slot(replacement) {
                        transfer.state.store(TRANSFER_EXPORTED, Ordering::Release);
                    }
                    self.send_status(RuntimeStatus::TransferError {
                        slot,
                        handle: replacement,
                        error: PcmTransferError::InvalidHandle,
                    });
                    return;
                };
                let Some(transfer) = self
                    .transfers
                    .slot(replacement)
                    .filter(|item| item.state.load(Ordering::Acquire) == TRANSFER_QUEUED)
                else {
                    self.send_status(RuntimeStatus::TransferError {
                        slot,
                        handle: replacement,
                        error: PcmTransferError::InvalidHandle,
                    });
                    return;
                };
                transfer.state.store(TRANSFER_CALLBACK, Ordering::Release);
                // SAFETY: CALLBACK grants this audio thread exclusive access.
                let pcm = unsafe { &mut *transfer.pcm.get() };
                pcm.len = target.len;
                let metadata = LoopTransferMetadata {
                    frames: pcm.len as u32,
                    position: target.position as u32,
                    mode: target.mode,
                    gain: target.gain,
                    pulse_frames: self.pulse_frames,
                    beats: (pcm.len as u32)
                        .checked_div(self.pulse_frames)
                        .map_or(0, |beats| beats.max(1)) as i64,
                };
                self.export_job = Some(ExportJob {
                    slot,
                    handle: replacement,
                    cursor: 0,
                    metadata,
                });
            }
            RuntimeCommand::RequestSnapshot => self.snapshot(),
            RuntimeCommand::Shutdown => {
                self.running = false;
                self.stop_recording(false);
                if let Some(job) = self.export_job.take() {
                    if let Some(transfer) = self.transfers.slot(job.handle) {
                        transfer.state.store(TRANSFER_EXPORTED, Ordering::Release);
                    }
                    self.send_status(RuntimeStatus::TransferError {
                        slot: job.slot,
                        handle: job.handle,
                        error: PcmTransferError::TransferBusy,
                    });
                }
                self.synth.shutdown();
                self.send_status(RuntimeStatus::ShutdownComplete);
            }
        }
    }

    fn drain_commands(&mut self) {
        while let Some(command) = self.commands.try_recv() {
            self.apply_command(command);
        }
    }

    fn advance_export(&mut self) {
        let Some(mut job) = self.export_job.take() else {
            return;
        };
        let target = &self.loops[job.slot as usize];
        let end = (job.cursor + EXPORT_COPY_FRAMES_PER_CALLBACK).min(job.metadata.frames as usize);
        let transfer = self
            .transfers
            .slot(job.handle)
            .expect("active export transfer must remain valid");
        // SAFETY: an active export keeps the transfer in CALLBACK state, which
        // grants this audio thread exclusive access until publication.
        let pcm = unsafe { &mut *transfer.pcm.get() };
        debug_assert!(pcm.left.len() >= job.metadata.frames as usize);
        debug_assert!(pcm.right.len() >= job.metadata.frames as usize);
        target.copy_range_to(
            job.cursor,
            &mut pcm.left[job.cursor..end],
            &mut pcm.right[job.cursor..end],
        );
        job.cursor = end;
        if end == job.metadata.frames as usize {
            transfer.state.store(TRANSFER_EXPORTED, Ordering::Release);
            self.send_status(RuntimeStatus::LoopExported {
                slot: job.slot,
                handle: job.handle,
                metadata: job.metadata,
            });
        } else {
            self.export_job = Some(job);
        }
    }

    /// Imported PCM has no C++ `PeaksAvgsManager`, so scan it once in bounded
    /// pieces. Native block-chain recordings maintain the same values while
    /// samples are written and never enter this playback-time path.
    fn refresh_scopes(&mut self) {
        let mut budget = SCOPE_REFRESH_SAMPLES_PER_CALLBACK;
        let mut empty_slots_seen = 0;
        while budget != 0 {
            let index = self.scope_refresh_slot;
            let slot = &mut self.loops[index];
            if slot.len == 0 || slot.uses_blocks() || slot.scope.complete {
                self.scope_refresh_slot = (self.scope_refresh_slot + 1) % self.loops.len();
                empty_slots_seen += 1;
                if empty_slots_seen == self.loops.len() {
                    return;
                }
                continue;
            }
            let column = slot.scope.column;
            if column >= (slot.len / PEAK_AVG_CHUNK_FRAMES).min(MAX_LOOP_SCOPE_CHUNKS) {
                slot.scope.complete = true;
                self.scope_refresh_slot = (self.scope_refresh_slot + 1) % self.loops.len();
                empty_slots_seen += 1;
                if empty_slots_seen == self.loops.len() {
                    return;
                }
                continue;
            }
            empty_slots_seen = 0;
            let frame = column * PEAK_AVG_CHUNK_FRAMES + slot.scope.sample;
            let (left, right) = slot.sample_at(frame);
            // Exact per-sample values from `PeaksAvgsManager::Manage`: linked
            // stereo range plus the mean of the channel absolute values.
            slot.scope.maximum = slot.scope.maximum.max(left).max(right);
            slot.scope.minimum = slot.scope.minimum.min(left).min(right);
            slot.scope.absolute_tally += (left.abs() + right.abs()) * 0.5;
            slot.scope.sample += 1;
            budget -= 1;

            if slot.scope.sample == PEAK_AVG_CHUNK_FRAMES {
                slot.scope.peaks[column] = slot.scope.maximum - slot.scope.minimum;
                slot.scope.averages[column] =
                    slot.scope.absolute_tally / PEAK_AVG_CHUNK_FRAMES as f32;
                slot.scope.sample = 0;
                slot.scope.maximum = 0.0;
                slot.scope.minimum = 0.0;
                slot.scope.absolute_tally = 0.0;
                slot.scope.column += 1;
            }
        }
    }
}

impl RuntimeCommand {
    fn mutates_loop(self, slot: u8) -> bool {
        match self {
            Self::Record { slot: target }
            | Self::Overdub { slot: target, .. }
            | Self::Erase { slot: target }
            | Self::ImportLoop { slot: target, .. } => target == slot,
            Self::SetTriggerGain { slot: target, .. }
            | Self::SetLoopGain { slot: target, .. }
            | Self::AdjustLoopGain { slot: target, .. } => target == slot,
            Self::AdjustLoopGainDelta { slot: target, .. } => target == slot,
            Self::MoveLoop { from, to } => from == slot || to == slot,
            _ => false,
        }
    }
}

impl<B: FluidSynthBackend> AudioProcessor for RuntimeAudioProcessor<B> {
    fn process(&mut self, callback: &mut AudioCallback<'_>) {
        self.drain_commands();
        self.advance_export();
        let frames = callback.nframes as usize;
        let frames = frames
            .min(callback.inputs[0].len())
            .min(callback.inputs[1].len())
            .min(callback.outputs[0].len())
            .min(callback.outputs[1].len())
            .min(self.synth_left.len());
        callback.outputs[0][..frames].fill(0.0);
        callback.outputs[1][..frames].fill(0.0);
        if !self.running {
            return;
        }
        if self.synth_enabled {
            self.synth.render(
                &mut self.synth_left[..frames],
                &mut self.synth_right[..frames],
            );
        } else {
            self.synth_left[..frames].fill(0.0);
            self.synth_right[..frames].fill(0.0);
        }
        if !self.synth_stereo {
            for (left, right) in self.synth_left[..frames]
                .iter_mut()
                .zip(&mut self.synth_right[..frames])
            {
                *left = (*left + *right) * 0.5;
                *right = 0.0;
            }
        }

        let mut input_peak = [0.0_f32; 2];
        let mut output_peak = [0.0_f32; 2];

        for frame in 0..frames {
            if self.pulse_sync_active && self.pulse_position == 0 {
                if self.recording_waiting_stop {
                    // C++ schedules `EndNow` at REC_TAIL_LEN after this
                    // downbeat rather than ending at the downbeat itself.
                    self.recording_waiting_stop = false;
                    // A stop requested before the start downbeat still
                    // starts at that downbeat in the C++ implementation.
                    self.recording_waiting_start = false;
                    self.recording_tail_remaining = Some(REC_TAIL_FRAMES);
                } else if self.recording_waiting_start {
                    self.recording_waiting_start = false;
                }

                // Pulse callbacks run at every downbeat in C++. They count
                // full beats while a new recording is active, and make every
                // active loop's iterator agree with the pulse long-count.
                // A recording created exactly at offset zero has len == 0 on
                // this first pass, so it correctly does not count a beat
                // until the *next* downbeat.
                for slot in &mut self.loops {
                    match slot.mode {
                        LoopMode::Recording
                            if !self.recording_waiting_start
                                && !self.recording_waiting_stop
                                && slot.pulse_synced
                                && slot.len != 0 =>
                        {
                            self.recording_pulse_beats =
                                self.recording_pulse_beats.saturating_add(1);
                        }
                        LoopMode::Playing | LoopMode::Overdubbing
                            if slot.pulse_synced && slot.pulse_beats != 0 && slot.len != 0 =>
                        {
                            let expected = pulse_synced_loop_position(
                                self.pulse_frames,
                                self.pulse_position,
                                self.pulse_long_count,
                                slot.pulse_beats,
                                slot.len,
                            );
                            if slot.position != expected {
                                // `PlayProcessor::PulseSync` calls
                                // `dopreprocess()` before its iterator jump.
                                // This precomputed tail/head blend is the
                                // native equivalent only when `curbeat`
                                // wraps back to beat zero. Intermediate beat
                                // jumps merely skip the retained record tail.
                                if matches!(slot.mode, LoopMode::Playing)
                                    && self.pulse_long_count % slot.pulse_beats == 0
                                {
                                    slot.boundary_fade_position = Some(0);
                                }
                                if matches!(slot.mode, LoopMode::Overdubbing) {
                                    // `RecordProcessor::Jump` retains the
                                    // previous raw fragment in
                                    // `od_last_lpbuf` for the next actual
                                    // process pass.
                                    slot.overdub_jump.begin_fade();
                                }
                                slot.position = expected;
                            }
                        }
                        _ => {}
                    }
                }
            }
            let input_l = callback.inputs[0][frame];
            let input_r = callback.inputs[1][frame];
            input_peak[0] = input_peak[0].max(input_l.abs());
            input_peak[1] = input_peak[1].max(input_r.abs());
            let mut left = input_l * self.monitor_gain;
            let mut right = input_r * self.monitor_gain;

            let mut finished_overdub = None;
            let mut finished_recording_tail = false;
            for index in 0..self.loops.len() {
                match self.loops[index].mode {
                    LoopMode::Recording => {
                        if self.recording_waiting_start && self.recording == Some(index) {
                            continue;
                        }
                        if self.loops[index].len == self.loops[index].capacity()
                            && self.loops[index].uses_blocks()
                        {
                            let _ = self.loop_storage.add_block(&mut self.loops[index]);
                        }
                        let slot = &mut self.loops[index];
                        if slot.len == slot.capacity() {
                            if !slot.pulse_synced {
                                smooth_unsynchronised_loop_endpoints(slot);
                            }
                            slot.boundary_fade_position = None;
                            slot.mode = LoopMode::Playing;
                            slot.position = 0;
                            self.recording = None;
                            self.recording_tail_remaining = None;
                            let _ = self
                                .statuses
                                .try_send(RuntimeStatus::RecordingFull { slot: index as u8 });
                        } else {
                            slot.set_sample(slot.len, input_l, input_r);
                            slot.record_scope_sample(input_l, input_r);
                            slot.recent_peak =
                                slot.recent_peak.max(input_l.abs()).max(input_r.abs());
                            slot.len += 1;
                            // A new C++ `RecordProcessor` has no play loop:
                            // its play section explicitly clears its output
                            // while it writes the input fragment. Live input
                            // auditioning is owned by the separate monitor
                            // path above, never by the just-recorded loop.
                        }
                    }
                    LoopMode::Overdubbing => {
                        let slot = &mut self.loops[index];
                        update_loop_gain(slot);
                        let pos = slot.position;
                        let (old_left, old_right) = slot.sample_at(pos);
                        let mut output_left = old_left;
                        let mut output_right = old_right;
                        let jump_fade = slot.overdub_jump.fade_position.and_then(|progress| {
                            (progress < slot.overdub_jump.count).then_some(progress)
                        });
                        if let Some(progress) = jump_fade {
                            let ramp = progress as f32 / LOOP_SMOOTH_FRAMES as f32;
                            let previous_position = slot.overdub_jump.fade_positions[progress];
                            let previous_left = slot.overdub_jump.fade_left[progress];
                            let previous_right = slot.overdub_jump.fade_right[progress];
                            // `od_prefadeout`: revise the old fragment with
                            // the current input fading to the unmodified loop
                            // signal and unity feedback.
                            slot.set_sample(
                                previous_position,
                                input_l * (1.0 - ramp)
                                    + previous_left * (ramp + (1.0 - ramp) * slot.feedback),
                                input_r * (1.0 - ramp)
                                    + previous_right * (ramp + (1.0 - ramp) * slot.feedback),
                            );
                            // `dopreprocess` rendered this old raw fragment
                            // before `Jump`; mix it into the new position's
                            // output exactly as `fadepreandcurrent` does.
                            output_left = old_left * ramp + previous_left * (1.0 - ramp);
                            output_right = old_right * ramp + previous_right * (1.0 - ramp);
                            slot.overdub_jump.fade_position =
                                (progress + 1 < slot.overdub_jump.count).then_some(progress + 1);
                        }
                        // `RecordProcessor` first emits the existing loop
                        // fragment, then stores the overdubbed fragment.  The
                        // latter must never leak into the current output
                        // sample merely because recording is active.
                        let (new_left, new_right, ends_fade) = if let Some(progress) = jump_fade {
                            let ramp = progress as f32 / LOOP_SMOOTH_FRAMES as f32;
                            (
                                input_l * ramp + old_left * (1.0 - ramp + ramp * slot.feedback),
                                input_r * ramp + old_right * (1.0 - ramp + ramp * slot.feedback),
                                false,
                            )
                        } else if let Some((progress, total)) = slot.overdub_fade_out {
                            let total = total.max(frames);
                            let ramp = progress as f32 / total as f32;
                            let input_gain = 1.0 - ramp;
                            let loop_gain = ramp + input_gain * slot.feedback;
                            slot.overdub_fade_out =
                                (progress + 1 < total).then_some((progress + 1, total));
                            (
                                input_l * input_gain + old_left * loop_gain,
                                input_r * input_gain + old_right * loop_gain,
                                progress + 1 == total,
                            )
                        } else {
                            (
                                old_left * slot.feedback + input_l,
                                old_right * slot.feedback + input_r,
                                false,
                            )
                        };
                        slot.set_sample(pos, new_left, new_right);
                        slot.overdub_jump.push(pos, old_left, old_right);
                        slot.recent_peak =
                            slot.recent_peak.max(new_left.abs()).max(new_right.abs());
                        let gain = capped_loop_gain(slot, self.dsp_settings.max_play_volume);
                        left += output_left * gain;
                        right += output_right * gain;
                        slot.position = (pos + 1) % slot.len;
                        if ends_fade {
                            finished_overdub = Some(index);
                        }
                    }
                    LoopMode::Playing => {
                        let slot = &mut self.loops[index];
                        update_loop_gain(slot);
                        let position = slot.position;
                        let (mut loop_left, mut loop_right) = slot.sample_at(position);
                        if let Some(fade_position) = slot.boundary_fade_position {
                            let smooth_frames = LOOP_SMOOTH_FRAMES.min(slot.len);
                            if fade_position < smooth_frames {
                                let mix = fade_position as f32 / smooth_frames as f32;
                                let tail_position = slot.len - smooth_frames + fade_position;
                                let (tail_left, tail_right) = slot.sample_at(tail_position);
                                loop_left = mix * loop_left + (1.0 - mix) * tail_left;
                                loop_right = mix * loop_right + (1.0 - mix) * tail_right;
                                slot.boundary_fade_position = (fade_position + 1 < smooth_frames)
                                    .then_some(fade_position + 1);
                            } else {
                                slot.boundary_fade_position = None;
                            }
                        }
                        slot.recent_peak =
                            slot.recent_peak.max(loop_left.abs()).max(loop_right.abs());
                        let gain = capped_loop_gain(slot, self.dsp_settings.max_play_volume);
                        left += loop_left * gain;
                        right += loop_right * gain;
                        slot.position = (position + 1) % slot.len;
                        if slot.position == 0
                            && slot.pulse_synced
                            && (slot.pulse_beats == 0
                                || slot.len
                                    == slot.pulse_beats as usize
                                        * self.pulse_frames.max(1) as usize)
                        {
                            // Imported/exact-length sync loops have no
                            // retained recording tail. Their natural wrap is
                            // the C++ PulseSync restart and still needs the
                            // preprocessed boundary blend.
                            slot.boundary_fade_position = Some(0);
                        }
                    }
                    LoopMode::Empty | LoopMode::Muted => {}
                }
            }
            if finished_overdub.is_some() {
                self.stop_recording(true);
            } else if let Some(remaining) = self.recording_tail_remaining {
                if remaining <= 1 {
                    self.recording_tail_remaining = None;
                    finished_recording_tail = true;
                } else {
                    self.recording_tail_remaining = Some(remaining - 1);
                }
                if finished_recording_tail {
                    // `PulseSync` is delivered after the pulse has advanced
                    // to its requested offset.  This sample loop advances
                    // the pulse at the bottom of the iteration, so present
                    // EndNow with that next position before publishing the
                    // PlayProcessor start offset.
                    let current_pulse_position = self.pulse_position;
                    self.pulse_position = (self.pulse_position + 1) % self.pulse_frames.max(1);
                    self.stop_recording(true);
                    self.pulse_position = current_pulse_position;
                }
            }

            left += self.synth_left[frame];
            right += self.synth_right[frame];
            if self.metro_enabled && self.metro_gain > 0.0 {
                let mut metronome = 0.0;
                if self.metro_noise_offset < self.metro_noise.len() {
                    metronome += self.metro_noise[self.metro_noise_offset];
                }
                if self.metro_hi_offset < self.metro_hi.len() {
                    metronome += self.metro_hi[self.metro_hi_offset];
                }
                if self.metro_lo_offset < self.metro_lo.len() {
                    metronome += self.metro_lo[self.metro_lo_offset];
                }
                left += metronome * self.metro_gain;
                right += metronome * self.metro_gain;
            }
            self.metro_noise_offset = self.metro_noise_offset.saturating_add(1);
            self.metro_hi_offset = self.metro_hi_offset.saturating_add(1);
            self.metro_lo_offset = self.metro_lo_offset.saturating_add(1);
            // The C++ graph routes the complete master mix through one linked
            // AutoLimitProcessor.  Do not clamp here: that creates a new
            // discontinuity every callback once several loops are active.
            callback.outputs[0][frame] = left * self.master_gain;
            callback.outputs[1][frame] = right * self.master_gain;
            if !self.input_history_left.is_empty() {
                self.input_history_left[self.input_history_position] = input_l;
                self.input_history_right[self.input_history_position] = input_r;
                self.input_history_position =
                    (self.input_history_position + 1) % self.input_history_left.len();
                self.input_history_len =
                    (self.input_history_len + 1).min(self.input_history_left.len());
            }
            self.pulse_position += 1;
            if self.pulse_position >= self.pulse_frames {
                self.pulse_position = 0;
                let long_length = self.pulse_long_length.max(1);
                self.pulse_long_count = (self.pulse_long_count + 1) % long_length;
                self.metro_noise_offset = 0;
            }
        }
        let (left_output, right_output) = callback.outputs.split_at_mut(1);
        self.master_limiter.process_stereo(
            &mut left_output[0][..frames],
            &mut right_output[0][..frames],
            self.dsp_settings,
        );
        for frame in 0..frames {
            output_peak[0] = output_peak[0].max(callback.outputs[0][frame].abs());
            output_peak[1] = output_peak[1].max(callback.outputs[1][frame].abs());
        }
        self.sample_clock = self.sample_clock.wrapping_add(frames as u64);
        self.input_peak = input_peak;
        self.output_peak = output_peak;
        self.refresh_scopes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audioio::JackPosition;

    #[derive(Default)]
    struct FakeSynth {
        note: Option<(u8, i32, u8)>,
        controller: Option<(u8, u8, u8)>,
        bend: Option<(u8, i32)>,
        patch: Option<(u8, i32, i32, i32)>,
        tuning: f64,
        render_value: f32,
        shutdown: bool,
    }

    impl FluidSynthBackend for FakeSynth {
        fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
            left.fill(self.render_value);
            right.fill(self.render_value * 2.0);
        }
        fn controller(&mut self, channel: u8, control: u8, value: u8) {
            self.controller = Some((channel, control, value));
        }
        fn pitch_bend(&mut self, channel: u8, value: i32) {
            self.bend = Some((channel, value));
        }
        fn note_on(&mut self, channel: u8, note: i32, velocity: u8) {
            self.note = Some((channel, note, velocity));
        }
        fn note_off(&mut self, _: u8, _: i32) {}
        fn program_select(&mut self, channel: u8, sf: i32, bank: i32, program: i32) {
            self.patch = Some((channel, sf, bank, program));
        }
        fn set_tuning(&mut self, cents: f64) {
            self.tuning = cents;
        }
        fn shutdown(&mut self) {
            self.shutdown = true;
        }
    }

    fn processor(render_value: f32) -> (RuntimeAudioProcessor<FakeSynth>, RuntimeControls) {
        runtime_audio_processor_with_backend(
            FakeSynth {
                render_value,
                ..FakeSynth::default()
            },
            48_000,
            8,
            32,
        )
    }

    fn run<B: FluidSynthBackend>(
        processor: &mut RuntimeAudioProcessor<B>,
        left: &[f32],
        right: &[f32],
    ) -> [Vec<f32>; 2] {
        let mut out_l = vec![0.0; left.len()];
        let mut out_r = vec![0.0; left.len()];
        let mut callback = AudioCallback {
            inputs: [left, right],
            outputs: [&mut out_l, &mut out_r],
            nframes: left.len() as u32,
            position: JackPosition::default(),
        };
        processor.process(&mut callback);
        [out_l, out_r]
    }

    #[test]
    fn records_triggers_mutes_overdubs_and_erases() {
        let (mut processor, mut controls) = processor(0.0);
        controls
            .try_command(RuntimeCommand::Record { slot: 0 })
            .unwrap();
        run(&mut processor, &[1.0, 0.5], &[0.25, -0.25]);
        controls.try_command(RuntimeCommand::StopRecord).unwrap();
        let played = run(&mut processor, &[0.0; 2], &[0.0; 2]);
        // The production master stage follows the C++ limiter: its final
        // format guard caps a full-scale sample at 0.99, while the gain
        // envelope only changes by a fraction of a percent here.
        assert_eq!(played[0][0], 0.99);
        assert!((played[0][1] - 0.5).abs() < 0.001);
        controls
            .try_command(RuntimeCommand::Overdub {
                slot: 0,
                feedback: 0.5,
                gain: 1.0,
            })
            .unwrap();
        let overdubbed = run(&mut processor, &[0.5, 0.5], &[0.0, 0.0]);
        assert_eq!(overdubbed[0][0], 0.99);
        // C++ outputs the old loop fragment before recording the overdubbed
        // replacement, so its second sample is the original 0.5, not 0.75.
        assert!((overdubbed[0][1] - 0.5).abs() < 0.002);
        controls.try_command(RuntimeCommand::StopRecord).unwrap();
        controls
            .try_command(RuntimeCommand::Mute {
                slot: 0,
                muted: true,
            })
            .unwrap();
        assert_eq!(run(&mut processor, &[0.0; 2], &[0.0; 2])[0], [0.0; 2]);
        controls
            .try_command(RuntimeCommand::Erase { slot: 0 })
            .unwrap();
        controls
            .try_command(RuntimeCommand::RequestSnapshot)
            .unwrap();
        run(&mut processor, &[], &[]);
        let snapshot = loop {
            match controls.try_status().expect("expected status") {
                RuntimeStatus::Snapshot(snapshot) => break snapshot,
                RuntimeStatus::LoopCompleted { slot: 0 } => {}
                other => panic!("unexpected status: {other:?}"),
            }
        };
        assert_eq!(snapshot.loops[0].mode, LoopMode::Empty);
    }

    #[test]
    fn overdub_plays_old_audio_and_fades_the_final_recording_buffer() {
        let (mut processor, mut controls) = processor(0.0);
        controls
            .try_command(RuntimeCommand::Record { slot: 0 })
            .unwrap();
        run(&mut processor, &[1.0; 4], &[0.0; 4]);
        controls.try_command(RuntimeCommand::StopRecord).unwrap();
        run(&mut processor, &[], &[]);
        controls
            .try_command(RuntimeCommand::Overdub {
                slot: 0,
                feedback: 0.5,
                gain: 1.0,
            })
            .unwrap();
        let before_stop = run(&mut processor, &[1.0; 4], &[0.0; 4]);
        // Output remains the loop as it was before this overdub pass.
        assert_eq!(before_stop[0], [0.99; 4]);

        controls.try_command(RuntimeCommand::StopRecord).unwrap();
        let final_pass = run(&mut processor, &[1.0; 4], &[0.0; 4]);
        assert_eq!(final_pass[0], [0.99; 4]);
        assert_eq!(processor.recording, None);
        assert_eq!(processor.loops[0].mode, LoopMode::Playing);
        for (frame, expected) in [1.75, 1.687_5, 1.625, 1.562_5].into_iter().enumerate() {
            let (actual, _) = processor.loops[0].sample_at(frame);
            assert!(
                (actual - expected).abs() < 0.000_01,
                "frame {frame}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn selected_pulse_uses_live_completed_loop_length() {
        let (mut processor, mut controls) = processor(0.0);
        controls
            .try_command(RuntimeCommand::Record { slot: 0 })
            .unwrap();
        run(&mut processor, &[1.0, 0.5, 0.25], &[0.0; 3]);
        controls.try_command(RuntimeCommand::StopRecord).unwrap();
        controls
            .try_command(RuntimeCommand::SetPulseFromLoop { slot: 0 })
            .unwrap();
        controls
            .try_command(RuntimeCommand::RequestSnapshot)
            .unwrap();
        run(&mut processor, &[], &[]);

        let snapshot = loop {
            match controls.try_status().expect("expected status") {
                RuntimeStatus::Snapshot(snapshot) => break snapshot,
                RuntimeStatus::LoopCompleted { slot: 0 } => {}
                other => panic!("unexpected status: {other:?}"),
            }
        };
        assert_eq!(snapshot.pulse_frames, 3);
    }

    #[test]
    fn reselecting_f1_keeps_the_existing_pulse_phase_for_later_loops() {
        let (mut processor, mut controls) = processor(0.0);
        controls
            .try_command(RuntimeCommand::Record { slot: 0 })
            .unwrap();
        run(&mut processor, &[1.0; 4], &[0.0; 4]);
        controls.try_command(RuntimeCommand::StopRecord).unwrap();
        run(&mut processor, &[], &[]);

        controls
            .try_command(RuntimeCommand::SetPulseFromLoop { slot: 0 })
            .unwrap();
        run(&mut processor, &[], &[]);
        processor.pulse_position = 2;
        processor.pulse_long_count = 5;
        processor.pulse_long_length = 6;

        // F1 with an existing pulse is a reselect in C++, not a pulse
        // reconstruction from whichever loop was recorded most recently.
        controls
            .try_command(RuntimeCommand::SetPulseFromLoop { slot: 0 })
            .unwrap();
        run(&mut processor, &[], &[]);

        assert_eq!(processor.pulse_frames, 4);
        assert_eq!(processor.pulse_position, 2);
        assert_eq!(processor.pulse_long_count, 5);
        assert_eq!(processor.pulse_long_length, 6);

        // Every new recording observes the still-active pulse, including
        // recordings started after the reselect.
        for slot in [1_u8, 2_u8] {
            controls
                .try_command(RuntimeCommand::Record { slot })
                .unwrap();
            run(&mut processor, &[0.5], &[0.0]);
            assert!(processor.loops[slot as usize].pulse_synced);
            controls.try_command(RuntimeCommand::StopRecord).unwrap();
            run(&mut processor, &[], &[]);
        }
    }

    #[test]
    fn cpp_subdivide_persists_and_divides_the_next_loop_defined_pulse() {
        let (mut processor, mut controls) = processor(0.0);
        controls
            .try_command(RuntimeCommand::Record { slot: 0 })
            .unwrap();
        run(&mut processor, &[1.0; 6], &[0.0; 6]);
        controls.try_command(RuntimeCommand::StopRecord).unwrap();
        controls
            .try_command(RuntimeCommand::SetPulseSubdivide { beats: 3 })
            .unwrap();
        controls
            .try_command(RuntimeCommand::SetPulseFromLoop { slot: 0 })
            .unwrap();
        controls
            .try_command(RuntimeCommand::RequestSnapshot)
            .unwrap();
        run(&mut processor, &[], &[]);

        let snapshot = loop {
            match controls.try_status().expect("expected status") {
                RuntimeStatus::Snapshot(snapshot) => break snapshot,
                RuntimeStatus::LoopCompleted { slot: 0 } => {}
                other => panic!("unexpected status: {other:?}"),
            }
        };
        assert_eq!(snapshot.pulse_frames, 2);
        assert_eq!(processor.loops[0].pulse_beats, 3);
    }

    #[test]
    fn pulse_long_count_uses_the_cpp_lcm_of_synchronised_loop_beats() {
        let (mut processor, mut controls) = processor(0.0);
        processor.pulse_sync_active = true;
        processor.pulse_frames = 4;
        processor.pulse_position = 3;
        // C++ grows the long count when synchronised loops are activated,
        // rather than deriving it from every stored loop on each callback.
        processor.extend_pulse_long_count(2, false);
        processor.extend_pulse_long_count(3, false);
        // One frame crosses a pulse boundary: lcm(2, 3) == 6 and the current
        // long-count advances once.
        run(&mut processor, &[0.0], &[0.0]);
        controls
            .try_command(RuntimeCommand::RequestSnapshot)
            .unwrap();
        run(&mut processor, &[], &[]);
        let snapshot = loop {
            match controls.try_status().expect("expected status") {
                RuntimeStatus::Snapshot(snapshot) => break snapshot,
                other => panic!("unexpected status: {other:?}"),
            }
        };
        assert_eq!(snapshot.pulse_long_length, 6);
        assert_eq!(snapshot.pulse_long_count, 1);
    }

    #[test]
    fn cpp_long_count_is_grow_only_and_end_justifies_new_cycles() {
        let (mut processor, _controls) = processor(0.0);
        processor.pulse_sync_active = true;
        processor.pulse_long_count = 1;
        processor.pulse_long_length = 2;
        processor.extend_pulse_long_count(3, true);
        // C++: end_delta = 2 - 1; lc_cur = 6 - end_delta.
        assert_eq!(processor.pulse_long_length, 6);
        assert_eq!(processor.pulse_long_count, 5);

        // Removing/muting loops does not call ExtendLongCount and therefore
        // cannot shrink the cycle back to a divisor.
        processor.loops[0].pulse_beats = 1;
        processor.loops[0].mode = LoopMode::Empty;
        assert_eq!(processor.pulse_long_length, 6);
    }

    #[test]
    fn second_half_stop_extends_long_count_before_the_cpp_record_tail() {
        let (mut processor, _controls) = processor(0.0);
        processor.pulse_sync_active = true;
        processor.pulse_frames = 4;
        processor.pulse_position = 2;
        processor.pulse_long_length = 3;
        processor.pulse_long_count = 1;
        processor.recording = Some(0);
        processor.recording_pulse_beats = 1;
        let slot = &mut processor.loops[0];
        slot.mode = LoopMode::Recording;
        slot.pulse_synced = true;
        slot.len = 1;

        // C++ `LoopManager::Deactivate` does this before calling
        // `RecordProcessor::End`, which then waits through the downbeat tail.
        processor.request_stop_recording(false);
        assert!(processor.recording_waiting_stop);
        assert_eq!(processor.loops[0].pulse_beats, 2);
        assert_eq!(processor.pulse_long_length, 6);
        // old length 3/current 1: C++ end_delta = 2, new current = 6 - 2.
        assert_eq!(processor.pulse_long_count, 4);

        // Completing the delayed recorder must retain that already-applied
        // phase rather than extending the LCM a second time.
        processor.stop_recording(false);
        assert_eq!(processor.pulse_long_length, 6);
        assert_eq!(processor.pulse_long_count, 4);
    }

    #[test]
    fn selected_pulse_quantizes_new_recording_to_beat_boundaries() {
        let (mut processor, mut controls) = processor(0.0);
        controls
            .try_command(RuntimeCommand::Record { slot: 0 })
            .unwrap();
        run(&mut processor, &[1.0; 4], &[0.0; 4]);
        controls.try_command(RuntimeCommand::StopRecord).unwrap();
        run(&mut processor, &[], &[]);

        controls
            .try_command(RuntimeCommand::SetPulseFromLoop { slot: 0 })
            .unwrap();
        controls
            .try_command(RuntimeCommand::Record { slot: 1 })
            .unwrap();
        run(&mut processor, &[0.5; 6], &[0.0; 6]);
        controls.try_command(RuntimeCommand::StopRecord).unwrap();
        // Stop was requested in the second half, so C++ continues through
        // the upcoming downbeat and records its 1,024-frame crossfade tail.
        run(&mut processor, &[0.5; 2], &[0.0; 2]);
        for _ in 0..(REC_TAIL_FRAMES / 32) {
            run(&mut processor, &[0.0; 32], &[0.0; 32]);
        }
        controls
            .try_command(RuntimeCommand::RequestSnapshot)
            .unwrap();
        run(&mut processor, &[], &[]);

        let snapshot = loop {
            match controls.try_status().expect("expected status") {
                RuntimeStatus::Snapshot(snapshot) => break snapshot,
                RuntimeStatus::LoopCompleted { .. } => {}
                other => panic!("unexpected status: {other:?}"),
            }
        };
        assert_eq!(snapshot.pulse_frames, 4);
        assert_eq!(snapshot.loops[1].frames, 8 + REC_TAIL_FRAMES as u32);
        // The loop phase is captured when C++ deactivates the recorder.  The
        // delayed tail then advances the live pulse, so comparing the loop
        // position with the later snapshot pulse position is incorrect.
        assert_eq!(snapshot.loops[1].position, snapshot.pulse_frames);
        // The retained tail is crossfade material, not 256 extra beats.
        assert_eq!(processor.loops[1].pulse_beats, 2);
    }

    #[test]
    fn synced_recording_prepends_input_since_the_previous_downbeat() {
        let (mut processor, mut controls) = processor(0.0);
        // `RecordProcessor` asks `BED_MarkerPoints` for the marker immediately
        // before the current audio-memory iterator and prepends that subchain.
        // Feed one partial pulse first so its rolling input history represents
        // the source interval `[previous downbeat, command)`.
        processor.pulse_sync_active = true;
        processor.pulse_frames = 8;
        processor.pulse_position = 0;
        run(&mut processor, &[0.10, 0.20, 0.30], &[-0.10, -0.20, -0.30]);
        assert_eq!(processor.pulse_position, 3);

        controls
            .try_command(RuntimeCommand::Record { slot: 0 })
            .unwrap();
        // The command is consumed before the callback frame is written, so
        // the C++ subchain is followed by this current input sample.
        run(&mut processor, &[0.40], &[-0.40]);

        let slot = &processor.loops[0];
        assert_eq!(slot.len, 4);
        assert_eq!(
            (0..slot.len)
                .map(|frame| slot.sample_at(frame))
                .collect::<Vec<_>>(),
            vec![(0.10, -0.10), (0.20, -0.20), (0.30, -0.30), (0.40, -0.40)]
        );
    }

    #[test]
    fn synced_stop_just_after_downbeat_keeps_the_cpp_record_tail_pending() {
        let (mut processor, _controls) = processor(0.0);
        processor.pulse_sync_active = true;
        processor.pulse_frames = 4096;
        processor.pulse_position = 1;
        processor.recording = Some(0);
        let slot = &mut processor.loops[0];
        slot.left = vec![0.0; 32];
        slot.right = vec![0.0; 32];
        slot.mode = LoopMode::Recording;
        slot.len = 6;

        // C++ RecordProcessor::End uses `GetPos() < REC_TAIL_LEN` as an
        // additional delayed-end condition, even though this is the first
        // half of the pulse. The recording therefore remains live until the
        // next downbeat plus the crossfade tail.
        processor.request_stop_recording(false);

        assert_eq!(processor.loops[0].len, 6);
        assert!(processor.recording_waiting_stop);
        assert!(processor.recording_tail_remaining.is_none());
    }

    #[test]
    fn synced_stop_far_enough_into_first_half_keeps_the_current_pulse_phase() {
        let (mut processor, _controls) = processor(0.0);
        processor.pulse_sync_active = true;
        processor.pulse_frames = 4096;
        processor.pulse_position = REC_TAIL_FRAMES as u32;
        processor.recording = Some(0);
        let slot = &mut processor.loops[0];
        slot.mode = LoopMode::Recording;
        slot.len = 6;

        processor.request_stop_recording(false);

        assert_eq!(processor.loops[0].len, 6);
        assert_eq!(processor.loops[0].mode, LoopMode::Playing);
        assert_eq!(processor.loops[0].position, REC_TAIL_FRAMES % 6);
    }

    #[test]
    fn stopping_before_a_synced_start_downbeat_records_the_cpp_tail() {
        let (mut processor, _controls) = processor(0.0);
        processor.pulse_sync_active = true;
        processor.pulse_frames = 4096;
        processor.pulse_position = 3000;
        processor.recording = Some(0);
        processor.recording_waiting_start = true;
        let slot = &mut processor.loops[0];
        slot.left = vec![0.0; 32];
        slot.right = vec![0.0; 32];
        slot.mode = LoopMode::Recording;
        slot.pulse_synced = true;

        processor.request_stop_recording(false);

        assert!(processor.recording_waiting_start);
        assert!(processor.recording_waiting_stop);
        assert_eq!(processor.loops[0].pulse_beats, 1);

        // Enter the downbeat. The waiting recorder must become active and
        // capture the same post-downbeat tail as the C++ implementation.
        processor.pulse_position = 0;
        run(&mut processor, &[0.25], &[0.25]);
        assert!(!processor.recording_waiting_start);
        assert!(processor.recording_tail_remaining.is_some());
        assert_eq!(processor.loops[0].len, 1);
    }

    #[test]
    fn routes_metronome_and_synth_and_acknowledges_shutdown() {
        let (mut processor, mut controls) = processor(0.1);
        controls
            .try_command(RuntimeCommand::SetPulse { frames: 4 })
            .unwrap();
        controls
            .try_command(RuntimeCommand::SetMetronome {
                enabled: true,
                gain: 0.1,
            })
            .unwrap();
        controls
            .try_command(RuntimeCommand::SynthNote {
                note: 69,
                velocity: 127,
            })
            .unwrap();
        assert!(
            run(&mut processor, &[0.0; 4], &[0.0; 4])[0]
                .iter()
                .any(|sample| *sample != 0.0)
        );
        controls.try_command(RuntimeCommand::Shutdown).unwrap();
        assert_eq!(run(&mut processor, &[1.0], &[1.0])[0], [0.0]);
        assert_eq!(controls.try_status(), Some(RuntimeStatus::ShutdownComplete));
        assert!(processor.synth.shutdown);
    }

    #[test]
    fn production_metronome_uses_cpp_hit_timing_and_decay_buffer() {
        let (mut processor, mut controls) = processor(0.0);
        controls
            .try_command(RuntimeCommand::SetPulse { frames: 4 })
            .unwrap();
        controls
            .try_command(RuntimeCommand::SetMetronome {
                enabled: true,
                gain: 1.0,
            })
            .unwrap();
        // Pulse's constructor starts metroofs at its hit length, so enabling
        // it does not inject a click before the first actual downbeat.
        assert_eq!(run(&mut processor, &[0.0; 4], &[0.0; 4])[0], [0.0; 4]);
        let after_wrap = run(&mut processor, &[0.0], &[0.0]);
        // The final shared limiter has begun its tiny release ramp; verify
        // the generated hit before that <0.01% downstream scaling.
        assert!((after_wrap[0][0] - processor.metro_noise[0]).abs() < 0.000_01);
        assert_eq!(after_wrap[1][0], after_wrap[0][0]);
    }

    #[test]
    fn routes_fixed_size_midi_patch_and_tuning_commands() {
        let (mut processor, mut controls) = processor(0.0);
        for command in [
            RuntimeCommand::SetSynthChannel(5),
            RuntimeCommand::SynthNote {
                note: 64,
                velocity: 99,
            },
            RuntimeCommand::SynthController {
                channel: 2,
                control: 74,
                value: 81,
            },
            RuntimeCommand::SynthPitchBend {
                channel: 3,
                value: 12_345,
            },
            RuntimeCommand::SynthPatch {
                channel: 4,
                soundfont_id: 7,
                bank: 8,
                program: 9,
            },
            RuntimeCommand::SynthTuning { cents: -12.5 },
        ] {
            controls.try_command(command).unwrap();
        }
        run(&mut processor, &[0.0], &[0.0]);
        assert_eq!(processor.synth.note, Some((5, 64, 99)));
        assert_eq!(processor.synth.controller, Some((5, 74, 81)));
        assert_eq!(processor.synth.bend, Some((5, 12_345 + PITCH_BEND_CENTER)));
        assert_eq!(processor.synth.patch, Some((4, 7, 8, 9)));
        assert_eq!(processor.synth.tuning, -12.5);
    }

    #[test]
    fn configured_mono_synth_matches_cpp_left_only_external_input() {
        let (mut processor, mut controls) = processor(0.25);
        controls
            .try_command(RuntimeCommand::SetSynthStereo(false))
            .unwrap();
        let output = run(&mut processor, &[0.0; 2], &[0.0; 2]);
        assert!(
            output[0]
                .iter()
                .all(|sample| (*sample - 0.375).abs() < 0.000_02)
        );
        assert!(output[1].iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn final_limiter_controls_a_hot_multi_loop_mix_without_unbounded_output() {
        // Five unity-gain loop sources can easily sum beyond the output
        // format range.  The C++-compatible limiter must link the channels,
        // lower their common gain envelope, and retain only its 0.99 safety
        // guard while that 1024-frame attack settles.
        let mut limiter = MasterLimiter::with_settings(DspSettings::default());
        let mut left = vec![4.0; 2_048];
        let mut right = vec![-2.0; 2_048];
        limiter.process_stereo(&mut left, &mut right, DspSettings::default());

        assert!(left.iter().all(|sample| sample.abs() <= 0.99));
        assert!(right.iter().all(|sample| sample.abs() <= 0.99));
        assert!(limiter.current_gain < 0.35);
        assert!(limiter.target_gain < 0.25);
    }

    #[test]
    fn unsynchronised_recording_smooths_its_endpoint_like_cpp_audioblock() {
        let mut slot = LoopSlot::new();
        slot.left = vec![-0.5; LOOP_SMOOTH_FRAMES];
        slot.right = vec![0.25; LOOP_SMOOTH_FRAMES];
        slot.left
            .extend(std::iter::repeat_n(0.5, LOOP_SMOOTH_FRAMES));
        slot.right
            .extend(std::iter::repeat_n(-0.25, LOOP_SMOOTH_FRAMES));
        slot.len = slot.left.len();

        smooth_unsynchronised_loop_endpoints(&mut slot);

        // C++ `AudioBlock::Smooth(1)` leaves the first tail sample unchanged,
        // blends the beginning into the final tail sample, then removes the
        // 64 head frames that were consumed by the crossfade.
        assert_eq!(slot.len, LOOP_SMOOTH_FRAMES);
        assert_eq!(slot.sample_at(0).0, 0.5);
        assert_eq!(slot.left[LOOP_SMOOTH_FRAMES], 0.5);
        assert!(
            (slot.sample_at(slot.len - 1).0 - (-0.5 + 1.0 / LOOP_SMOOTH_FRAMES as f32)).abs()
                < 0.000_001
        );
        assert!(
            (slot.sample_at(slot.len - 1).1 - (0.25 - 0.5 / LOOP_SMOOTH_FRAMES as f32)).abs()
                < 0.000_001
        );
    }

    #[test]
    fn active_loop_gain_delta_is_applied_before_cpp_playback_not_while_muted() {
        let (mut processor, _controls) = processor(0.0);
        let slot = &mut processor.loops[0];
        slot.left = vec![1.0];
        slot.right = vec![1.0];
        slot.len = 1;
        slot.mode = LoopMode::Playing;
        slot.gain = 0.001;
        slot.gain_delta = 2.0;

        let played = run(&mut processor, &[0.0], &[0.0]);
        // `Loop::UpdateVolume`: lift from MIN_VOL then multiply before the
        // same fragment is rendered.
        assert!((played[0][0] - 0.02).abs() < 1e-6);
        assert!((processor.loops[0].gain - 0.02).abs() < 1e-6);

        processor.loops[0].mode = LoopMode::Muted;
        processor.loops[0].gain_delta = 2.0;
        run(&mut processor, &[0.0], &[0.0]);
        assert!((processor.loops[0].gain - 0.02).abs() < 1e-6);
    }

    #[test]
    fn synchronised_loop_crossfades_at_its_restart() {
        let (mut processor, _controls) = processor(0.0);
        let slot = &mut processor.loops[0];
        slot.left = vec![-0.5; LOOP_SMOOTH_FRAMES];
        slot.right = vec![-0.5; LOOP_SMOOTH_FRAMES];
        slot.left
            .extend(std::iter::repeat_n(0.5, LOOP_SMOOTH_FRAMES));
        slot.right
            .extend(std::iter::repeat_n(0.5, LOOP_SMOOTH_FRAMES));
        slot.len = slot.left.len();
        slot.position = slot.len - 1;
        slot.mode = LoopMode::Playing;
        slot.pulse_synced = true;

        let output = run(&mut processor, &[0.0; 3], &[0.0; 3]);
        // Final old sample, then the old tail at the first new-loop sample,
        // then C++'s one-64th fade toward the new loop head.
        assert!((output[0][0] - 0.5).abs() < 0.001);
        assert!((output[0][1] - 0.5).abs() < 0.001);
        assert!((output[0][2] - (0.5 - 1.0 / LOOP_SMOOTH_FRAMES as f32)).abs() < 0.001);
    }

    #[test]
    fn synced_playback_skips_the_record_tail_at_each_pulse_boundary() {
        let (mut processor, _controls) = processor(0.0);
        processor.pulse_sync_active = true;
        processor.pulse_frames = 4;
        processor.pulse_position = 3;
        processor.pulse_long_count = 0;
        processor.pulse_long_length = 2;
        let slot = &mut processor.loops[0];
        slot.left = (0..(8 + REC_TAIL_FRAMES))
            .map(|sample| sample as f32 * 0.01)
            .collect();
        slot.right = slot.left.clone();
        slot.len = slot.left.len();
        slot.position = 7;
        slot.mode = LoopMode::Playing;
        slot.pulse_synced = true;
        slot.pulse_beats = 2;

        let output = run(&mut processor, &[0.0; 2], &[0.0; 2]);
        // First sample completes beat zero. On the following downbeat C++
        // `PlayProcessor::PulseSync` jumps to beat one (offset 4), rather
        // than allowing the iterator to enter the 1,024-frame record tail.
        for channel in output {
            assert!((channel[0] - 0.07).abs() < 0.000_01);
            assert!((channel[1] - 0.04).abs() < 0.000_01);
        }
    }

    #[test]
    fn synced_overdub_fades_the_old_fragment_out_and_new_beat_in() {
        let (mut processor, _controls) = processor(0.0);
        processor.pulse_sync_active = true;
        processor.pulse_frames = 4;
        processor.pulse_position = 2;
        processor.pulse_long_count = 0;
        processor.pulse_long_length = 1;
        let slot = &mut processor.loops[0];
        slot.left = (0..(8 + REC_TAIL_FRAMES))
            .map(|sample| sample as f32 * 0.01)
            .collect();
        slot.right = slot.left.clone();
        slot.len = slot.left.len();
        slot.position = 2;
        slot.mode = LoopMode::Overdubbing;
        slot.pulse_synced = true;
        slot.pulse_beats = 1;
        slot.feedback = 0.5;

        // Cache raw positions 2 and 3, then cross the downbeat. The next
        // process sample makes `RecordProcessor::Jump` revise old position 2
        // with FadeOut_Input and write FadeIn_Input at new beat position 0.
        run(&mut processor, &[0.01, 0.02], &[0.01, 0.02]);
        let output = run(&mut processor, &[0.03], &[0.03]);

        // Fadepre starts at the old fragment, while FadeOut_Input starts with
        // full current input and FadeIn_Input starts with no new input.
        assert!(
            (output[0][0] - 0.02).abs() < 0.000_01,
            "left output was {:?}",
            output[0]
        );
        assert!(
            (output[1][0] - 0.02).abs() < 0.000_01,
            "right output was {:?}",
            output[1]
        );
        assert!((processor.loops[0].sample_at(2).0 - 0.04).abs() < 0.000_01);
        assert!((processor.loops[0].sample_at(0).0).abs() < 0.000_01);
    }

    #[test]
    fn supports_high_u8_loop_ids_without_per_id_sample_buffers() {
        let (mut processor, mut controls) = processor(0.0);
        controls
            .try_command(RuntimeCommand::Record { slot: 255 })
            .unwrap();
        run(&mut processor, &[0.75], &[0.25]);
        controls.try_command(RuntimeCommand::StopRecord).unwrap();
        let played = run(&mut processor, &[0.0], &[0.0]);
        assert!((played[0][0] - 0.75).abs() < 0.001);
        assert!((played[1][0] - 0.25).abs() < 0.001);
        assert!(
            processor.loops[..255]
                .iter()
                .all(|slot| slot.left.is_empty() && slot.blocks.is_empty())
        );
    }

    #[test]
    fn recording_storage_is_replenished_beyond_initial_cpp_ready_set() {
        let (mut processor, mut controls) = processor(0.0);
        // The original starts with 40 ready blocks but `MemoryManager`
        // refills one asynchronously after every RTNew. Consume that startup
        // set, then give the real manager-thread boundary a chance to run
        // before requiring the next block; an instantaneous 41-command burst
        // is allowed to exhaust either implementation's ready list.
        for slot in 0..DEFAULT_AUDIO_BLOCKS as u8 {
            controls
                .try_command(RuntimeCommand::Record { slot })
                .unwrap();
            run(&mut processor, &[0.0], &[0.0]);
        }
        // Poll the real worker/refill ring instead of assuming the test host
        // schedules its manager thread within one arbitrary 10ms slice.
        // C++ has the same asynchronous handoff; the condition being tested
        // is that a replacement eventually becomes ready, not scheduler
        // latency under a concurrently running test suite.
        for _ in 0..100 {
            controls.service_loop_storage();
            processor.loop_storage.collect_refills();
            if !processor.loop_storage.free.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!processor.loop_storage.free.is_empty());
        controls
            .try_command(RuntimeCommand::Record {
                slot: DEFAULT_AUDIO_BLOCKS as u8,
            })
            .unwrap();
        run(&mut processor, &[0.0], &[0.0]);
        let mut exhausted = false;
        while let Some(status) = controls.try_status() {
            exhausted |= matches!(
                status,
                RuntimeStatus::TransferError {
                    error: PcmTransferError::RecordingStorageExhausted,
                    ..
                }
            );
        }
        assert!(!exhausted);
        assert!(
            processor.loops[..=DEFAULT_AUDIO_BLOCKS]
                .iter()
                .all(LoopSlot::uses_blocks)
        );
    }

    #[test]
    fn recording_grows_through_cpp_sized_blocks_without_callback_allocation() {
        let (mut processor, mut controls) = runtime_audio_processor_with_backend(
            FakeSynth::default(),
            48_000,
            AUDIO_BLOCK_FRAMES + 1,
            AUDIO_BLOCK_FRAMES + 1,
        );
        controls
            .try_command(RuntimeCommand::Record { slot: 0 })
            .unwrap();
        run(
            &mut processor,
            &vec![0.25; AUDIO_BLOCK_FRAMES + 1],
            &vec![-0.5; AUDIO_BLOCK_FRAMES + 1],
        );
        assert_eq!(processor.loops[0].len, AUDIO_BLOCK_FRAMES + 1);
        assert_eq!(processor.loops[0].blocks.len(), 2);
        assert_eq!(
            processor.loops[0].sample_at(AUDIO_BLOCK_FRAMES),
            (0.25, -0.5)
        );
    }

    #[test]
    fn recording_chain_grows_past_the_initial_forty_blocks() {
        let (mut processor, mut controls) = runtime_audio_processor_with_backend(
            FakeSynth::default(),
            48_000,
            CPP_AUDIO_POOL_FRAMES,
            AUDIO_BLOCK_FRAMES,
        );
        controls.service_loop_storage();
        controls
            .try_command(RuntimeCommand::Record { slot: 0 })
            .unwrap();
        let input = vec![0.25; AUDIO_BLOCK_FRAMES];
        for _ in 0..=DEFAULT_AUDIO_BLOCKS {
            run(&mut processor, &input, &input);
            // This is the non-RT `MemoryManager` turn between callback
            // periods. It makes the next `RTNew`-equivalent block available.
            controls.service_loop_storage();
        }
        assert_eq!(
            processor.loops[0].len,
            CPP_AUDIO_POOL_FRAMES + AUDIO_BLOCK_FRAMES
        );
        assert_eq!(processor.loops[0].blocks.len(), DEFAULT_AUDIO_BLOCKS + 1);
        assert_eq!(
            processor.loops[0].sample_at(CPP_AUDIO_POOL_FRAMES),
            (0.25, 0.25)
        );
    }

    #[test]
    fn block_chain_tail_stays_valid_after_head_release() {
        let block = |value| {
            Box::new(LoopStorageBlock {
                storage: StereoTransfer {
                    left: vec![value; AUDIO_BLOCK_FRAMES],
                    right: vec![-value; AUDIO_BLOCK_FRAMES],
                    len: 0,
                },
                next: None,
            })
        };
        let mut chain = LoopBlockChain::default();
        chain.append(block(1.0));
        chain.append(block(2.0));
        let _ = chain.pop_first();
        chain.append(block(3.0));
        assert_eq!(chain.len(), 2);
        assert_eq!(chain.block_at(0).storage.left[0], 2.0);
        assert_eq!(chain.block_at(1).storage.left[0], 3.0);
        let _ = chain.pop_first();
        let _ = chain.pop_first();
        assert!(chain.is_empty());
        chain.append(block(4.0));
        assert_eq!(chain.block_at(0).storage.left[0], 4.0);
    }

    #[test]
    fn scope_columns_use_cpp_stereo_range_and_mean_absolute_amplitude() {
        let (mut processor, _controls) = processor(0.0);
        let slot = &mut processor.loops[0];
        slot.left = vec![0.75; PEAK_AVG_CHUNK_FRAMES];
        slot.right = vec![-0.25; PEAK_AVG_CHUNK_FRAMES];
        slot.len = PEAK_AVG_CHUNK_FRAMES;
        slot.mode = LoopMode::Playing;
        for _ in 0..PEAK_AVG_CHUNK_FRAMES.div_ceil(SCOPE_REFRESH_SAMPLES_PER_CALLBACK) {
            processor.refresh_scopes();
        }

        // PeaksAvgsManager starts its extrema at zero, so this stereo range
        // is 0.75 - (-0.25) = 1.0. Its average is (|.75| + |-.25|) / 2.
        assert_eq!(processor.loops[0].scope.peaks[0], 1.0);
        assert_eq!(processor.loops[0].scope.averages[0], 0.5);
    }

    #[test]
    fn imported_pcm_arrives_with_control_thread_scope_columns() {
        let (mut processor, mut controls) = runtime_audio_processor_with_backend(
            FakeSynth::default(),
            48_000,
            PEAK_AVG_CHUNK_FRAMES,
            PEAK_AVG_CHUNK_FRAMES,
        );
        let handle = controls.try_acquire_transfer().unwrap();
        controls
            .write_transfer(
                handle,
                &vec![0.75; PEAK_AVG_CHUNK_FRAMES],
                &vec![-0.25; PEAK_AVG_CHUNK_FRAMES],
            )
            .unwrap();
        controls
            .try_import_loop(handle.index as u8, handle, 0, LoopMode::Playing, 1.0)
            .unwrap();
        run(&mut processor, &[], &[]);
        let scope = &processor.loops[handle.index as usize].scope;
        assert!(scope.complete);
        assert_eq!(scope.column, 1);
        assert_eq!(scope.peaks[0], 1.0);
        assert_eq!(scope.averages[0], 0.5);
    }

    #[test]
    fn native_recording_generates_scope_columns_without_playback_scanning() {
        let (mut processor, mut controls) = runtime_audio_processor_with_backend(
            FakeSynth::default(),
            48_000,
            PEAK_AVG_CHUNK_FRAMES,
            PEAK_AVG_CHUNK_FRAMES,
        );
        controls
            .try_command(RuntimeCommand::Record { slot: 0 })
            .unwrap();
        run(
            &mut processor,
            &vec![0.75; PEAK_AVG_CHUNK_FRAMES],
            &vec![-0.25; PEAK_AVG_CHUNK_FRAMES],
        );

        assert!(processor.loops[0].uses_blocks());
        assert_eq!(processor.loops[0].scope.column, 1);
        assert_eq!(processor.loops[0].scope.peaks[0], 1.0);
        assert_eq!(processor.loops[0].scope.averages[0], 0.5);
    }
}
