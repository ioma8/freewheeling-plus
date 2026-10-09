use freewheeling_plus::audioio::{AudioBackend, JackPosition};
use freewheeling_plus::audioio_platform::AudioIoPlatform;
use freewheeling_plus::realtime_guard::{
    CallbackCountingAllocator, RealtimeMetrics, blocking_lock_attempts, callback_allocations,
    reset_violation_counters,
};
use freewheeling_plus::realtime_queue;
use std::sync::Arc;

#[global_allocator]
static ALLOCATOR: CallbackCountingAllocator = CallbackCountingAllocator;

/// Serializes the tests that assert on the process-global violation counters.
static COUNTER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn owned_platform_callback_and_bounded_queues_do_not_allocate_or_lock() {
    let _counters = COUNTER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let realtime = RealtimeMetrics::new(48_000, 128).unwrap();
    let (mut command_tx, mut command_rx) = realtime_queue::bounded(8);
    let (mut status_tx, mut status_rx) = realtime_queue::bounded(8);
    command_tx.try_send(2.0_f32).unwrap();

    let mut backend = AudioIoPlatform::new(48_000, 128);
    backend
        .activate(Box::new(move |callback| {
            let gain = command_rx.try_recv().unwrap_or(1.0);
            for frame in 0..callback.nframes as usize {
                callback.outputs[0][frame] = callback.inputs[0][frame] * gain;
                callback.outputs[1][frame] = callback.inputs[1][frame] * gain;
            }
            let _ = status_tx.try_send(callback.nframes);
        }))
        .unwrap();

    let input = [[0.25_f32; 128], [0.5_f32; 128]];
    let mut left = [0.0_f32; 128];
    let mut right = [0.0_f32; 128];
    reset_violation_counters();
    {
        let _guard = realtime.enter_callback();
        backend
            .invoke_callback(
                [&input[0], &input[1]],
                [&mut left, &mut right],
                128,
                JackPosition::default(),
            )
            .unwrap();
    }

    assert_eq!(callback_allocations(), 0);
    assert_eq!(blocking_lock_attempts(), 0);
    assert_eq!(status_rx.try_recv(), Some(128));
    assert!(left.iter().all(|sample| *sample == 0.5));
    assert!(right.iter().all(|sample| *sample == 1.0));
}

#[test]
fn boxed_processor_runs_without_callback_allocation() {
    let _counters = COUNTER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    use freewheeling_plus::audioio::{
        AudioCallback, AudioCallbackFn, AudioIO, AudioProcessor, BackendInfo,
    };

    struct ImmediateBackend;
    impl AudioBackend for ImmediateBackend {
        fn open(&mut self, _: &str) -> Result<BackendInfo, String> {
            Ok(BackendInfo {
                sample_rate: 48_000,
                buffer_size: 4,
            })
        }
        fn activate(&mut self, mut callback: AudioCallbackFn) -> Result<(), String> {
            let input = [1.0_f32; 4];
            let mut left = [0.0_f32; 4];
            let mut right = [0.0_f32; 4];
            let mut audio = AudioCallback {
                inputs: [&input, &input],
                outputs: [&mut left, &mut right],
                nframes: 4,
                position: JackPosition::default(),
                transport_rolling: false,
            };
            let metrics = RealtimeMetrics::new(48_000, 4).unwrap();
            reset_violation_counters();
            let _guard = metrics.enter_callback();
            callback(&mut audio);
            assert_eq!(callback_allocations(), 0);
            Ok(())
        }
        fn close(&mut self) {}
        fn relocate(&mut self, _: u32) {}
    }
    struct BoxedGain(Arc<f32>);
    impl AudioProcessor for BoxedGain {
        fn process(&mut self, callback: &mut AudioCallback<'_>) {
            callback.outputs[0][0] = callback.inputs[0][0] * *self.0;
        }
    }

    let mut io = AudioIO::new(ImmediateBackend);
    io.open("boxed").unwrap();
    io.activate(BoxedGain(Arc::new(2.0)))
        .unwrap();
}

