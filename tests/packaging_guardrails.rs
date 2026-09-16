use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "support/scratch.rs"]
mod scratch;

use scratch::ScratchDir;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

#[cfg(target_os = "macos")]
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
    let arguments: Vec<&OsStr> = arguments
        .iter()
        .map(|argument| argument.as_os_str())
        .collect();
    run_raw(script, &arguments)
}

/// Like [`run`], for arguments that are not paths (flags, numbers).
fn run_raw(script: &str, arguments: &[&OsStr]) -> Output {
    let mut command = Command::new("python3");
    command.arg(root().join("scripts").join(script));
    command.args(arguments);
    command.output().expect("python3 must run validator")
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
    let directory = ScratchDir::new("screenshots-pass");
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
}

#[test]
fn screenshot_gate_rejects_below_threshold_and_missing_goldens() {
    let directory = ScratchDir::new("screenshots-fail");
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
    // Every read failure goes through the script's clean `error: ...` report,
    // and the message names the fixture that could not be read.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error:"), "{stderr}");
    assert!(stderr.contains("cpp-golden-missing.rgba"), "{stderr}");

    // An out-of-range threshold is an argument error, not a silent pass.
    let invalid = run_raw(
        "compare_screenshots.py",
        &[
            OsStr::new("--max-delta"),
            OsStr::new("-1"),
            reference.as_os_str(),
            candidate.as_os_str(),
        ],
    );
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("--max-delta must be >= 0"),
        "{}",
        String::from_utf8_lossy(&invalid.stderr)
    );
}

#[test]
fn performance_result_validator_enforces_realtime_acceptance() {
    let directory = ScratchDir::new("performance-result");
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
}

#[cfg(target_os = "macos")]
#[test]
fn bundle_verifier_requires_executable_resources_license_and_microphone_text() {
    let directory = ScratchDir::new("bundle-fixture");
    let bundle = directory.join("FreeWheeling.app");
    let contents = bundle.join("Contents");
    let resources = contents.join("Resources");
    fs::create_dir_all(contents.join("MacOS")).unwrap();
    fs::create_dir_all(resources.join("data")).unwrap();
    fs::create_dir_all(resources.join("licenses")).unwrap();
    // A system stub, not the crate's own binary: this test is about the
    // verifier's checks, and copying the real executable would make it depend
    // on the feature set it was built with (a `--features jack` build links a
    // Homebrew dylib, which the dependency scan correctly rejects).
    fs::copy("/usr/bin/true", contents.join("MacOS/freewheeling-plus")).unwrap();
    // The stub keeps its own architecture list (`/usr/bin/true` is a universal
    // binary), which is what the verifier compares against.
    let architectures = String::from_utf8(
        Command::new("lipo")
            .args(["-archs", "/usr/bin/true"])
            .output()
            .expect("lipo must be available on macOS")
            .stdout,
    )
    .unwrap();
    let architectures: Vec<&OsStr> = architectures.split_whitespace().map(OsStr::new).collect();
    assert!(!architectures.is_empty(), "lipo reported no architectures");
    let mut verify_arguments = vec![
        bundle.as_os_str(),
        OsStr::new("--fixture"),
        OsStr::new("--architectures"),
    ];
    verify_arguments.extend(architectures.iter().copied());
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
        contents.join("Info.plist"),
        br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>freewheeling-plus</string>
<key>NSMicrophoneUsageDescription</key><string>Record live sound.</string>
<key>CFBundleDocumentTypes</key><array><dict><key>CFBundleTypeName</key><string>Audio</string></dict></array>
</dict></plist>"#,
    )
    .unwrap();
    // `--fixture` is the explicit opt-in for a minimal test bundle: without
    // it the verifier requires a real resource seal, so a tampered bundle
    // cannot downgrade its own verification through Info.plist.
    let output = run_raw("verify_macos_bundle.py", &verify_arguments);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_file(resources.join("data/basic.sf2")).unwrap();
    let output = run_raw("verify_macos_bundle.py", &verify_arguments);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("basic.sf2"));
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
fn soundfont_handoff_marks_current_asset_public_domain_and_documents_replacements() {
    let handoff = fs::read_to_string(root().join("docs/basic-sf2-clean-room-handoff.md"))
        .expect("clean-room SoundFont handoff must be documented");
    for requirement in [
        "public\ndomain",
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
