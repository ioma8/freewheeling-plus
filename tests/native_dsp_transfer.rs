use freewheeling_plus::audioio::{AudioCallback, AudioProcessor, JackPosition};
use freewheeling_plus::fluidsynth::FluidSynthBackend;
use freewheeling_plus::native_dsp_graph::{
    DEFAULT_TRANSFER_SLOTS, EXPORT_COPY_FRAMES_PER_CALLBACK, LoopMode, PcmTransferError,
    RuntimeStatus, runtime_audio_processor_with_backend,
};
use freewheeling_plus::realtime_guard::{
    CallbackCountingAllocator, RealtimeMetrics, callback_allocations, reset_violation_counters,
};

#[global_allocator]
static ALLOCATOR: CallbackCountingAllocator = CallbackCountingAllocator;

#[derive(Default)]
struct SilentSynth;

impl FluidSynthBackend for SilentSynth {
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
    }
    fn controller(&mut self, _: u8, _: u8, _: u8) {}
    fn pitch_bend(&mut self, _: u8, _: i32) {}
    fn note_on(&mut self, _: u8, _: i32, _: u8) {}
    fn note_off(&mut self, _: u8, _: i32) {}
    fn program_select(&mut self, _: u8, _: i32, _: i32, _: i32) {}
    fn set_tuning(&mut self, _: f64) {}
    fn shutdown(&mut self) {}
}

fn process(processor: &mut impl AudioProcessor, frames: usize) -> ([f32; 4], [f32; 4]) {
    let input = [0.0; 4];
    let mut left = [0.0; 4];
    let mut right = [0.0; 4];
    processor.process(&mut AudioCallback {
        inputs: [&input[..frames], &input[..frames]],
        outputs: [&mut left[..frames], &mut right[..frames]],
        nframes: frames as u32,
        position: JackPosition::default(),
    });
    (left, right)
}

#[test]
fn imported_pcm_is_audible_and_export_roundtrips_exactly() {
    let (mut processor, mut controls) =
        runtime_audio_processor_with_backend(SilentSynth, 48_000, 16, 4);
    let imported = controls.try_acquire_transfer().unwrap();
    let source_left = [0.125, -0.25, 0.5, -0.75];
    let source_right = [-0.5, 0.25, -0.125, 0.75];
    controls
        .write_transfer(imported, &source_left, &source_right)
        .unwrap();
    controls
        .try_import_loop(3, imported, 0, LoopMode::Playing, 1.0)
        .unwrap();

    let audible = process(&mut processor, 4);
    // The C++ AutoLimitProcessor begins at unity then applies its configured
    // 0.000020 release increment after each sample, even below threshold.
    // Imported storage remains exact; only the audible master output follows
    // this final limiter envelope.
    for (index, (&actual, &source)) in audible.0.iter().zip(&source_left).enumerate() {
        // At frame zero the C++ adjustment also observes curlimitvol above
        // max gain and stops the positive release delta, so it is applied
        // exactly once.
        let expected = source * if index == 0 { 1.0 } else { 1.000_020 };
        assert!((actual - expected).abs() < 0.000_001, "left frame {index}");
    }
    for (index, (&actual, &source)) in audible.1.iter().zip(&source_right).enumerate() {
        let expected = source * if index == 0 { 1.0 } else { 1.000_020 };
        assert!((actual - expected).abs() < 0.000_001, "right frame {index}");
    }
    assert_eq!(
        controls.try_status(),
        Some(RuntimeStatus::LoopImported {
            slot: 3,
            handle: imported
        })
    );
    controls.release_transfer(imported).unwrap();

    let requested_export = controls.try_request_loop_export(3).unwrap();
    process(&mut processor, 0);
    let RuntimeStatus::LoopExported {
        slot,
        handle,
        metadata,
    } = controls.try_status().unwrap()
    else {
        panic!("expected exported PCM handle")
    };
    assert_eq!(slot, 3);
    assert_eq!(handle, requested_export);
    assert_eq!(metadata.frames, 4);
    controls
        .with_exported_pcm(handle, |left, right| {
            assert_eq!(left, source_left);
            assert_eq!(right, source_right);
        })
        .unwrap();
    controls.release_transfer(handle).unwrap();
    assert_eq!(
        controls.with_exported_pcm(handle, |_, _| ()),
        Err(PcmTransferError::InvalidHandle)
    );
}

