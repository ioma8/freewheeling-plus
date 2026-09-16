use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cpp-golden")
}

fn kv(path: &Path) -> BTreeMap<String, String> {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let entries: Vec<(String, String)> = text
        .lines()
        .map(|line| {
            line.split_once('=')
                .unwrap_or_else(|| panic!("{}: malformed line {line:?}", path.display()))
        })
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
    // A duplicated key would silently keep the last value, which is exactly the
    // kind of fixture drift (a name swapped, case changed) these asserts exist
    // to catch.
    let mut keys = std::collections::BTreeSet::new();
    for (key, _) in &entries {
        assert!(
            keys.insert(key.clone()),
            "{}: duplicate key {key:?}",
            path.display()
        );
    }
    entries.into_iter().collect()
}

fn verify_manifest(directory: &Path) {
    let manifest_path = directory.join("MANIFEST.sha256");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("{}: {error}", manifest_path.display()));
    let entries: Vec<&str> = manifest.lines().filter(|line| !line.is_empty()).collect();
    // An empty or truncated manifest would otherwise verify nothing.
    assert!(
        !entries.is_empty(),
        "{}: manifest contains no entries",
        manifest_path.display()
    );
    let mut seen = std::collections::BTreeSet::new();
    for line in entries {
        let (expected, name) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("manifest line must be '<hex>  <file>': {line:?}"));
        assert!(
            seen.insert(name.to_owned()),
            "{}: duplicate manifest entry for {name}",
            manifest_path.display()
        );
        let path = directory.join(name);
        let data = fs::read(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let digest = format!("{:x}", Sha256::digest(&data));
        assert_eq!(digest, expected, "checksum mismatch for {name}");
    }
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
    for (name, magic) in [
        ("reference.wav", b"RIFF".as_slice()),
        ("reference.flac", b"fLaC".as_slice()),
        ("reference.ogg", b"OggS".as_slice()),
    ] {
        let path = codec.join(name);
        let data = fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert!(
            data.starts_with(magic),
            "{name} does not start with the expected container magic"
        );
    }
    assert!(
        !codec.join("reference.au").exists(),
        "reference.au must not exist: AU input is intentionally unsupported"
    );
    let unsupported_path = codec.join("UNSUPPORTED");
    let unsupported = fs::read_to_string(&unsupported_path)
        .unwrap_or_else(|error| panic!("{}: {error}", unsupported_path.display()));
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
    let scene = fs::read_to_string(persistence.join("scene.xml"))
        .unwrap_or_else(|error| panic!("{}: {error}", persistence.join("scene.xml").display()));
    assert!(scene.contains("hash=\"00112233445566778899aabbccddeeff\""));
    assert!(scene.contains("name=\"Golden &amp; snapshot\""));
    assert!(scene.contains("triggervol=\"0.75000\""));
    let loop_xml = fs::read_to_string(persistence.join("loop.xml"))
        .unwrap_or_else(|error| panic!("{}: {error}", persistence.join("loop.xml").display()));
    assert!(loop_xml.contains("version=\"1\""));
    assert!(loop_xml.contains("nbeats=\"4\""));
    assert!(loop_xml.contains("pulselen=\"24000\""));
    verify_manifest(&persistence);
}
