//! Disk output streaming from the realtime audio callback.
//! Spawns an encode thread connected via a lock-free ring buffer.
//! Used for DAW export and stem recording (ToggleDiskOutput).

use crate::audioio::{NFrames, Sample};
use crate::block::Codec;
use crate::file_codecs::{IFileEncoder, SndFileEncoder};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Number of negotiated-size PCM callback blocks buffered for the encoder.
const DEFAULT_BUFFER_BLOCKS: usize = 128;

const STATUS_IDLE: u8 = 0;
const STATUS_WRITING: u8 = 1;
const STATUS_STOP_PENDING: u8 = 2;
const STATUS_ERROR: u8 = 3;

/// A PCM frame block pushed from the audio callback to the encode thread.
pub struct PcmBlock {
    pub left: Box<[Sample]>,
    pub right: Box<[Sample]>,
    pub frames: NFrames,
}

/// Audio-side producer handle.  Installed into `RuntimeAudioProcessor` so the
/// realtime callback can push PCM blocks into the ring buffer.
pub struct PcmOutput {
    producer: Producer<PcmBlock>,
    recycled: Consumer<PcmBlock>,
    free: Vec<PcmBlock>,
    /// Pool size this handle was created with; `Vec::capacity` is only an
    /// allocation hint and cannot be relied on once a block is lost.
    pool_blocks: usize,
    status: Arc<AtomicU8>,
    overruns: Arc<AtomicU64>,
}

