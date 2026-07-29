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
    status: Arc<AtomicU8>,
}

impl PcmOutput {
    /// Push one stereo PCM block into the ring buffer.
    /// Returns `false` if the buffer is full or stop/error has been signaled.
    pub fn push_audio(&mut self, left: &[Sample], right: &[Sample], frames: NFrames) -> bool {
        let s = self.status.load(Ordering::Relaxed);
        if s != STATUS_WRITING {
            return false;
        }
        let cap = left.len().min(right.len()).min(frames as usize);
        while self.free.len() < self.free.capacity() {
            let Ok(block) = self.recycled.pop() else {
                break;
            };
            self.free.push(block);
        }
        let Some(mut block) = self.free.pop() else {
            self.status.store(STATUS_ERROR, Ordering::Release);
            return false;
        };
        if cap > block.left.len() || cap > block.right.len() {
            self.free.push(block);
            self.status.store(STATUS_ERROR, Ordering::Release);
            return false;
        }
        block.left[..cap].copy_from_slice(&left[..cap]);
        block.right[..cap].copy_from_slice(&right[..cap]);
        block.frames = cap as NFrames;
        match self.producer.push(block) {
            Ok(()) => true,
            Err(PushError::Full(block)) => {
                self.free.push(block);
                self.status.store(STATUS_ERROR, Ordering::Release);
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
        if max_callback_frames == 0 {
            return Err("stream callback size must be non-zero".into());
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

        self.status.store(STATUS_WRITING, Ordering::Release);
        let status = Arc::clone(&self.status);
        let bytes_written = Arc::new(AtomicU64::new(0));
        let bw = Arc::clone(&bytes_written);
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
                    },
                    status,
                    bw,
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
            status: Arc::clone(&self.status),
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
}

fn run_encode_thread(
    mut consumer: Consumer<PcmBlock>,
    mut recycled: Producer<PcmBlock>,
    settings: EncodeSettings,
    status: Arc<AtomicU8>,
    bytes_written: Arc<AtomicU64>,
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
                        write_block(&mut encoder, block, &mut recycled, &bytes_written)?;
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
                    write_block(&mut encoder, block, &mut recycled, &bytes_written)?;
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
    bytes_written: &AtomicU64,
) -> Result<(), String> {
    let frames = block.frames as usize;
    let written = encoder
        .write_samples_to_disk(&block.left[..frames], Some(&block.right[..frames]))
        .map_err(|error| format!("write stream samples: {error}"));
    let _ = recycled.push(block);
    let written = written?;
    if written != frames {
        return Err(format!(
            "short stream write: wrote {written} of {frames} frames"
        ));
    }
    bytes_written.fetch_add((written * 8) as u64, Ordering::Release);
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
