use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cpp-golden/screenshots")
}

fn kv(path: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.into(), value.into()))
        .collect()
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
        let png = fs::read(root().join(format!("{name}.png"))).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "{name} is not PNG");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), dw);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), dh);
        let meta = kv(&root().join(format!("{name}.meta")));
        assert_eq!(meta["logical_width"], lw.to_string());
        assert_eq!(meta["logical_height"], lh.to_string());
        assert_eq!(meta["drawable_width"], dw.to_string());
        assert_eq!(meta["drawable_height"], dh.to_string());
    }
    let provenance = kv(&root().join("PROVENANCE"));
    assert_eq!(provenance["schema"], "freewheeling-cpp-screenshots-v1");
    assert_eq!(provenance["cpp_revision"].len(), 40);
    for key in [
        "cpp_binary_sha256",
        "videoio_source_sha256",
        "display_source_sha256",
        "graphics_config_sha256",
        "capture_script_sha256",
    ] {
        assert_eq!(provenance[key].len(), 64, "invalid {key}");
    }
}

#[test]
fn screenshot_manifest_verifies() {
    let output = Command::new("shasum")
        .args(["-a", "256", "-c", "MANIFEST.sha256"])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
