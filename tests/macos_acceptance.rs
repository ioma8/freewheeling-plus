#![cfg(target_os = "macos")]

use freewheeling_plus::macos::{CocoaPlatform, application_support_path};
use freewheeling_plus::macos_sdlmain::{LaunchArguments, run_macos};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
// Only the `smoke-test` build runs the packaged binary (see below).
#[cfg(feature = "smoke-test")]
use std::process::Command;
use std::sync::Mutex;

#[path = "support/scratch.rs"]
mod scratch;

use scratch::ScratchDir;

fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

/// Serializes the tests that change or depend on the process-global working
/// directory.
///
/// `std::env::set_current_dir` affects the whole process, so a lock held by one
/// test only helps if every cwd-sensitive test in this binary takes it.
static CURRENT_DIR_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn cocoa_platform_has_headless_support_path_and_paired_lifecycle() {
    let platform = CocoaPlatform::default();
    let support = platform.application_support_dir().unwrap();
    let home = std::env::var_os("HOME").unwrap();
    assert_eq!(support, application_support_path(Path::new(&home)));

    let mut platform = platform;
    platform.initialize().unwrap();
    platform.cleanup();
    platform.cleanup();
}

#[test]
fn sdlmain_filters_finder_launch_and_handles_bundle_parent() {
    let mut launch = LaunchArguments::from_args(strings(&["Fweelin", "-psn_0_123"]));
    assert!(launch.finder_launch());
    assert!(launch.open_file("Dropped.wav", false));
    assert!(!launch.open_file("Too-late.wav", true));
    assert_eq!(launch.args(), strings(&["Fweelin", "Dropped.wav"]));

    let bundle_executable = PathBuf::from("/Applications/Fweelin.app/Contents/MacOS/Fweelin");
    assert_eq!(
        bundle_executable.parent().map(Path::to_path_buf),
        Some(PathBuf::from("/Applications/Fweelin.app/Contents/MacOS"))
    );
}

#[test]
fn sdlmain_changes_to_bundle_directory_before_handoff() {
    let _lock = CURRENT_DIR_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let original = std::env::current_dir().unwrap();
    let root = ScratchDir::new("macos-bundle-directory");
    let bundle = root.join("Fweelin.app/Contents/MacOS/Fweelin");
    fs::create_dir_all(bundle.parent().unwrap()).unwrap();
    let expected_directory = fs::canonicalize(bundle.parent().unwrap()).unwrap();

    let launch = LaunchArguments::from_args(strings(&["Fweelin", "-psn_0_123"]));
    let status = run_macos(&launch, &bundle, |_| {
        assert_eq!(std::env::current_dir().unwrap(), expected_directory);
        23
    })
    .unwrap();
    assert_eq!(status, 23);
    std::env::set_current_dir(original).unwrap();
}

// The harness only exists in a `smoke-test` build (Cargo.toml); CI runs the
// test suite with that feature enabled.
#[cfg(feature = "smoke-test")]
#[test]
fn compiled_binary_smoke_test_succeeds() {
    let binary = env!("CARGO_BIN_EXE_freewheeling-plus");
    let output = Command::new(binary).arg("--smoke-test").output().unwrap();
    assert!(
        output.status.success(),
        "--smoke-test failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
