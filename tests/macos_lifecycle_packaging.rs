use freewheeling_plus::macos::{
    application_support_path, bundle_resources_path, create_application_support_path,
};
use freewheeling_plus::macos_sdlmain::LaunchArguments;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn finder_arguments_keep_documents_already_supplied_by_launch_services() {
    let launch = LaunchArguments::from_args([
        OsString::from("Fweelin"),
        OsString::from("-psn_0_42"),
        OsString::from("/tmp/session.xml"),
    ]);
    assert!(launch.finder_launch());
    assert_eq!(
        launch.args(),
        [
            OsString::from("Fweelin"),
            OsString::from("/tmp/session.xml")
        ]
    );
}

#[test]
fn bundle_resources_are_derived_from_executable_location() {
    assert_eq!(
        bundle_resources_path(Path::new(
            "/Applications/FreeWheeling.app/Contents/MacOS/freewheeling-plus"
        )),
        Some(PathBuf::from(
            "/Applications/FreeWheeling.app/Contents/Resources"
        ))
    );
}

#[test]
fn application_support_creation_is_recursive_and_idempotent() {
    let home = std::env::temp_dir().join(format!("fweelin-home-{}", std::process::id()));
    let expected = application_support_path(&home);
    assert_eq!(create_application_support_path(&home).unwrap(), expected);
    assert_eq!(create_application_support_path(&home).unwrap(), expected);
    assert!(expected.is_dir());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn packaging_documents_fail_closed_soundfont_policy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = fs::read_to_string(root.join("scripts/package-macos-arm64.sh")).unwrap();
    let documentation = fs::read_to_string(root.join("PACKAGING.md")).unwrap();
    assert!(script.contains("BASIC_SF2_LICENSE_FILE"));
    assert!(script.contains("basic.sf2 has no proven distribution license"));
    assert!(documentation.contains("fails closed"));
    assert!(documentation.contains("5bf4c275a3dec39410ee130a0d90384be3ef6388"));
}

#[test]
fn verifier_runs_license_gate_after_integrity_and_signing_gates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let verifier = fs::read_to_string(root.join("scripts/verify_macos_bundle.py")).unwrap();
    let signature_gate = verifier.find("verify_signature(bundle").unwrap();
    let license_gate = verifier.find("sf2_license_path =").unwrap();
    assert!(signature_gate < license_gate);
    assert!(verifier.contains("sole distribution blocker"));

    let documentation = fs::read_to_string(root.join("PACKAGING.md")).unwrap();
    assert!(documentation.contains("NSMicrophoneUsageDescription"));
    assert!(documentation.contains("sealed-resource code-signature"));
}