impl PcmOutput {
    /// Push one stereo PCM block into the ring buffer.
    ///
    /// Returns `false` when the block was not queued. A full ring or a
    /// momentarily exhausted pool only drops this block and counts an overrun:
    /// ending (and deleting) a whole take because one callback was late is not
    /// a recovery policy.
    pub fn push_audio(&mut self, left: &[Sample], right: &[Sample], frames: NFrames) -> bool {
        let s = self.status.load(Ordering::Relaxed);
        if s != STATUS_WRITING {
            return false;
        }
        let frames = frames as usize;
        if left.len() != right.len() || frames > left.len() {
            // Mismatched channels or a frame count beyond the buffers: the
            // caller's block is unusable, so it is dropped rather than
            // truncated and written misaligned.
            self.overruns.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if frames > self.free.first().map_or(0, |block| block.left.len()) {
            // Blocks are preallocated for the configured callback size.
            self.overruns.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        while self.free.len() < self.pool_blocks {
            let Ok(block) = self.recycled.pop() else {
                break;
            };
            self.free.push(block);
        }
        let Some(mut block) = self.free.pop() else {
            self.overruns.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        block.left[..frames].copy_from_slice(&left[..frames]);
        block.right[..frames].copy_from_slice(&right[..frames]);
        block.frames = frames as NFrames;
        match self.producer.push(block) {
            Ok(()) => true,
            Err(PushError::Full(block)) => {
                self.free.push(block);
                self.overruns.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Whether this handle should be retired by the audio processor.
    pub fn is_finished(&self) -> bool {
        self.status.load(Ordering::Acquire) != STATUS_WRITING
    }
}

/// Control-side disk-output streamer.  Owns the encode thread and provides
/// start/stop/finalize lifecycle for the control thread.  Each call to
/// `start_writing` returns a `PcmOutput` that must be installed into the
/// realtime audio processor.
pub struct AudioStreamer {
    encode_thread: Option<JoinHandle<Result<(), String>>>,
    status: Arc<AtomicU8>,
    bytes_written: Arc<AtomicU64>,
    /// Blocks the audio callback could not queue (full ring or empty pool).
    overruns: Arc<AtomicU64>,
    /// Blocks the encode thread could not return to the recycle ring.
    lost_blocks: Arc<AtomicU64>,
    output_path: Option<PathBuf>,
}

impl Default for AudioStreamer {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioStreamer {
    pub fn new() -> Self {
        Self {
            encode_thread: None,
            status: Arc::new(AtomicU8::new(STATUS_IDLE)),
            bytes_written: Arc::new(AtomicU64::new(0)),
            overruns: Arc::new(AtomicU64::new(0)),
            lost_blocks: Arc::new(AtomicU64::new(0)),
            output_path: None,
        }
    }

    /// Start writing to a file.  Creates the ring buffer, spawns the encode
    /// thread, and returns a `PcmOutput` for the audio callback.
    pub fn start_writing(
        &mut self,
        path: PathBuf,
        format: Codec,
        samplerate: u32,
        stereo: bool,
        max_callback_frames: usize,
    ) -> Result<PcmOutput, String> {
        let s = self.status.load(Ordering::Acquire);
        if s != STATUS_IDLE {
            return Err("streamer is already active".into());
        }
        if max_callback_frames == 0
            || max_callback_frames > crate::file_codecs::MAX_STREAMING_FRAMES
        {
            // The pool preallocates `DEFAULT_BUFFER_BLOCKS` blocks of two f32
            // channels each, so an unbounded callback size would allocate
            // gigabytes (or abort) instead of failing cleanly.
            return Err(format!(
                "stream callback size must be 1..={} frames",
                crate::file_codecs::MAX_STREAMING_FRAMES
            ));
        }

        // Create output directory and validate format before spawning thread.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create stream directory: {e}"))?;
        }
        // Quick validation that the encoder can be created.
        let _encoder = SndFileEncoder::new(samplerate, stereo, format)
            .map_err(|e| format!("create stream encoder: {e}"))?;

        // Split the ring buffer.
        let (producer, consumer) = RingBuffer::<PcmBlock>::new(DEFAULT_BUFFER_BLOCKS);
        let (recycle_producer, recycled) =
            RingBuffer::<PcmBlock>::new(DEFAULT_BUFFER_BLOCKS);
        let mut free = Vec::with_capacity(DEFAULT_BUFFER_BLOCKS);
        for _ in 0..DEFAULT_BUFFER_BLOCKS {
            free.push(PcmBlock {
                left: vec![0.0; max_callback_frames].into_boxed_slice(),
                right: vec![0.0; max_callback_frames].into_boxed_slice(),
                frames: 0,
            });
        }

        // A fresh status per stream: a `PcmOutput` left over from the previous
        // generation must not be able to change this stream's state.
        self.status = Arc::new(AtomicU8::new(STATUS_WRITING));
        let status = Arc::clone(&self.status);
        let bytes_written = Arc::new(AtomicU64::new(0));
        let bw = Arc::clone(&bytes_written);
        let lost_blocks = Arc::clone(&self.lost_blocks);
        let bytes_per_frame = match format {
            Codec::Wav | Codec::Au => Some(if stereo { 8 } else { 4 }),
            // Compressed frames have no fixed size; the thread reports the
            // file length instead.
            _ => None,
        };
        let out_path = path.clone();
        let handle = match thread::Builder::new()
            .name("fweelin-stream".into())
            .spawn(move || {
                run_encode_thread(
                    consumer,
                    recycle_producer,
                    EncodeSettings {
                        path: out_path,
                        format,
                        samplerate,
                        stereo,
                        bytes_per_frame,
                    },
                    status,
                    bw,
                    lost_blocks,
                )
            })
        {
            Ok(handle) => handle,
            Err(error) => {
                self.status.store(STATUS_IDLE, Ordering::Release);
                return Err(format!("spawn stream thread: {error}"));
            }
        };

        self.encode_thread = Some(handle);
        self.bytes_written = bytes_written;
        self.output_path = Some(path.clone());

        Ok(PcmOutput {
            producer,
            recycled,
            free,
            pool_blocks: DEFAULT_BUFFER_BLOCKS,
            status: Arc::clone(&self.status),
            overruns: Arc::clone(&self.overruns),
        })
    }

    /// Request a graceful stop.  The encode thread will drain remaining blocks
    /// and close the output file.
    pub fn request_stop(&mut self) {
        let _ = self.status.compare_exchange(
            STATUS_WRITING,
            STATUS_STOP_PENDING,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Block until the encode thread finishes and the file is closed.
    /// Call from the control thread after `request_stop` or when the stream
    /// naturally ends.  Returns the final `Result` (failure means the encoder
    /// closed with an error; the partial file has already been removed).
    pub fn finalize(&mut self) -> Result<(), String> {
        self.request_stop();
        let result = self
            .encode_thread
            .take()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| "stream thread panicked".to_owned())?
            })
            .unwrap_or(Ok(()));
        self.status.store(STATUS_IDLE, Ordering::Release);
        self.output_path = None;
        result
    }

    /// Number of bytes written to disk so far (approximate, updated
    /// asynchronously by the encode thread).
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Acquire)
    }

    /// Whether the streamer is currently writing.
    pub fn is_writing(&self) -> bool {
        self.status.load(Ordering::Acquire) == STATUS_WRITING
    }

    /// Current status code.
    pub fn status(&self) -> u8 {
        self.status.load(Ordering::Acquire)
    }

    /// Blocks the audio callback could not queue for this stream.
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Acquire)
    }

    /// Blocks lost on their way back to the pool (recycle ring full).
    pub fn lost_blocks(&self) -> u64 {
        self.lost_blocks.load(Ordering::Acquire)
    }
}

impl Drop for AudioStreamer {
    fn drop(&mut self) {
        if self.encode_thread.is_some() {
            let _ = self.finalize();
        }
    }
}

/// Background encode thread.  Creates the output file and encoder, then loops
/// popping blocks from the consumer and writing them until stop is signaled.
struct EncodeSettings {
    path: PathBuf,
    format: Codec,
    samplerate: u32,
    stereo: bool,
    /// Bytes per written frame for PCM streams; `None` for compressed codecs,
    /// whose frames have no fixed size.
    bytes_per_frame: Option<u64>,
}

fn run_encode_thread(
    mut consumer: Consumer<PcmBlock>,
    mut recycled: Producer<PcmBlock>,
    settings: EncodeSettings,
    status: Arc<AtomicU8>,
    bytes_written: Arc<AtomicU64>,
    lost_blocks: Arc<AtomicU64>,
) -> Result<(), String> {
    // Create output file and encoder inside the thread so we don't need
    // SndFileEncoder (containing raw vorbis pointers) to be Send.
    let file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&settings.path)
    {
        Ok(file) => file,
        Err(error) => {
            status.store(STATUS_ERROR, Ordering::Release);
            return Err(format!(
                "create stream file '{}': {error}",
                settings.path.display()
            ));
        }
    };
    let result = (|| {
        let mut encoder =
            SndFileEncoder::new(settings.samplerate, settings.stereo, settings.format)
            .map_err(|error| format!("create stream encoder: {error}"))?;
        encoder
            .setup_file_for_writing(file)
            .map_err(|error| format!("open stream encoder: {error}"))?;

        loop {
            match status.load(Ordering::Acquire) {
                STATUS_STOP_PENDING => {
                    while let Ok(block) = consumer.pop() {
                        write_block(
                            &mut encoder,
                            block,
                            &mut recycled,
                            &settings,
                            &bytes_written,
                            &lost_blocks,
                        )?;
                    }
                    encoder
                        .prepare_file_for_closing()
                        .map_err(|error| format!("close stream file: {error}"))?;
                    return Ok(());
                }
                STATUS_ERROR => return Err("disk stream buffer overflow".into()),
                _ => {}
            }

            match consumer.pop() {
                Ok(block) => {
                    write_block(
                        &mut encoder,
                        block,
                        &mut recycled,
                        &settings,
                        &bytes_written,
                        &lost_blocks,
                    )?;
                }
                Err(_) => thread::park_timeout(Duration::from_millis(1)),
            }
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&settings.path);
    }
    status.store(
        if result.is_ok() {
            STATUS_IDLE
        } else {
            STATUS_ERROR
        },
        Ordering::Release,
    );
    result
}

fn write_block(
    encoder: &mut SndFileEncoder,
    block: PcmBlock,
    recycled: &mut Producer<PcmBlock>,
    settings: &EncodeSettings,
    bytes_written: &AtomicU64,
    lost_blocks: &AtomicU64,
) -> Result<(), String> {
    let frames = block.frames as usize;
    let written = encoder
        .write_samples_to_disk(&block.left[..frames], Some(&block.right[..frames]))
        .map_err(|error| format!("write stream samples: {error}"));
    // A block that cannot go back to the ring is lost from the pool; report it
    // instead of silently shrinking the pool for the rest of the session.
    if recycled.push(block).is_err() {
        lost_blocks.fetch_add(1, Ordering::Release);
    }
    let written = written?;
    if written != frames {
        return Err(format!(
            "short stream write: wrote {written} of {frames} frames"
        ));
    }
    match settings.bytes_per_frame {
        Some(bytes_per_frame) => {
            bytes_written.fetch_add(written as u64 * bytes_per_frame, Ordering::Release);
        }
        None => {
            // Compressed output: the file itself is the only accurate measure.
            if let Ok(metadata) = fs::metadata(&settings.path) {
                bytes_written.store(metadata.len(), Ordering::Release);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "freewheeling-stream-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn finalization_reports_worker_errors_without_deleting_an_existing_file() {
        let path = temporary("existing.wav");
        fs::write(&path, b"keep").unwrap();
        let mut streamer = AudioStreamer::new();
        let output = streamer
            .start_writing(path.clone(), Codec::Wav, 48_000, true, 32)
            .unwrap();

        let error = streamer.finalize().unwrap_err();

        assert!(error.contains("create stream file"));
        assert_eq!(fs::read(&path).unwrap(), b"keep");
        assert!(output.is_finished());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn preallocated_blocks_are_recycled_and_streamer_can_restart() {
        let directory = temporary("restart");
        fs::create_dir_all(&directory).unwrap();
        let mut streamer = AudioStreamer::new();
        let first_path = directory.join("first.wav");
        let mut first = streamer
            .start_writing(first_path.clone(), Codec::Wav, 48_000, true, 32)
            .unwrap();
        assert!(first.push_audio(&[0.25; 16], &[-0.25; 16], 16));
        streamer.finalize().unwrap();
        assert!(first.is_finished());
        assert!(first_path.is_file());

        let second_path = directory.join("second.wav");
        let mut second = streamer
            .start_writing(second_path.clone(), Codec::Wav, 48_000, true, 32)
            .unwrap();
        assert!(second.push_audio(&[0.5; 16], &[-0.5; 16], 16));
        streamer.finalize().unwrap();
        assert!(second.is_finished());
        assert!(second_path.is_file());

        fs::remove_dir_all(directory).unwrap();
    }
}
