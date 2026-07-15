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
    use std::ffi::c_void;
    use std::ptr;

    #[link(name = "Cocoa", kind = "framework")]
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const i8) -> *mut c_void;
        fn sel_registerName(name: *const i8) -> *mut c_void;
        fn objc_msgSend() -> *mut c_void;
    }

    /// Cocoa-backed implementation.  The pool is deliberately owned here so
    /// setup and teardown remain paired even when the caller uses a worker
    /// thread (the behavior of FweelinMac::Setup/TakedownCocoaThread).
    pub struct CocoaPlatform {
        pool: *mut c_void,
        initialized: bool,
    }

    impl CocoaPlatform {
        pub fn new() -> Self {
            Self {
                pool: ptr::null_mut(),
                initialized: false,
            }
        }

        unsafe fn send(receiver: *mut c_void, selector: &[u8]) -> *mut c_void {
            let selector = unsafe { sel_registerName(selector.as_ptr().cast()) };
            // objc_msgSend has a variadic, selector-dependent ABI. These two
            // calls have no arguments and return an object pointer.
            unsafe {
                std::mem::transmute::<
                    unsafe extern "C" fn() -> *mut c_void,
                    unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void,
                >(objc_msgSend)(receiver, selector)
            }
        }

        unsafe fn send_bool(receiver: *mut c_void, selector: &[u8], value: bool) {
            let selector = unsafe { sel_registerName(selector.as_ptr().cast()) };
            let send = unsafe {
                std::mem::transmute::<
                    unsafe extern "C" fn() -> *mut c_void,
                    unsafe extern "C" fn(*mut c_void, *mut c_void, bool),
                >(objc_msgSend)
            };
            unsafe { send(receiver, selector, value) };
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
            unsafe {
                let class = objc_getClass(c"NSAutoreleasePool".as_ptr());
                self.pool = Self::send(class, b"alloc\0");
                self.pool = Self::send(self.pool, b"init\0");
            }
            self.initialized = !self.pool.is_null();
            self.initialized
                .then_some(())
                .ok_or_else(|| "could not create autorelease pool".into())
        }

        fn set_menu_and_foreground(&mut self) -> Result<(), Self::Error> {
            unsafe {
                let app = objc_getClass(c"NSApplication".as_ptr());
                let app = Self::send(app, b"sharedApplication\0");
                // SDL may create the menu later; setting a regular activation
                // policy here makes Finder launches own a Dock icon and menu
                // bar immediately. NSApplicationActivationPolicyRegular == 0.
                let selector = sel_registerName(c"setActivationPolicy:".as_ptr());
                let send = std::mem::transmute::<
                    unsafe extern "C" fn() -> *mut c_void,
                    unsafe extern "C" fn(*mut c_void, *mut c_void, i64) -> bool,
                >(objc_msgSend);
                let _ = send(app, selector, 0);
                Self::send_bool(app, b"activateIgnoringOtherApps:\0", true);
            }
            Ok(())
        }

        fn cleanup(&mut self) {
            if !self.pool.is_null() {
                unsafe {
                    let _ = Self::send(self.pool, b"drain\0");
                }
                self.pool = ptr::null_mut();
            }
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
