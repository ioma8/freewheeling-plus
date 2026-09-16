//! A scratch directory that cleans itself up, shared by the integration tests.
//!
//! Included with `#[path = "support/scratch.rs"] mod scratch;` so a test file
//! gets the guard without pulling in the crate-wide fixtures in
//! `tests/support/mod.rs`.

use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

/// A temporary directory removed when the guard drops.
///
/// The path carries a per-run nonce (pid, wall-clock nanoseconds and a process
/// counter), so a recycled pid or two tests running concurrently can never
/// share a directory and pick up each other's leftovers. Removal happens on
/// the unwind path too: a failing assertion must not leak state that the next
/// run would silently reuse.
pub struct ScratchDir {
    path: PathBuf,
}

// The module is included by several test binaries; not every one of them uses
// every helper.
#[allow(dead_code)]
impl ScratchDir {
    /// Create a fresh scratch directory below the system temporary directory.
    ///
    /// # Panics
    ///
    /// Panics when the directory cannot be created: a test that cannot get the
    /// scratch space it asked for must fail loudly.
    pub fn new(label: &str) -> Self {
        assert!(
            !label.is_empty(),
            "scratch directory label must not be empty"
        );
        assert!(
            label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "scratch directory label must be ASCII alphanumeric or '-', got {label:?}"
        );
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "freewheeling-{label}-{}-{nonce}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("cannot create scratch directory {}: {error}", path.display())
        });
        Self { path }
    }

    /// Create (with parents) a directory below the scratch directory.
    pub fn subdir(&self, relative: &str) -> PathBuf {
        let path = self.path.join(relative);
        fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("cannot create scratch subdirectory {}: {error}", path.display())
        });
        path
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Deref for ScratchDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        match fs::remove_dir_all(&self.path) {
            Ok(()) => {}
            // `NotFound` means the test (or the code under test) removed it
            // already; anything else would otherwise leave the tree behind
            // without a trace.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "warning: cannot remove scratch directory {}: {error}",
                self.path.display()
            ),
        }
    }
}
