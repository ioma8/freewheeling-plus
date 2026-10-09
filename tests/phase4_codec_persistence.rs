//! Phase 4 regression coverage: persistence and codec-correctness fixes.

// 4.2 — a 32-byte *byte*-length name containing multibyte characters must
// reject instead of panicking on a non-char-boundary slice.
#[test]
fn decode_hash_rejects_multibyte_input_instead_of_panicking() {
    use freewheeling_plus::core_persistence::{decode_hash, encode_hash, HASH_LENGTH};

    // Exactly 32 bytes, with a multibyte character mid-pair: the old
    // implementation sliced `&s[0..2]`, landing inside the character.
    let hostile = format!("AB€{}", "0".repeat(27));
    assert_eq!(hostile.len(), 32);
    assert_eq!(decode_hash(&hostile), None);
    assert_eq!(decode_hash("€€"), None);
    // Digests still round-trip both cases.
    let text = encode_hash(&[
        0xA1, 0x2B, 0xC3, 0x4D, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    ]);
    let expected: [u8; HASH_LENGTH] = [
        0xA1, 0x2B, 0xC3, 0x4D, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    ];
    assert_eq!(decode_hash(&text), Some(expected));
    assert_eq!(decode_hash(&text.to_lowercase()), Some(expected));
    // Non-hex bytes are rejected too.
    assert_eq!(decode_hash("ØG"), None);
}

#[test]
fn saveable_stub_cannot_nest_a_component() {
    use freewheeling_plus::core_persistence::saveable_stub;
    // A '/' inside an imported object name must not build a nested path.
    let stub = saveable_stub(
        "loop",
        &"A".repeat(32),
        Some("a/b\0c"),
        Some(".wav"),
    );
    assert!(!stub.contains('/'), "stub was not sanitized: {stub}");
    assert!(!stub.contains('\0'), "stub was not sanitized: {stub}");
    assert!(stub.starts_with("loop-"));
    // A clean name is untouched.
    let clean = saveable_stub("loop", &"B".repeat(32), Some("take one"), None);
    assert!(clean.ends_with("-take one"));
}

#[test]
fn rename_saveable_rejects_a_separator_name() {
    use freewheeling_plus::core_persistence::{is_saveable_name, saveable_stub, split_filename};
    let stub = saveable_stub("loop", &"A".repeat(32), Some("before"), None);
    let (base, hash, _) = split_filename(&stub, 4).expect("well-formed stub must split");
    assert_eq!(base, "loop");
    assert_eq!(hash.len(), 32);
    // The runtime's rename pre-check: any name carrying a component separator
    // is refused before any file is moved, and plain names stay valid.
    assert!(!is_saveable_name("nested/name"));
    assert!(!is_saveable_name("trailing/"));
    assert!(!is_saveable_name("capture\0nul"));
    assert!(is_saveable_name("plain name"));
    assert!(is_saveable_name(""));
}

