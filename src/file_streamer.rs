//! Disk output streaming from the realtime audio callback.
//! Spawns an encode thread connected via a lock-free ring buffer.
//! Used for DAW export and stem recording (ToggleDiskOutput).

use crate::audioio::{NFrames, Sample};
use crate::block::Codec;
use crate::file_codecs::{IFileEncoder, SndFileEncoder};
use rtrb::{Consumer, Producer, RingBuffer};
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::thread::{self, JoinHandle};

/// Number of PCM blocks in the ring buffer between audio callback and encode
/// thread.  128 blocks of ~4096 frames each = ~131k frames of headroom.
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
    status: Arc<AtomicU8>,
}

impl PcmOutput {
    /// Push one stereo PCM block into the ring buffer.
    /// Returns `false` if the buffer is full or stop/error has been signaled.
    pub fn push_audio(&mut self, left: &[Sample], right: &[Sample], frames: NFrames) -> bool {
        let s = self.status.load(Ordering::Relaxed);
        if s >= STATUS_STOP_PENDING {
            return false;
        }
        let cap = left.len().min(right.len()).min(frames as usize);
        let block = PcmBlock {
            left: left[..cap].to_vec().into_boxed_slice(),
            right: right[..cap].to_vec().into_boxed_slice(),
            frames: cap as NFrames,
        };
        self.producer.push(block).is_ok()
    }

    /// Whether the encode thread has been asked to stop (e.g. for the
    /// processor to skip future pushes without allocating blocks).
    pub fn is_stopping(&self) -> bool {
        self.status.load(Ordering::Relaxed) >= STATUS_STOP_PENDING
    }
}

/// Control-side disk-output streamer.  Owns the encode thread and provides
/// start/stop/finalize lifecycle for the control thread.  Each call to
/// `start_writing` returns a `PcmOutput` that must be installed into the
/// realtime audio processor.
pub struct AudioStreamer {
    encode_thread: Option<JoinHandle<()>>,
    status: Arc<AtomicU8>,
    bytes_written: Arc<AtomicU64>,
    output_path: Option<PathBuf>,
    result: Option<Result<(), String>>,
}

impl AudioStreamer {
    pub fn new() -> Self {
        Self {
            encode_thread: None,
            status: Arc::new(AtomicU8::new(STATUS_IDLE)),
            bytes_written: Arc::new(AtomicU64::new(0)),
            output_path: None,
            result: None,
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
    ) -> Result<PcmOutput, String> {
        let s = self.status.load(Ordering::Acquire);
        if s != STATUS_IDLE {
            return Err("streamer is already active".into());
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
        let (producer, consumer) =
            RingBuffer::<PcmBlock>::new(DEFAULT_BUFFER_BLOCKS);

        self.status.store(STATUS_WRITING, Ordering::Release);
        let status = Arc::clone(&self.status);
        let bytes_written = Arc::new(AtomicU64::new(0));
        let bw = Arc::clone(&bytes_written);
        let out_path = path.clone();
        let handle = thread::Builder::new()
            .name("fweelin-stream".into())
            .spawn(move || {
                run_encode_thread(consumer, out_path, format, samplerate, stereo, status, bw);
            })
            .map_err(|e| format!("spawn stream thread: {e}"))?;

        self.encode_thread = Some(handle);
        self.bytes_written = bytes_written;
        self.output_path = Some(path.clone());
        self.result = None;

        Ok(PcmOutput {
            producer,
            status: Arc::clone(&self.status),
        })
    }

    /// Request a graceful stop.  The encode thread will drain remaining blocks
    /// and close the output file.
    pub fn request_stop(&mut self) {
        self.status.store(STATUS_STOP_PENDING, Ordering::Release);
    }

    /// Block until the encode thread finishes and the file is closed.
    /// Call from the control thread after `request_stop` or when the stream
    /// naturally ends.  Returns the final `Result` (failure means the encoder
    /// closed with an error; the partial file has already been removed).
    pub fn finalize(&mut self) -> Result<(), String> {
        self.request_stop();
        if let Some(handle) = self.encode_thread.take() {
            let _ = handle.join().map_err(|_| "stream thread panicked")?;
        }
        let result = self.result.take().unwrap_or(Ok(()));
        if result.is_err() {
            if let Some(path) = &self.output_path {
                let _ = fs::remove_file(path);
            }
        }
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
}

impl Drop for AudioStreamer {
    fn drop(&mut self) {
        if self.encode_thread.is_some() {
            self.request_stop();
            if let Some(handle) = self.encode_thread.take() {
                let _ = handle.join();
            }
            if self.result.as_ref().map_or(false, |r| r.is_err()) {
                if let Some(path) = &self.output_path {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
}

/// Background encode thread.  Creates the output file and encoder, then loops
/// popping blocks from the consumer and writing them until stop is signaled.
fn run_encode_thread(
    mut consumer: Consumer<PcmBlock>,
    path: PathBuf,
    format: Codec,
    samplerate: u32,
    stereo: bool,
    status: Arc<AtomicU8>,
    bytes_written: Arc<AtomicU64>,
) {
    // Create output file and encoder inside the thread so we don't need
    // SndFileEncoder (containing raw vorbis pointers) to be Send.
    let file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(_) => {
            status.store(STATUS_ERROR, Ordering::Release);
            return;
        }
    };
    let mut encoder = match SndFileEncoder::new(samplerate, stereo, format) {
        Ok(e) => e,
        Err(_) => {
            let _ = fs::remove_file(&path);
            status.store(STATUS_ERROR, Ordering::Release);
            return;
        }
    };
    if encoder.setup_file_for_writing(file).is_err() {
        let _ = fs::remove_file(&path);
        status.store(STATUS_ERROR, Ordering::Release);
        return;
    }

    loop {
        let s = status.load(Ordering::Acquire);
        if s == STATUS_STOP_PENDING {
            // Drain remaining blocks before closing.
            while let Ok(block) = consumer.pop() {
                let n = encoder
                    .write_samples_to_disk(&block.left, Some(&block.right))
                    .unwrap_or(0);
                bytes_written.fetch_add((n * 8) as u64, Ordering::Release);
            }
            if encoder.prepare_file_for_closing().is_err() {
                status.store(STATUS_ERROR, Ordering::Release);
            } else {
                status.store(STATUS_IDLE, Ordering::Release);
            }
            return;
        }
        if s == STATUS_ERROR {
            return;
        }

        // Non-blocking pop; yield if empty so the thread stays responsive
        // without busy-waiting.
        match consumer.pop() {
            Ok(block) => {
                let n = encoder
                    .write_samples_to_disk(&block.left, Some(&block.right))
                    .unwrap_or(0);
                bytes_written.fetch_add((n * 8) as u64, Ordering::Release);
            }
            Err(_) => {
                std::thread::yield_now();
            }
        }
    }
}
