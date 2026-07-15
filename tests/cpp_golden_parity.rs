use freewheeling_plus::{
    block::AudioBlock,
    core_dsp::{AudioLevel, Pulse},
    core_persistence::{encode_hash, saveable_path, saveable_stub},
    signal::format_signal_message,
    stacktrace::format_symbol_entry,
    string_utils::alloc_saveable_stub,
    video_scaling::{compute_video_scale, scale_extent},
};

fn assert_close(actual: f32, expected: f32, eps: f32) {
    assert!(
        (actual - expected).abs() <= eps,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn cpp_iec_fader_golden_vectors() {
    // IEC 60-268-18 piecewise formulas, with maxDb=0 (100% travel).
    for (level, expected) in [
        (0.0, -1000.0),
        (0.025, -60.0),
        (0.075, -50.0),
        (0.15, -40.0),
        (0.30, -30.0),
        (0.50, -20.0),
        (1.0, 0.0),
    ] {
        assert_close(AudioLevel::fader_to_db(level, 0.0), expected, 2e-5);
    }
    for (db, expected) in [
        (-1000.0, 0.0),
        (-70.0, 0.0),
        (-60.0, 0.025),
        (-50.0, 0.075),
        (-40.0, 0.15),
        (-30.0, 0.30),
        (-20.0, 0.50),
        (0.0, 1.0),
        (6.0, 1.0),
    ] {
        assert_close(AudioLevel::db_to_fader(db, 0.0), expected, 2e-6);
    }
}

#[test]
fn cpp_signal_and_symbol_format_golden_vectors() {
    let mut buf = [0u8; 160];
    let n = format_signal_message(libc::SIGSEGV, &mut buf);
    assert_eq!(
        &buf[..n],
        b">>> FATAL ERROR: Segmentation fault (SIGSEGV) occurred! <<<\n"
    );
    let n = format_signal_message(999, &mut buf);
    assert_eq!(
        &buf[..n],
        b">>> FATAL ERROR: Fatal signal received (SIGNAL) occurred! <<<\n"
    );
    assert_eq!(
        format_symbol_entry(3, 0x12ab, Some("foo"), 0x2, 'T'),
        "[3] 0x000012ab <foo + 0x2> T\n"
    );
    assert_eq!(
        format_symbol_entry(4, 0xfeed, None, 0, '?'),
        "[4] 0x0000feed ???\n"
    );
}

#[test]
fn cpp_block_wire_format_golden_vector() {
    let mut b = AudioBlock::new(2);
    b.samples = vec![1.0, -2.5];
    b.link(AudioBlock {
        samples: vec![3.25],
        extra: None,
        next: None,
    });
    let mut bytes = Vec::new();
    b.serialize(&mut bytes).unwrap();
    let mut expected = b"FWB1".to_vec();
    expected.extend_from_slice(&3u64.to_le_bytes());
    for value in [1.0f32, -2.5, 3.25] {
        expected.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(bytes, expected);
    let decoded = AudioBlock::deserialize(&mut bytes.as_slice()).unwrap();
    assert_eq!(decoded.samples, vec![1.0, -2.5, 3.25]);
}

#[test]
fn cpp_pulse_quantization_golden_vectors() {
    let p = Pulse::new(480, 0);
    for (src, expected) in [
        (0, 0),
        (239, 0),
        (240, 480),
        (241, 480),
        (719, 480),
        (720, 960),
        (961, 960),
    ] {
        assert_eq!(p.quantize_length(src), expected, "src={src}");
    }
    assert_eq!(Pulse::new(0, 0).quantize_length(1234), 1234);
}

#[test]
fn cpp_video_scaling_golden_vectors() {
    let s = compute_video_scale(640, 480, 1280, 960);
    assert_eq!(
        (
            s.logical_width,
            s.logical_height,
            s.drawable_width,
            s.drawable_height
        ),
        (640, 480, 1280, 960)
    );
    assert_close(s.scale_x, 2.0, f32::EPSILON);
    assert_eq!(scale_extent(7, 1.5), 11); // positive-half-up: floor(10.5 + .5)
    assert_eq!(scale_extent(1, 0.4), 1);
    assert_eq!(scale_extent(-1, 2.0), 0);
}

#[test]
fn cpp_persistence_naming_golden_vectors() {
    assert_eq!(
        saveable_stub("loop", "0011AABB", Some("lead"), Some(".ogg")),
        "loop-0011AABB-lead.ogg"
    );
    assert_eq!(
        saveable_stub("loop", "0011AABB", Some(""), None),
        "loop-0011AABB"
    );
    assert_eq!(
        saveable_path("library", "loop", "0011AABB", None, Some(".dat")),
        "library/loop-0011AABB.dat"
    );
    assert_eq!(
        alloc_saveable_stub(Some("loop"), Some("0011AABB"), Some("lead"), Some(".ogg")),
        "loop-0011AABB-lead.ogg"
    );
    assert_eq!(
        encode_hash(&[0, 1, 0xAB, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        "0001ABFF000000000000000000000000"
    );
}
