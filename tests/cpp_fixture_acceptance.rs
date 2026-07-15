use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cpp-golden")
}

fn parse_kv(path: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

const FIXTURE_CLASSES: [&str; 7] = [
    "codec",
    "dsp",
    "midi",
    "persistence",
    "renderer",
    "screenshots",
    "startup",
];

fn category_files(root: &Path) -> BTreeSet<String> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeSet<String>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }

    let mut files = BTreeSet::new();
    for class in FIXTURE_CLASSES {
        visit(root, &root.join(class), &mut files);
    }
    files
}

#[test]
fn deterministic_cpp_captures_have_provenance_and_payloads() {
    let root = root();
    let provenance = parse_kv(&root.join("PROVENANCE"));
    assert_eq!(
        provenance.get("schema").map(String::as_str),
        Some("freewheeling-cpp-golden-v1")
    );
    assert_eq!(provenance.get("cpp_revision").map(String::len), Some(40));
    for key in [
        "codec_provenance",
        "persistence_provenance",
        "screenshots_provenance",
        "midi_startup_provenance",
        "full_startup_provenance",
    ] {
        let relative = provenance
            .get(key)
            .unwrap_or_else(|| panic!("root provenance omits {key}"));
        assert!(
            root.join(relative).is_file(),
            "missing provenance: {relative}"
        );
    }
    for relative in [
        "dsp/fader.tsv",
        "persistence/fweelin.xml",
        "renderer/fixed-sizes.tsv",
        "startup/rollback.log",
        "startup/full-rollback.log",
        "startup/full-application.log",
        "midi/messages.log",
    ] {
        let bytes = fs::read(root.join(relative))
            .unwrap_or_else(|e| panic!("required genuine C++ fixture {relative}: {e}"));
        assert!(!bytes.is_empty(), "empty C++ fixture: {relative}");
    }
    let startup = fs::read_to_string(root.join("startup/rollback.log")).unwrap();
    assert!(
        startup.ends_with("rollback 2\nrollback 1\n"),
        "historical guard must roll back in reverse order"
    );
    let midi = fs::read_to_string(root.join("midi/messages.log")).unwrap();
    for family in [
        "note-on",
        "note-off",
        "control-change",
        "program-change",
        "channel-pressure",
        "pitch-bend",
    ] {
        assert!(midi.contains(family), "MIDI trace omits {family}");
    }
    for realtime in ["bytes=F8", "bytes=FA", "bytes=FC"] {
        assert!(midi.contains(realtime), "MIDI trace omits {realtime}");
    }
    let full = fs::read_to_string(root.join("startup/full-rollback.log")).unwrap();
    assert_eq!(full.matches("scenario fail-after=").count(), 13);
    assert_eq!(full.matches("complete live=0").count(), 12);
    assert!(!full.contains("rollback-order-error"));
    let application = fs::read_to_string(root.join("startup/full-application.log")).unwrap();
    for marker in [
        "SDLIO: SDL Input thread start.",
        "MIDI: end",
        "AUDIO: end",
        "EVENT: manager end.",
        "MEM: End cleanup.",
        "MAIN: end",
    ] {
        assert!(
            application.contains(marker),
            "full startup log omits {marker}"
        );
    }
    assert!(application.contains("<addr>"));
    assert!(!application.contains("/var/folders/"));
}

#[test]
fn manifest_names_every_available_artifact() {
    let root = root();
    let manifest = fs::read_to_string(root.join("MANIFEST.sha256")).unwrap();
    let named: BTreeSet<_> = manifest
        .lines()
        .map(|line| {
            line.split_whitespace()
                .nth(1)
                .expect("malformed manifest row")
        })
        .collect();
    let actual = category_files(&root);
    assert_eq!(
        named,
        actual.iter().map(String::as_str).collect(),
        "root manifest must name every category file exactly once"
    );
    assert!(
        !root.join("UNAVAILABLE").exists(),
        "completed capture must remove UNAVAILABLE"
    );

    let verified = Command::new("shasum")
        .args(["-a", "256", "-c", "MANIFEST.sha256"])
        .current_dir(&root)
        .output()
        .expect("shasum is required to verify C++ fixture provenance");
    assert!(
        verified.status.success(),
        "fixture hash verification failed:\n{}",
        String::from_utf8_lossy(&verified.stderr)
    );
}

#[test]
fn unavailable_artifacts_are_not_silently_substituted() {
    if std::env::var_os("FW_REQUIRE_COMPLETE_CPP_GOLDEN").is_none() {
        return;
    }
    assert!(!root().join("UNAVAILABLE").exists());
}
