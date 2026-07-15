use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn linux_release_lane_is_reproducible_and_hardware_independent() {
    let package = fs::read_to_string(root().join("scripts/linux/package-release.sh")).unwrap();
    assert!(package.contains("SOURCE_DATE_EPOCH"));
    assert!(package.contains("--sort=name"));
    assert!(package.contains("gzip -n"));
    assert!(package.contains("BASIC_SF2_LICENSE_FILE"));

    let acceptance =
        fs::read_to_string(root().join("scripts/linux/run-virtual-acceptance.sh")).unwrap();
    assert!(acceptance.contains("jackd --no-realtime -d dummy"));
    assert!(acceptance.contains("locate 48000\\nplay\\nquit"));
    assert!(acceptance.contains(":midi_in_0$"));
    assert!(acceptance.contains(":midi_out_0$"));

    let workflow =
        fs::read_to_string(root().join("scripts/linux/run-virtual-workflow.sh")).unwrap();
    assert!(workflow.contains("FWP_ACCEPTANCE_REVISION=\"$REVISION\""));
    assert!(workflow.contains("performance_result_sha256"));
    assert!(workflow.contains("temporary.replace(attestation_path)"));
}

#[test]
fn linux_lane_scripts_pass_static_validation() {
    let status = Command::new("sh")
        .arg(root().join("scripts/linux/validate.sh"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn linux_backend_uses_direct_alsa_not_amixer_processes() {
    let native = fs::read_to_string(root().join("src/linux_native.rs")).unwrap();
    assert!(native.contains("alsa::hctl::HCtl"));
    assert!(!native.contains("Command::new(\"amixer\")"));
}
