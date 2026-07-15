use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cpp-golden")
}

fn kv(path: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
}

fn verify_manifest(directory: &Path) {
    let output = Command::new("shasum")
        .args(["-a", "256", "-c", "MANIFEST.sha256"])
        .current_dir(directory)
        .output()
        .expect("shasum is required to verify historical fixtures");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn genuine_historical_codec_files_have_expected_containers() {
    let codec = root().join("codec");
    assert_eq!(
        kv(&codec.join("PROVENANCE"))
            .get("schema")
            .map(String::as_str),
        Some("freewheeling-cpp-codec-golden-v1")
    );
    assert!(
        fs::read(codec.join("reference.wav"))
            .unwrap()
            .starts_with(b"RIFF")
    );
    assert!(
        fs::read(codec.join("reference.flac"))
            .unwrap()
            .starts_with(b"fLaC")
    );
    assert!(
        fs::read(codec.join("reference.ogg"))
            .unwrap()
            .starts_with(b"OggS")
    );
    assert!(!codec.join("reference.au").exists());
    let unsupported = fs::read_to_string(codec.join("UNSUPPORTED")).unwrap();
    assert!(unsupported.contains("SF_FORMAT_WAV | SF_FORMAT_FLOAT"));
    verify_manifest(&codec);
}

#[test]
fn historical_scene_and_loop_metadata_are_present_and_hashed() {
    let persistence = root().join("persistence");
    assert_eq!(
        kv(&persistence.join("PROVENANCE"))
            .get("schema")
            .map(String::as_str),
        Some("freewheeling-cpp-persistence-golden-v1")
    );
    let scene = fs::read_to_string(persistence.join("scene.xml")).unwrap();
    assert!(scene.contains("hash=\"00112233445566778899aabbccddeeff\""));
    assert!(scene.contains("name=\"Golden &amp; snapshot\""));
    assert!(scene.contains("triggervol=\"0.75000\""));
    let loop_xml = fs::read_to_string(persistence.join("loop.xml")).unwrap();
    assert!(loop_xml.contains("version=\"1\""));
    assert!(loop_xml.contains("nbeats=\"4\""));
    assert!(loop_xml.contains("pulselen=\"24000\""));
    verify_manifest(&persistence);
}
