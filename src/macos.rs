//! macOS application integration.
//!
//! The old Cocoa entry point mixed policy with Objective-C globals.  This
//! module keeps the policy in [`Platform`] so it can be exercised without a
//! window server, while the real Cocoa implementation is compiled only on
//! macOS.

use std::path::{Path, PathBuf};

pub const APPLICATION_NAME: &str = "Fweelin";
pub const BUNDLE_IDENTIFIER: &str = "org.freewheeling.freewheeling-plus";

/// Operations which the application performs at its platform boundary.
pub trait Platform {
    type Error;

    fn application_support_dir(&self) -> Result<PathBuf, Self::Error>;
    fn initialize(&mut self) -> Result<(), Self::Error>;
    fn set_menu_and_foreground(&mut self) -> Result<(), Self::Error>;
    fn cleanup(&mut self);
}

/// RAII owner for Cocoa/SDL application-thread setup. Construction performs
/// foreground activation only after platform initialization succeeds.
pub struct PlatformSession<P: Platform> {
    platform: P,
}

impl<P: Platform> PlatformSession<P> {
    pub fn start(mut platform: P) -> Result<Self, P::Error> {
        platform.initialize()?;
        if let Err(error) = platform.set_menu_and_foreground() {
            platform.cleanup();
            return Err(error);
        }
        Ok(Self { platform })
    }

    pub fn platform(&self) -> &P {
        &self.platform
    }

    pub fn platform_mut(&mut self) -> &mut P {
        &mut self.platform
    }
}

impl<P: Platform> Drop for PlatformSession<P> {
    fn drop(&mut self) {
        self.platform.cleanup();
    }
}

/// Resolve the traditional per-user macOS support directory.
pub fn application_support_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join(APPLICATION_NAME)
}

/// Return `Contents/Resources` when `executable` is inside an application
/// bundle.  No current-directory assumptions are involved, which is important
/// for Finder launches (Finder does not promise a useful working directory).
pub fn bundle_resources_path(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    Some(macos.parent()?.join("Resources"))
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
    }

    impl Default for CocoaPlatform {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Platform for CocoaPlatform {
        type Error = String;

        fn application_support_dir(&self) -> Result<PathBuf, Self::Error> {
            let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
            create_application_support_path(Path::new(&home))
                .map_err(|error| format!("could not create application support directory: {error}"))
        }

        fn initialize(&mut self) -> Result<(), Self::Error> {
            // SAFETY: NSAutoreleasePool::new is unsafe because the pool
            // interacts with the ObjC runtime's autorelease mechanism, but
            // this is the standard, safe-on-main-thread creation pattern.
            self.pool = Some(unsafe { NSAutoreleasePool::new() });
            self.initialized = true;
            Ok(())
        }

        fn set_menu_and_foreground(&mut self) -> Result<(), Self::Error> {
            let marker = MainThreadMarker::new()
                .ok_or_else(|| "CocoaPlatform must be used on the main thread".to_string())?;
            let app = NSApplication::sharedApplication(marker);
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            Ok(())
        }

        fn cleanup(&mut self) {
            drop(self.pool.take());
            self.initialized = false;
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
