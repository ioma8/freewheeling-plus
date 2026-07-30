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
pub fn bundle_resources_path(executable: &Path) -> Option<PathBuf> {
    let parent = executable.parent()?;
    let parent_name = parent.file_name()?;
    if parent_name == "MacOS" {
        let bundle = parent.parent()?.parent()?;
        if bundle.extension().is_some_and(|ext| ext == "app") {
            return Some(bundle.join("Contents/Resources"));
        }
    }
    None
}

/// Create the writable per-user directory before any persistence subsystem is
/// started.
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
        initialized: bool,
    }

    impl CocoaPlatform {
        pub fn new() -> Self {
            Self {
                pool: None,
                initialized: false,
            }
        }

        pub fn application_support_dir(&self) -> Result<PathBuf, String> {
            let home =
                std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
            create_application_support_path(Path::new(&home))
                .map_err(|error| format!("could not create application support directory: {error}"))
        }

        pub fn initialize(&mut self) -> Result<(), String> {
            // SAFETY: NSAutoreleasePool::new is unsafe because the pool
            // interacts with the ObjC runtime's autorelease mechanism, but
            // this is the standard, safe-on-main-thread creation pattern.
            self.pool = Some(unsafe { NSAutoreleasePool::new() });
            self.initialized = true;
            Ok(())
        }

        pub fn set_menu_and_foreground(&mut self) -> Result<(), String> {
            let marker = MainThreadMarker::new()
                .ok_or_else(|| "CocoaPlatform must be used on the main thread".to_string())?;
            let app = NSApplication::sharedApplication(marker);
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            Ok(())
        }

        pub fn cleanup(&mut self) {
            drop(self.pool.take());
            self.initialized = false;
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
