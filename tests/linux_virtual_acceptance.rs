use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn linux_release_lane_is_reproducible_and_hardware_independent() {
    let package = fs::read_to_string(root().join("scripts/linux/package-release.sh")).unwrap();
    assert!(package.contains("SOURCE_DATE_EPOCH"));
    assert!(package.contains("--sort=name"));
    // The archive is compressed in a separate step now (a pipeline would hide
    // a failing `tar`), still with gzip's reproducibility flag.
    assert!(package.contains("gzip -c -n -9"));
    assert!(!package.contains("BASIC_SF2_LICENSE_FILE"));

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
    // The attestation status is derived from the acceptance step's validation
    // stamp rather than asserted unconditionally.
    assert!(workflow.contains("acceptance validation stamp missing"));
    assert!(acceptance.contains("validated_result_sha256"));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_lane_scripts_pass_static_validation() {
    let script = root().join("scripts/linux/validate.sh");
    // Output is captured: `.status()` discards the script's stderr, which is
    // exactly what makes a non-actionable CI failure.
    let output = Command::new("sh")
        .arg(&script)
        .output()
        .unwrap_or_else(|error| panic!("cannot run sh {}: {error}", script.display()));
    assert!(
        output.status.success(),
        "validate.sh failed with {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn linux_backend_uses_amixer_for_hardware_control() {
    let mixer = fs::read_to_string(root().join("src/amixer.rs")).unwrap();
    assert!(mixer.contains("Command::new(\"amixer\")"));
    assert!(mixer.contains("cset"));
}
