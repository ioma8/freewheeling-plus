#![cfg(target_os = "macos")]

use freewheeling_plus::macos::{CocoaPlatform, Platform, application_support_path};
use freewheeling_plus::macos_sdlmain::{LaunchArguments, run_macos};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

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
        freewheeling_plus::macos_sdlmain::app_parent_directory(&bundle_executable),
        Some(PathBuf::from("/Applications/Fweelin.app/Contents/MacOS"))
    );
}

#[test]
fn sdlmain_changes_to_bundle_directory_before_handoff() {
    static CURRENT_DIR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _lock = CURRENT_DIR_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap();
    let original = std::env::current_dir().unwrap();
    let root = std::env::temp_dir().join(format!(
        "freewheeling-plus-macos-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
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
    fs::remove_dir_all(root).unwrap();
}

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
