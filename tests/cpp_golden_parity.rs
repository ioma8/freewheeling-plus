use freewheeling_plus::videoio::RenderMetrics;
use freewheeling_plus::{
    block::AudioBlock,
    core_dsp::{AudioLevel, Pulse},
    core_persistence::{decode_hash, encode_hash, saveable_path, saveable_stub},
    signal::format_signal_message,
    stacktrace::format_symbol_entry,
    string_utils::alloc_saveable_stub,
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
        // The dB range spans -1000..0 and one f32 ULP grows with the
        // magnitude (~6e-5 at 1000), so the bound is relative to the expected
        // value with an absolute floor for the values near zero.
        let tolerance = (f32::abs(expected) * 1e-7).max(2e-5);
        assert_close(AudioLevel::fader_to_db(level, 0.0), expected, tolerance);
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
    // A buffer that cannot hold the whole message stops at the boundary and
    // still terminates the text it did write.
    let mut small = [0xaa_u8; 16];
    let written = format_signal_message(libc::SIGSEGV, &mut small);
    assert!(written < small.len(), "the message must be truncated");
    assert_eq!(small[written], 0, "the truncated message stays terminated");
    // 15 bytes fit (`len - 1`, keeping room for the terminator).
    assert_eq!(written, b">>> FATAL ERROR: ".len() - 2);
    // No buffer at all: nothing is written and the caller can tell.
    let mut empty: [u8; 0] = [];
    assert_eq!(format_signal_message(libc::SIGSEGV, &mut empty), 0);

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
    let mut expected = b"FWB2".to_vec();
    expected.extend_from_slice(&2u32.to_le_bytes());
    expected.extend_from_slice(&2u32.to_le_bytes());
    expected.push(0);
    for value in [1.0f32, -2.5] {
        expected.extend_from_slice(&value.to_le_bytes());
    }
    expected.extend_from_slice(&1u32.to_le_bytes());
    expected.push(0);
    expected.extend_from_slice(&3.25f32.to_le_bytes());
    assert_eq!(bytes, expected);
    let decoded = AudioBlock::deserialize(&mut bytes.as_slice()).unwrap();
    assert_eq!(decoded.samples, vec![1.0, -2.5]);
    assert_eq!(decoded.next.as_deref().unwrap().samples, vec![3.25]);
}

#[test]
fn cpp_block_legacy_wire_format_still_decodes() {
    let mut bytes = b"FWB1".to_vec();
    bytes.extend_from_slice(&3u64.to_le_bytes());
    for value in [1.0f32, -2.5, 3.25] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let decoded = AudioBlock::deserialize(&mut bytes.as_slice()).unwrap();
    assert_eq!(decoded.samples, vec![1.0, -2.5, 3.25]);
    assert!(decoded.next.is_none());
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
        // A negative length cannot be expressed: `Pulse::quantize_length`
        // takes an unsigned frame count, so the C++ signed round-half-away
        // case has no reachable input here (pinned by the type, not by a
        // branch).
    ] {
        assert_eq!(p.quantize_length(src), expected, "src={src}");
    }
    assert_eq!(Pulse::new(0, 0).quantize_length(1234), 1234);
}

#[test]
fn cpp_video_scaling_golden_vectors() {
    // Exercised against the production `RenderMetrics`, not against locally
    // recomputed arithmetic: the golden vectors only have value if a regression
    // in the real scaling code makes them fail.
    let metrics = RenderMetrics::new(640, 480, 1280, 960);
    assert_close(metrics.scale_x, 2.0, f32::EPSILON);
    assert_close(metrics.scale_y, 2.0, f32::EPSILON);
    // Positive values round half up, like C++'s `(value * scale + 0.5)` cast.
    assert_eq!(metrics.x(7), 14);
    assert_eq!(metrics.extent(7, 1.5), 11);
    assert_eq!(metrics.extent(1, 0.4), 1);
    // Non-positive values and scales are guarded instead of scaled.
    assert_eq!(metrics.extent(0, 2.0), 0);
    assert_eq!(metrics.extent(-3, 2.0), 0);
    assert_eq!(metrics.extent(5, 0.0), 5);
    // A non-positive drawable size means "unset": the logical size is used.
    let unset = RenderMetrics::new(640, 480, 0, 0);
    assert_close(unset.scale_x, 1.0, f32::EPSILON);
    assert_eq!(unset.x(7), 7);
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
    // The encoder is case-insensitive on decode and must round-trip every
    // 16-byte value, including the all-zero and all-ones edges.
    for hash in [
        [0x00_u8; 16],
        [0xff_u8; 16],
        [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10],
    ] {
        let text = encode_hash(&hash);
        assert_eq!(text.len(), 32, "the encoded hash is 16 bytes as hex");
        assert_eq!(decode_hash(&text), Some(hash), "round-trip {text}");
    }
    assert_eq!(decode_hash("0011"), None);
    assert_eq!(
        alloc_saveable_stub("loop", "0011AABB", "lead", ".ogg"),
        "loop-0011AABB-lead.ogg"
    );
    assert_eq!(
        encode_hash(&[0, 1, 0xAB, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        "0001ABFF000000000000000000000000"
    );
}
