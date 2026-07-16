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

#[test]
fn owned_platform_callback_and_bounded_queues_do_not_allocate_or_lock() {
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
    io.activate_boxed(Box::new(BoxedGain(Arc::new(2.0))))
        .unwrap();
}
