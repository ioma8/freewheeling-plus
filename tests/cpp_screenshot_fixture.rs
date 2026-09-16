use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cpp-golden/screenshots")
}

fn kv(path: &Path) -> BTreeMap<String, String> {
    let text =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    text.lines()
        .map(|line| {
            line.split_once('=')
                .unwrap_or_else(|| panic!("{}: malformed line {line:?}", path.display()))
        })
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
}

/// Read a fixture, naming the file when it is missing.
fn read_fixture(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn historical_cpp_screenshots_have_exact_dimensions_and_provenance() {
    let expected = [
        ("window-640x480", 640, 480, 640, 480),
        ("configured-800x600", 800, 600, 800, 600),
        ("fullscreen-logical-1024x768", 1024, 768, 1024, 768),
        ("hidpi-640x480-1x", 640, 480, 640, 480),
        ("hidpi-640x480-2x", 640, 480, 1280, 960),
    ];
    for (name, lw, lh, dw, dh) in expected {
        let png_path = root().join(format!("{name}.png"));
        let png = read_fixture(&png_path);
        // Length first: indexing a truncated fixture would panic with an
        // opaque range error instead of naming the file.
        assert!(
            png.len() >= 24,
            "{}: truncated PNG ({} bytes)",
            png_path.display(),
            png.len()
        );
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "{name} is not PNG");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), dw);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), dh);
        let meta = kv(&root().join(format!("{name}.meta")));
        for (key, wanted) in [
            ("logical_width", lw),
            ("logical_height", lh),
            ("drawable_width", dw),
            ("drawable_height", dh),
        ] {
            assert_eq!(
                meta.get(key)
                    .unwrap_or_else(|| panic!("{name}.meta missing {key}")),
                &wanted.to_string()
            );
        }
    }
    let provenance = kv(&root().join("PROVENANCE"));
    assert_eq!(
        provenance.get("schema").map(String::as_str),
        Some("freewheeling-cpp-screenshots-v1")
    );
    assert_eq!(
        provenance.get("cpp_revision").map(String::len),
        Some(40),
        "cpp_revision must be a full git revision"
    );
    for key in [
        "cpp_binary_sha256",
        "videoio_source_sha256",
        "display_source_sha256",
        "graphics_config_sha256",
        "capture_script_sha256",
    ] {
        let value = provenance
            .get(key)
            .unwrap_or_else(|| panic!("PROVENANCE missing {key}"));
        // Validate the shape, not just the length.
        assert!(
            value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()),
            "invalid {key}: {value:?}"
        );
    }
}

#[test]
fn screenshot_manifest_verifies() {
    let manifest_path = root().join("MANIFEST.sha256");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("{}: {error}", manifest_path.display()));
    let mut entries = 0;
    for line in manifest.lines().filter(|line| !line.is_empty()) {
        entries += 1;
        let (expected, name) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("manifest row format: {line:?}"));
        // A manifest entry must be a plain file name: `Path::join` accepts
        // absolute paths and `..`, which would read outside the fixture tree.
        assert!(
            Path::new(name).components().count() == 1,
            "manifest row contains an unsafe path: {name}"
        );
        let data = read_fixture(&root().join(name));
        let digest = format!("{:x}", Sha256::digest(&data));
        assert_eq!(digest, expected, "checksum mismatch for {name}");
    }
    assert!(entries > 0, "screenshot manifest is empty");
}