#[test]
fn disk_stream_push_uses_only_preallocated_blocks() {
    let _counters = COUNTER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    use freewheeling_plus::block::Codec;
    use freewheeling_plus::file_streamer::AudioStreamer;
    use std::time::{SystemTime, UNIX_EPOCH};

    let path = std::env::temp_dir().join(format!(
        "freewheeling-rt-stream-{}-{}.wav",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut streamer = AudioStreamer::new();
    let mut output = streamer
        .start_writing(path.clone(), Codec::Wav, 48_000, true, 128)
        .unwrap();
    let left = [0.25; 128];
    let right = [-0.25; 128];
    let realtime = RealtimeMetrics::new(48_000, 128).unwrap();

    reset_violation_counters();
    {
        let _guard = realtime.enter_callback();
        assert!(output.push_audio(&left, &right, 128));
    }

    assert_eq!(callback_allocations(), 0);
    assert_eq!(blocking_lock_attempts(), 0);
    streamer.finalize().unwrap();
    std::fs::remove_file(path).unwrap();
}

/// Positive control for the instrumentation itself.
///
/// Every `assert_eq!(callback_allocations(), 0)` above passes vacuously if
/// `in_callback()` is always false or the counting allocator is not installed.
#[test]
fn the_violation_counters_actually_count() {
    let realtime = RealtimeMetrics::new(48_000, 128).unwrap();
    // Serialized with the other counter assertions: the counters are
    // process-global, so a concurrent test could observe this allocation.
    // (`COUNTER_LOCK` is not reentrant: acquire it exactly once per test.)
    let _serialized = COUNTER_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    reset_violation_counters();
    {
        let _guard = realtime.enter_callback();
        let allocated = vec![0u8; 1024];
        std::hint::black_box(&allocated);
        assert!(
            callback_allocations() > 0,
            "a heap allocation inside a callback window was not counted"
        );
    }
    reset_violation_counters();
}

/// The production DSP graph — the code that used to allocate while recording
/// crossed a `LoopSlot.blocks` deque boundary — must also record a long loop
/// with zero allocations on the audio thread.
#[test]
fn recording_into_block_storage_allocates_nothing_on_the_audio_thread() {
    let _counters = COUNTER_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    use freewheeling_plus::audioio::{AudioCallback, AudioProcessor};
    use freewheeling_plus::core_dsp::Sample;
    use freewheeling_plus::fluidsynth::{FluidSynthBackend, Patch};
    use freewheeling_plus::native_dsp_graph::RuntimeCommand;
    use freewheeling_plus::native_dsp_graph::runtime_audio_processor_with_backend;

    struct SilentSynth;
    impl FluidSynthBackend for SilentSynth {
        fn render(&mut self, left: &mut [Sample], right: &mut [Sample]) {
            left.iter_mut().for_each(|sample| *sample = 0.0);
            right.iter_mut().for_each(|sample| *sample = 0.0);
        }
        fn controller(&mut self, _: u8, _: u8, _: u8) {}
        fn pitch_bend(&mut self, _: u8, _: i32) {}
        fn note_on(&mut self, _: u8, _: i32, _: u8) {}
        fn note_off(&mut self, _: u8, _: i32) {}
        fn program_select(&mut self, _: u8, _: i32, _: i32, _: i32) {}
        fn patches(&self) -> Vec<Patch> {
            Vec::new()
        }
    }

    // 1,228,800 frames of recording is 61 blocks of 20,000 frames: enough to
    // cross the preallocated deque's capacity growth boundaries several times.
    const CALLBACKS: usize = 2_400;
    let frames = 512;
    let metrics = RealtimeMetrics::new(48_000, frames as u32).unwrap();
    let (mut processor, mut controls) =
        runtime_audio_processor_with_backend(SilentSynth, 48_000, frames, frames);
    let input = vec![0.1_f32; frames];
    let mut left = vec![0.0_f32; frames];
    let mut right = vec![0.0_f32; frames];

    reset_violation_counters();
    let _callback = metrics.enter_callback();
    controls
        .try_command(RuntimeCommand::Record { slot: 0, presslen_ms: 0 })
        .map_err(|command| format!("record command rejected: {command:?}"))
        .unwrap();
    for _ in 0..CALLBACKS {
        let mut callback = AudioCallback {
            inputs: [&input[..], &input[..]],
            outputs: [&mut left[..], &mut right[..]],
            nframes: frames as u32,
            position: JackPosition::default(),
            transport_rolling: false,
        };
        processor.process(&mut callback);
        controls.service_loop_storage();
    }
    drop(_callback);

    assert_eq!(
        callback_allocations(),
        0,
        "the DSP callback allocated while recording block-chain storage"
    );
    assert_eq!(blocking_lock_attempts(), 0);
}
