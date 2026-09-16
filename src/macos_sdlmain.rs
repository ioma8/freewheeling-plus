//! The small, platform-specific part of the old `SDLMain` entry point.
//!
//! Cocoa is responsible for delivering the application lifecycle, but the
//! rules it used for arguments and the working directory are useful without
//! Cocoa too.  They live here as ordinary Rust so they can be tested on every
//! host.  The macOS adapter calls [`run_macos`] from its real entry point.

use std::ffi::OsString;
use std::io;
use std::path::Path;

/// Arguments as seen by the application after SDL's Finder launch argument
/// has been consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchArguments {
    args: Vec<OsString>,
    finder_launch: bool,
}

impl LaunchArguments {
    /// Reproduces SDLMain's `-psn...` handling without losing non-UTF-8 args.
    ///
    /// The iterator **must** include `argv[0]`: the `-psn` argument is only
    /// ever the first argument after the program name, and that position is
    /// what tells a Finder launch from a user-supplied `-psn...` value passed
    /// later on the command line.
    pub fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut args: Vec<OsString> = args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect();
        debug_assert!(
            args.len() != 1
                || !args[0]
                    .to_str()
                    .is_some_and(|arg| arg.starts_with("-psn")),
            "from_args expects argv[0]; passing only the -psn argument would misdetect a Finder launch",
        );
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

/// Run the application handoff after the bundle setup.  Keeping this callback
/// based makes the ordering and error propagation testable without Cocoa.
///
/// Side effect: a Finder launch changes the process-global working directory
/// to the bundle's parent.  That must happen before other threads start (and
/// before the video/audio subsystems hand off), because those threads share
/// the process cwd.
pub fn run_macos<F>(
    launch: &LaunchArguments,
    bundle_path: impl AsRef<Path>,
    app_main: F,
) -> io::Result<i32>
where
    F: FnOnce(&[OsString]) -> i32,
{
    if launch.finder_launch {
        let parent = bundle_path.as_ref().parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "application bundle has no parent",
            )
        })?;
        // NOTE: process-global side effect - must run before other threads
        // start.
        std::env::set_current_dir(parent)?;
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
    use std::path::PathBuf;
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

    /// Run `body` with the process cwd temporarily changed, restoring it even
    /// when the body fails.
    fn with_isolated_cwd<T>(body: impl FnOnce() -> T) -> T {
        let original = std::env::current_dir().expect("cwd is readable");
        let sandbox = std::env::temp_dir().join(format!(
            "fweelin-sdlmain-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&sandbox).expect("sandbox directory");
        std::env::set_current_dir(&sandbox).expect("enter sandbox");
        let result = body();
        std::env::set_current_dir(&original).expect("restore cwd");
        let _ = std::fs::remove_dir(&sandbox);
        result
    }

    #[test]
    fn a_finder_launch_moves_the_working_directory_to_the_bundle_parent() {
        with_isolated_cwd(|| {
            let bundle_parent = PathBuf::from("/Applications");
            let launch = LaunchArguments::from_args(args(&["fweelin", "-psn_0_1"]));
            let status = run_macos(&launch, bundle_parent.join("Foo.app"), |_| 3).unwrap();
            assert_eq!(status, 3);
            assert_eq!(std::env::current_dir().unwrap(), bundle_parent);

            // A normal launch leaves the cwd alone.
            let launch = LaunchArguments::from_args(args(&["fweelin"]));
            run_macos(&launch, bundle_parent.join("Foo.app"), |_| 0).unwrap();
            assert_eq!(std::env::current_dir().unwrap(), bundle_parent);
        });
    }

    #[test]
    fn a_bundle_without_a_parent_is_reported() {
        let launch = LaunchArguments::from_args(args(&["fweelin", "-psn_0_1"]));
        let error = run_macos(&launch, "/", |_| 0).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
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
