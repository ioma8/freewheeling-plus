//! macOS application integration.
//!
//! The module keeps platform policy separate from the ObjC runtime globals.
//! Application support path, bundle resources, and directory creation are
//! exposed as free functions so they can be tested from any host.

use std::path::{Path, PathBuf};

pub const APPLICATION_NAME: &str = "Fweelin";
pub const BUNDLE_IDENTIFIER: &str = "org.freewheeling.freewheeling-plus";

/// Resolve the traditional per-user macOS support directory.
pub fn application_support_path(home: &Path) -> PathBuf {
    home.join("Library/Application Support").join(APPLICATION_NAME)
}

/// Return `Contents/Resources` when `executable` is inside an application
/// bundle.  No current-directory assumptions are involved, which is important
/// for Finder launches (Finder does not promise a useful working directory).
/// Return `Contents/Resources` when `executable` is inside an application
/// bundle.
///
/// The path is canonicalized first, so a symlinked executable or `..`
/// components still resolve to the bundle.  The layout itself is assumed to be
/// the standard `<name>.app/Contents/MacOS/<binary>`: a non-standard bundle
/// returns `None`, which callers treat as "no bundled resources".
pub fn bundle_resources_path(executable: &Path) -> Option<PathBuf> {
    let resolved = executable.canonicalize().unwrap_or_else(|_| executable.to_path_buf());
    let parent = resolved.parent()?;
    if parent.file_name()? != "MacOS" {
        return None;
    }
    let bundle = parent.parent()?.parent()?;
    // The extension is not stripped by any step above, so `.app` here really
    // means an application bundle.
    if bundle.extension().is_some_and(|ext| ext == "app") {
        return Some(bundle.join("Contents/Resources"));
    }
    None
}

/// Create the writable per-user directory before any persistence subsystem is
/// started.
/// Create the per-user application support directory.
///
/// The `io::Error` is returned unchanged so callers can inspect the kind
/// (permission denied versus no space, for example).
pub fn create_application_support_path(home: &Path) -> std::io::Result<PathBuf> {
    let path = application_support_path(home);
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

#[cfg(target_os = "macos")]
mod cocoa {
    use super::*;
    use objc2::MainThreadMarker;
    use objc2_foundation::NSAutoreleasePool;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    /// Cocoa-backed implementation backed by safe objc2 bindings.
    pub struct CocoaPlatform {
        pool: Option<objc2::rc::Retained<NSAutoreleasePool>>,
        /// Thread that created `pool`. `NSAutoreleasePool` is *per thread*, so
        /// the pool must be drained on the thread that made it.
        pool_thread: Option<std::thread::ThreadId>,
        /// Set while [`CocoaPlatform::initialize`] has run and
        /// [`CocoaPlatform::cleanup`] has not: observable lifecycle state.
        initialized: bool,
    }

    impl CocoaPlatform {
        pub fn new() -> Self {
            Self {
                pool: None,
                pool_thread: None,
                initialized: false,
            }
        }

        /// Whether [`CocoaPlatform::initialize`] has run without a matching
        /// [`CocoaPlatform::cleanup`].
        pub fn is_initialized(&self) -> bool {
            self.initialized
        }

        /// Per-user application support directory.
        ///
        /// Resolved through `NSHomeDirectory` rather than `$HOME`, which a
        /// Finder/launchd launch may leave unset or stale.
        pub fn application_support_dir(&self) -> std::io::Result<PathBuf> {
            let home = objc2_foundation::NSHomeDirectory();
            let home = PathBuf::from(home.to_string());
            if home.as_os_str().is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "the home directory is not available",
                ));
            }
            // The `io::Error` is propagated rather than formatted so callers
            // keep the `ErrorKind` and the source chain.
            create_application_support_path(&home)
        }

        /// Create the thread's autorelease pool.
        ///
        /// Calling this twice without an intervening `cleanup` releases the
        /// previous pool first, so the thread never owns two.
        pub fn initialize(&mut self) -> Result<(), String> {
            self.cleanup();
            // SAFETY: NSAutoreleasePool::new interacts with the ObjC runtime's
            // autorelease mechanism; the pool is stored together with the id
            // of the creating thread, and `cleanup` (also from `Drop`) only
            // drains it on that same thread.
            self.pool = Some(unsafe { NSAutoreleasePool::new() });
            self.pool_thread = Some(std::thread::current().id());
            self.initialized = true;
            Ok(())
        }

        /// Put the process in the foreground.
        ///
        /// Only the activation policy and the foreground activation are set
        /// (the historical name promised an `NSMenu`); the UI is drawn by the
        /// SDL window rather than AppKit, so no menu bar is installed.
        pub fn activate_foreground(&mut self) -> Result<(), String> {
            let marker = MainThreadMarker::new()
                .ok_or_else(|| "CocoaPlatform must be used on the main thread".to_string())?;
            let app = NSApplication::sharedApplication(marker);
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            Ok(())
        }

        /// Drain the pool.
        ///
        /// Safe to call more than once (and on an instance that was never
        /// initialized): the second call is a no-op. Must run on the creating
        /// thread, which is checked here - draining another thread's pool is
        /// undefined behaviour.
        pub fn cleanup(&mut self) {
            if let Some(thread) = self.pool_thread.take()
                && thread != std::thread::current().id()
            {
                eprintln!(
                    "FreeWheeling: refusing to drain the autorelease pool from a different thread"
                );
                self.pool = None;
                self.initialized = false;
                return;
            }
            drop(self.pool.take());
            self.initialized = false;
        }
    }

    impl Drop for CocoaPlatform {
        fn drop(&mut self) {
            // The pool must be drained even when the caller never calls
            // `cleanup` (or returns early on an error path).
            self.cleanup();
        }
    }

    impl Default for CocoaPlatform {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(target_os = "macos")]
pub use cocoa::CocoaPlatform;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_application_support_location() {
        assert_eq!(
            application_support_path(Path::new("/Users/alice")),
            PathBuf::from("/Users/alice/Library/Application Support/Fweelin")
        );
    }

    #[test]
    fn resolves_bundle_resources_without_using_cwd() {
        assert_eq!(
            bundle_resources_path(Path::new(
                "/Applications/Fweelin.app/Contents/MacOS/Fweelin"
            )),
            Some(PathBuf::from(
                "/Applications/Fweelin.app/Contents/Resources"
            ))
        );
        assert_eq!(bundle_resources_path(Path::new("/tmp/fweelin")), None);
    }
}