#[test]
fn transfer_pool_exhaustion_is_explicit_and_reclamation_restores_capacity() {
    let (_processor, controls) = runtime_audio_processor_with_backend(SilentSynth, 48_000, 4, 4);
    let handles: Vec<_> = (0..DEFAULT_TRANSFER_SLOTS)
        .map(|_| controls.try_acquire_transfer().unwrap())
        .collect();
    assert_eq!(
        controls.try_acquire_transfer(),
        Err(PcmTransferError::PoolExhausted)
    );
    controls.release_transfer(handles[0]).unwrap();
    let recycled = controls.try_acquire_transfer().unwrap();
    assert_ne!(recycled, handles[0], "generation must reject stale handles");
    assert_eq!(
        controls.write_transfer(handles[0], &[1.0], &[1.0]),
        Err(PcmTransferError::InvalidHandle)
    );
}

#[test]
fn import_and_export_callbacks_allocate_nothing() {
    let (mut processor, mut controls) =
        runtime_audio_processor_with_backend(SilentSynth, 48_000, 16, 4);
    let imported = controls.try_acquire_transfer().unwrap();
    controls
        .write_transfer(imported, &[0.1, 0.2], &[0.3, 0.4])
        .unwrap();
    controls
        .try_import_loop(0, imported, 0, LoopMode::Playing, 1.0)
        .unwrap();
    let realtime = RealtimeMetrics::new(48_000, 4).unwrap();
    reset_violation_counters();
    {
        let _guard = realtime.enter_callback();
        process(&mut processor, 2);
    }
    assert_eq!(callback_allocations(), 0);
    assert_eq!(
        controls.try_status(),
        Some(RuntimeStatus::LoopImported {
            slot: 0,
            handle: imported
        })
    );
    controls.release_transfer(imported).unwrap();

    controls.try_request_loop_export(0).unwrap();
    reset_violation_counters();
    {
        let _guard = realtime.enter_callback();
        process(&mut processor, 0);
    }
    assert_eq!(callback_allocations(), 0);
}

#[test]
fn long_exports_are_copied_over_multiple_bounded_callbacks() {
    let frames = EXPORT_COPY_FRAMES_PER_CALLBACK * 2 + 17;
    let (mut processor, mut controls) =
        runtime_audio_processor_with_backend(SilentSynth, 48_000, frames, 4);
    let source_left: Vec<_> = (0..frames)
        .map(|frame| frame as f32 / frames as f32)
        .collect();
    let source_right: Vec<_> = source_left.iter().map(|sample| -*sample).collect();
    let imported = controls.try_acquire_transfer().unwrap();
    controls
        .write_transfer(imported, &source_left, &source_right)
        .unwrap();
    controls
        .try_import_loop(2, imported, 0, LoopMode::Playing, 1.0)
        .unwrap();
    process(&mut processor, 0);
    assert!(matches!(
        controls.try_status(),
        Some(RuntimeStatus::LoopImported { slot: 2, .. })
    ));
    controls.release_transfer(imported).unwrap();

    let exported = controls.try_request_loop_export(2).unwrap();
    process(&mut processor, 0);
    assert_eq!(controls.try_status(), None);
    assert_eq!(
        controls.with_exported_pcm(exported, |_, _| ()),
        Err(PcmTransferError::InvalidHandle)
    );
    process(&mut processor, 0);
    assert_eq!(controls.try_status(), None);
    process(&mut processor, 0);
    assert!(matches!(
        controls.try_status(),
        Some(RuntimeStatus::LoopExported { slot: 2, .. })
    ));
    controls
        .with_exported_pcm(exported, |left, right| {
            assert_eq!(left, source_left);
            assert_eq!(right, source_right);
        })
        .unwrap();
    controls.release_transfer(exported).unwrap();
}