// 4.3 — every encoder sanitizes the same sample set.
#[test]
fn encoders_agree_on_pathological_samples() {
    use freewheeling_plus::block::{AudioBlock, AudioBlockIterator, Codec, ExtraChannel};
    use freewheeling_plus::file_codecs::{encode_audio_file, IFileDecoder, SndFileDecoder};
    use std::time::{SystemTime, UNIX_EPOCH};

    let directory = std::env::temp_dir().join(format!(
        "freewheeling-phase4-codecs-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    const RATE: u32 = 48_000;
    // Long enough for every codec's block-size floor (FLAC wants ≥16 frames).
    let head_left = [f32::NAN, f32::NEG_INFINITY, f32::INFINITY, 2.0, -2.0, 0.5];
    let head_right = [0.25, -0.25, 1.5, f32::NAN, 0.0, 0.25];
    let left: Vec<f32> = head_left
        .iter()
        .chain(std::iter::repeat_n(&0.5_f32, 42))
        .copied()
        .collect();
    let right: Vec<f32> = head_right
        .iter()
        .chain(std::iter::repeat_n(&0.25_f32, 42))
        .copied()
        .collect();

    for (codec, extension) in [
        (Codec::Wav, "wav"),
        (Codec::Vorbis, "ogg"),
        (Codec::Flac, "flac"),
        (Codec::Au, "au"),
    ] {
        let path = directory.join(format!("sanitized.{extension}"));
        encode_audio_file(&path, RATE, codec, &left, Some(&right)).expect("encode");
        let mut decoder = SndFileDecoder::new(RATE, codec);
        decoder
            .read_from_file(std::fs::File::open(&path).unwrap())
            .unwrap();
        assert!(decoder.stereo(), "{extension}: expected a stereo file");
        let mut block = AudioBlock::new(64);
        block.extra = Some(ExtraChannel::new(64));
        let decoded = {
            let mut iterator = AudioBlockIterator::new(&mut block, 64);
            loop {
                let count = decoder.read_samples(&mut iterator, 64).unwrap();
                if count == 0 {
                    break iterator.position;
                }
            }
        };
        assert_eq!(decoded, left.len(), "{extension}: frame count changed");
        // `vorbis` is lossy: a sanitized zero can come back near zero but not
        // exactly zero, so the lossy codec asserts finiteness and range only,
        // while the bit-exact codecs assert the exact sanitize contract.
        let lossy = codec == Codec::Vorbis;
        for (stored, original) in block.samples[..decoded].iter().zip(left.iter()) {
            assert!(
                stored.is_finite() && *stored >= -1.0 && *stored <= 1.0 - f32::EPSILON,
                "{extension}: sample outside the window: {stored} <- {original}"
            );
            if !lossy {
                let expected = if original.is_finite() { *original } else { 0.0 };
                let expected = expected.clamp(-1.0, 1.0 - f32::EPSILON);
                assert!(
                    (*stored - expected).abs() < 1e-6,
                    "{extension}: unexpected round-trip sample: {stored} != {expected} (from {original})"
                );
            }
        }
        let stored_right = &block.extra.as_ref().unwrap().samples;
        for (stored, original) in stored_right[..decoded].iter().zip(right.iter()) {
            assert!(
                stored.is_finite() && *stored >= -1.0 && *stored <= 1.0 - f32::EPSILON,
                "{extension}: sample outside the window: {stored} <- {original}"
            );
            if !lossy {
                let expected = if original.is_finite() { *original } else { 0.0 };
                let expected = expected.clamp(-1.0, 1.0 - f32::EPSILON);
                assert!(
                    (*stored - expected).abs() < 1e-6,
                    "{extension}: unexpected round-trip sample: {stored} != {expected} (from {original})"
                );
            }
        }
        std::fs::remove_file(&path).unwrap();
    }
    std::fs::remove_dir_all(&directory).unwrap();
}

// 4.4 — re-saving identical quantised audio is a reuse; a file that decodes
// to different audio under the same hash bucket is never aliased silently.
#[test]
fn loop_export_reuse_is_verified_not_assumed() {
    use freewheeling_plus::block::{AudioBlockIterator, AudioBlock, Codec, ExtraChannel};
    use freewheeling_plus::core_persistence::{encode_hash, md5_loop_samples};
    use freewheeling_plus::file_codecs::{IFileDecoder, SndFileDecoder};
    use freewheeling_plus::native_dsp_graph::LoopMode;
    use freewheeling_plus::native_dsp_graph::LoopTransferMetadata;
    use freewheeling_plus::production_app::native_runtime::save_exported_loop;
    use std::time::{SystemTime, UNIX_EPOCH};

    let directory = std::env::temp_dir().join(format!(
        "freewheeling-phase4-export-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    const RATE: u32 = 48_000;
    let metadata = LoopTransferMetadata {
        frames: 8,
        position: 0,
        mode: LoopMode::Playing,
        gain: 1.0,
        pulse_frames: RATE / 2,
        beats: 4,
    };
    let first = [0.5_f32; 8];
    let right = [0.125_f32; 8];
    let first_hash = encode_hash(&md5_loop_samples(&first, Some(&right)));
    let audio = directory.join(format!("loop-{first_hash}-take.wav"));
    let xml = directory.join(format!("loop-{first_hash}-take.xml"));

    save_exported_loop(&audio, &xml, RATE, Codec::Wav, &first, &right, metadata)
        .expect("the first save creates the file");
    assert!(audio.exists() && xml.exists());

    // The identical take must reuse the file, completing the metadata if it
    // went missing.
    std::fs::remove_file(&xml).unwrap();
    save_exported_loop(&audio, &xml, RATE, Codec::Wav, &first, &right, metadata)
        .expect("identical audio is a dedup reuse, not a collision");
    assert!(xml.exists(), "reuse completed the missing metadata");

    // A foreign file at the same name must not be aliased: it cannot be
    // decoded as this loop's audio, so the save fails visibly.
    let foreign = directory.join(format!("loop-{first_hash}-foreign.wav"));
    std::fs::write(&foreign, vec![0u8; 64]).unwrap();
    let outcome = save_exported_loop(&foreign, &xml, RATE, Codec::Wav, &first, &right, metadata);
    assert!(outcome.is_err(), "a foreign file must not be re-pointed silently");

    // Reassure the decode path stays exercised: a whole-file decode of the
    // reused file returns exactly the written, sanitized samples.
    let mut decoder = SndFileDecoder::new(RATE, Codec::Wav);
    decoder
        .read_from_file(std::fs::File::open(&audio).unwrap())
        .unwrap();
    let mut block = AudioBlock::new(first.len() + 64);
    block.extra = Some(ExtraChannel::new(first.len() + 64));
    let decoded = {
        let mut iterator = AudioBlockIterator::new(&mut block, 64);
        loop {
            let count = decoder.read_samples(&mut iterator, 64).unwrap();
            if count == 0 {
                break iterator.position;
            }
        }
    };
    assert_eq!(decoded, first.len());
    assert!(block.samples[..decoded].iter().all(|sample| *sample == 0.5));
    std::fs::remove_dir_all(&directory).unwrap();
}

// 4.6 — scene backups are pruned to the newest few.
#[test]
fn scene_backups_do_not_accumulate_without_bound() {
    use freewheeling_plus::production_app::native_runtime::{prune_scene_backups, MAX_SCENE_BACKUPS};
    use std::time::{SystemTime, UNIX_EPOCH};

    let directory = std::env::temp_dir().join(format!(
        "freewheeling-phase4-backups-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let scene = directory.join("scene-ABCD.xml");
    std::fs::write(&scene, b"current").unwrap();
    for sequence in 0..12 {
        std::fs::write(
            directory.join(format!("scene-ABCD.xml.backup.{sequence}")),
            b"old",
        )
        .unwrap();
    }
    prune_scene_backups(&scene, MAX_SCENE_BACKUPS);
    let backup_count = std::fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".backup.")
        })
        .count();
    assert_eq!(backup_count, 8, "backups above the cap were not pruned");
    std::fs::remove_dir_all(&directory).unwrap();
}
