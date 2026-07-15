//! The small, platform-specific part of the old `SDLMain` entry point.
//!
//! Cocoa is responsible for delivering the application lifecycle, but the
//! rules it used for arguments and the working directory are useful without
//! Cocoa too.  They live here as ordinary Rust so they can be tested on every
//! host.  The macOS adapter calls [`run_macos`] from its real entry point.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// Arguments as seen by the application after SDL's Finder launch argument
/// has been consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchArguments {
    args: Vec<OsString>,
    finder_launch: bool,
}

impl LaunchArguments {
    /// Reproduces SDLMain's `-psn...` handling without losing non-UTF-8 args.
    pub fn from_args<I>(args: I) -> Self
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut args = args.into_iter().collect::<Vec<_>>();
        let finder_launch = args
            .get(1)
            .is_some_and(|arg| arg.to_str().is_some_and(|arg| arg.starts_with("-psn")));
        if finder_launch {
            args.remove(1);
        }
        Self {
            args,
            finder_launch,
        }
    }

    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    pub fn finder_launch(&self) -> bool {
        self.finder_launch
    }

    /// Accepts document-open events only before application startup, as
    /// SDLMain did.  Returns whether the path was accepted.
    pub fn open_file(&mut self, path: impl Into<OsString>, app_started: bool) -> bool {
        if !self.finder_launch || app_started {
            return false;
        }
        self.args.push(path.into());
        true
    }
}

/// The executable directory used by the legacy SDLMain launcher.
pub fn app_parent_directory(bundle_path: impl AsRef<Path>) -> Option<PathBuf> {
    let path = bundle_path.as_ref();
    path.parent().map(Path::to_path_buf)
}

/// Change to the executable directory. Resource discovery itself is based on
/// the executable URL, but this preserves compatibility with relative paths
/// used by old Finder-launched configurations.
pub fn set_working_directory(bundle_path: impl AsRef<Path>) -> io::Result<()> {
    let parent = app_parent_directory(bundle_path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "application bundle has no parent",
        )
    })?;
    std::env::set_current_dir(parent)
}

/// Run the application handoff after the bundle setup.  Keeping this callback
/// based makes the ordering and error propagation testable without Cocoa.
pub fn run_macos<F>(
    launch: &LaunchArguments,
    bundle_path: impl AsRef<Path>,
    app_main: F,
) -> io::Result<i32>
where
    F: FnOnce(&[OsString]) -> i32,
{
    if launch.finder_launch {
        set_working_directory(bundle_path)?;
    }
    Ok(app_main(&launch.args))
}

#[cfg(target_os = "macos")]
pub fn application_main<F>(
    args: impl IntoIterator<Item = OsString>,
    bundle_path: impl AsRef<Path>,
    app_main: F,
) -> i32
where
    F: FnOnce(&[OsString]) -> i32,
{
    let launch = LaunchArguments::from_args(args);
    match run_macos(&launch, bundle_path, app_main) {
        Ok(status) => status,
        Err(error) => {
            eprintln!("FreeWheeling macOS setup failed: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn forwards_normal_command_line_unchanged() {
        let launch = LaunchArguments::from_args(args(&["fweelin", "--foo", "file.wav"]));
        assert!(!launch.finder_launch());
        assert_eq!(launch.args(), args(&["fweelin", "--foo", "file.wav"]));
    }

    #[test]
    fn consumes_psn_and_accepts_finder_documents_before_start() {
        let mut launch =
            LaunchArguments::from_args(args(&["fweelin", "-psn_0_123", "already.wav"]));
        assert!(launch.finder_launch());
        assert!(launch.open_file("one.wav", false));
        assert!(!launch.open_file("two.wav", true));
        assert_eq!(launch.args(), args(&["fweelin", "already.wav", "one.wav"]));
    }

    #[test]
    fn bundle_parent_is_used_for_working_directory() {
        assert_eq!(
            app_parent_directory("/Applications/Foo.app/Contents/MacOS/Foo"),
            Some(PathBuf::from("/Applications/Foo.app/Contents/MacOS"))
        );
        assert_eq!(
            app_parent_directory("/Applications/Foo.app"),
            Some(PathBuf::from("/Applications"))
        );
    }

    #[test]
    fn setup_precedes_handoff_and_preserves_status() {
        let launch = LaunchArguments::from_args(args(&["fweelin", "--audio"]));
        let status = run_macos(&launch, ".", |received| {
            assert_eq!(received, args(&["fweelin", "--audio"]));
            17
        })
        .unwrap();
        assert_eq!(status, 17);
    }
}
