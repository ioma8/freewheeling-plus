use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

#[test]
fn macos_diagnostic_runner_is_manual_and_non_attesting() {
    let script = fs::read_to_string(root().join("scripts/run_macos_diagnostics.sh")).unwrap();
    let docs = fs::read_to_string(root().join("scripts/README.md")).unwrap();
    for required in [
        "FWEELIN_DATADIR",
        "system_profiler SPAudioDataType",
        "log stream",
        "SDL",
        "rust-stderr-rejections.txt",
        "crash-reports",
        "input test",
        "acceptance-evidence",
        "not acceptance evidence",
    ] {
        assert!(
            script.contains(required) || docs.contains(required),
            "missing diagnostic guardrail: {required}"
        );
    }
    assert!(script.contains("uname -s") && script.contains("Darwin"));
    assert!(!script.contains("attestation.json"));
    assert!(!script.contains("status=passed"));
}

fn run(script: &str, arguments: &[&Path]) -> Output {
    let mut command = Command::new("python3");
    command.arg(root().join("scripts").join(script));
    command.args(arguments);
    command.output().expect("python3 must run validator")
}

fn temporary(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("freewheeling-{name}-{}", std::process::id()))
}

fn rgba(path: &Path, width: u32, height: u32, pixels: &[[u8; 4]]) {
    let mut bytes = b"FWRGBA1\n".to_vec();
    bytes.extend(width.to_le_bytes());
    bytes.extend(height.to_le_bytes());
    bytes.extend(pixels.iter().flatten());
    fs::write(path, bytes).unwrap();
}

#[test]
fn screenshot_gate_accepts_exactly_99_5_percent_with_delta_two() {
    let directory = temporary("screenshots-pass");
    fs::create_dir_all(&directory).unwrap();
    let reference = directory.join("reference.rgba");
    let candidate = directory.join("candidate.rgba");
    rgba(&reference, 200, 1, &vec![[20; 4]; 200]);
    let mut changed = vec![[22; 4]; 200];
    changed[0] = [23; 4];
    rgba(&candidate, 200, 1, &changed);
    let output = run("compare_screenshots.py", &[&reference, &candidate]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("99.500000%"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn screenshot_gate_rejects_below_threshold_and_missing_goldens() {
    let directory = temporary("screenshots-fail");
    fs::create_dir_all(&directory).unwrap();
    let reference = directory.join("reference.rgba");
    let candidate = directory.join("candidate.rgba");
    rgba(&reference, 100, 1, &vec![[0; 4]; 100]);
    rgba(&candidate, 100, 1, &vec![[3; 4]; 100]);
    assert!(
        !run("compare_screenshots.py", &[&reference, &candidate])
            .status
            .success()
    );
    let missing = directory.join("cpp-golden-missing.rgba");
    let output = run("compare_screenshots.py", &[&missing, &candidate]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("required screenshot fixture is missing")
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn performance_result_validator_enforces_realtime_acceptance() {
    let directory = temporary("performance");
    fs::create_dir_all(&directory).unwrap();
    let valid = directory.join("valid.json");
    fs::write(
        &valid,
        r#"{
      "schema_version": 1, "sample_rate_hz": 48000, "buffer_frames": 128,
      "duration_seconds": 7200, "callback_p99_us": 1800.0,
      "callback_deadline_us": 2666.6667, "callback_allocations": 0,
      "blocking_lock_attempts": 0, "unexplained_xruns": 0,
      "rss_start_bytes": 1000000, "rss_peak_bytes": 1200000
    }"#,
    )
    .unwrap();
    let output = Command::new("python3")
        .arg(root().join("scripts/validate_performance_result.py"))
        .arg(&valid)
        .arg("--require-stress")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let invalid = directory.join("invalid.json");
    fs::write(
        &invalid,
        fs::read_to_string(&valid)
            .unwrap()
            .replace("1800.0", "1900.0"),
    )
    .unwrap();
    let output = run("validate_performance_result.py", &[&invalid]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("below 70%"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn bundle_verifier_requires_executable_resources_license_and_microphone_text() {
    let directory = temporary("bundle");
    let bundle = directory.join("FreeWheeling.app");
    let contents = bundle.join("Contents");
    let resources = contents.join("Resources");
    fs::create_dir_all(contents.join("MacOS")).unwrap();
    fs::create_dir_all(resources.join("data")).unwrap();
    fs::create_dir_all(resources.join("licenses")).unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_freewheeling-plus"),
        contents.join("MacOS/freewheeling-plus"),
    )
    .unwrap();
    for file in ["Vera.ttf", "VeraBd.ttf", "basic.sf2"] {
        fs::write(resources.join("data").join(file), b"fixture").unwrap();
    }
    fs::write(resources.join("data/fweelin.xml"), b"<freewheeling/>").unwrap();
    fs::write(resources.join("licenses/COPYING"), b"fixture license").unwrap();
    fs::write(
        resources.join("licenses/Bitstream-Vera-NOTICE.txt"),
        b"Bitstream Vera\nPermission is hereby granted for the Font Software",
    )
    .unwrap();
    fs::write(
        resources.join("licenses/basic.sf2-LICENSE.txt"),
        b"Reviewed fixture distribution license evidence for basic.sf2.",
    )
    .unwrap();
    fs::write(
        contents.join("Info.plist"),
        br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>freewheeling-plus</string>
<key>NSMicrophoneUsageDescription</key><string>Record live sound.</string>
<key>CFBundleDocumentTypes</key><array><dict><key>CFBundleTypeName</key><string>Audio</string></dict></array>
</dict></plist>"#,
    )
    .unwrap();
    let output = run("verify_macos_bundle.py", &[&bundle]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_file(resources.join("data/basic.sf2")).unwrap();
    let output = run("verify_macos_bundle.py", &[&bundle]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("basic.sf2"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn baseline_inventory_and_matrix_name_required_assets_and_workflows() {
    let matrix = fs::read_to_string(root().join("FEATURE_MATRIX.md"))
        .unwrap()
        .to_lowercase();
    for workflow in [
        "record",
        "overdub",
        "trigger",
        "mute",
        "erase",
        "snapshots",
        "scenes",
        "midi mapping",
        "midi clock",
        "osc",
        "fullscreen",
        "browser rename",
        "device loss",
    ] {
        assert!(matrix.contains(workflow), "feature matrix omits {workflow}");
    }
    let inventory = fs::read_to_string(root().join("PACKAGING.md")).unwrap();
    for resource in [
        "Vera.ttf",
        "VeraBd.ttf",
        "basic.sf2",
        "COPYING",
        "AUTHORS",
        "NSMicrophoneUsageDescription",
    ] {
        assert!(
            inventory.contains(resource),
            "packaging inventory omits {resource}"
        );
    }
}

#[test]
fn soundfont_handoff_keeps_unresolved_provenance_and_requires_clean_room_evidence() {
    let handoff = fs::read_to_string(root().join("../docs/basic-sf2-clean-room-handoff.md"))
        .expect("clean-room SoundFont handoff must be documented");
    for requirement in [
        "no lawful drop-in is currently available",
        "Do not\ninspect",
        "compare against",
        "SPDX-compatible license",
        "FluidSynth",
        "FluidLite",
        "valid SF2 containing exactly bank",
        "rejects the legacy digest",
    ] {
        assert!(handoff.contains(requirement), "handoff omits {requirement}");
    }
    assert!(handoff.contains("2e6cf4a8a1d78e6be3b00a0c22358d3ceec8c5a27a000714e65215e3f9b1d15a"));
}
