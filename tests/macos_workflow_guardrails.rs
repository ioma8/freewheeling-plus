use std::fs;
use std::path::Path;

#[test]
fn macos_workflow_runner_is_native_and_fail_closed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = fs::read_to_string(root.join("scripts/run_macos_workflow_capture.py")).unwrap();
    let docs = fs::read_to_string(root.join("acceptance-evidence/macos/README.md")).unwrap();
    for required in [
        "system_profiler",
        "codesign",
        "osascript",
        "screencapture",
        "arm64",
        "temporary.replace(attestation)",
    ] {
        assert!(
            script.contains(required),
            "runner missing guardrail: {required}"
        );
    }
    assert!(script.contains("if sys.platform != \"darwin\""));
    assert!(script.contains("attestation.exists()"));
    assert!(docs.contains("Do not press ahead"));
    assert!(docs.contains("virtual device"));
}
