//! Polling filesystem support for the loop library and browser.
//!
//! This deliberately uses the browser's filesystem boundary rather than a
//! platform watcher.  It consequently also behaves consistently on network
//! and virtual filesystems.

use crate::browser::{Browser, BrowserFileSystem, OsBrowserFileSystem};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    pub modified: Option<SystemTime>,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryChange {
    Added(PathBuf),
    Removed(PathBuf),
    Modified(PathBuf),
}

pub type LibrarySnapshot = BTreeMap<PathBuf, FileState>;

pub struct LibraryHelper<F = OsBrowserFileSystem> {
    filesystem: F,
    directory: PathBuf,
    snapshot: LibrarySnapshot,
}

impl LibraryHelper<OsBrowserFileSystem> {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self::with_filesystem(directory, OsBrowserFileSystem)
    }
}

impl<F: BrowserFileSystem> LibraryHelper<F> {
    pub fn with_filesystem(directory: impl Into<PathBuf>, filesystem: F) -> Self {
        Self {
            filesystem,
            directory: directory.into(),
            snapshot: LibrarySnapshot::new(),
        }
    }

    pub fn snapshot(&self) -> &LibrarySnapshot {
        &self.snapshot
    }

    pub fn scan(&mut self) -> io::Result<LibrarySnapshot> {
        let mut next = LibrarySnapshot::new();
        for entry in self.filesystem.entries(&self.directory)? {
            if entry.is_file {
                let metadata = std::fs::metadata(&entry.path).ok();
                next.insert(
                    entry.path,
                    FileState {
                        modified: entry.modified,
                        length: metadata.map(|m| m.len()).unwrap_or(0),
                    },
                );
            }
        }
        self.snapshot = next.clone();
        Ok(next)
    }

    pub fn changes(&mut self) -> io::Result<Vec<LibraryChange>> {
        let old = self.snapshot.clone();
        let new = self.scan()?;
        let mut changes = Vec::new();
        for path in new.keys() {
            match old.get(path) {
                None => changes.push(LibraryChange::Added(path.clone())),
                Some(state) if state != &new[path] => {
                    changes.push(LibraryChange::Modified(path.clone()))
                }
                _ => {}
            }
        }
        for path in old.keys() {
            if !new.contains_key(path) {
                changes.push(LibraryChange::Removed(path.clone()));
            }
        }
        Ok(changes)
    }

    pub fn watch_once(&mut self, browser: &mut Browser) -> Result<Vec<LibraryChange>, String> {
        let changes = self.changes().map_err(|e| {
            format!(
                "Failed to read library path '{}': {e}",
                self.directory.display()
            )
        })?;
        if !changes.is_empty() {
            browser.scan_with(&self.filesystem)?;
        }
        Ok(changes)
    }

    pub fn watch_once_with_callback<C>(
        &mut self,
        browser: &mut Browser,
        mut callback: C,
    ) -> Result<(), String>
    where
        C: FnMut(&LibraryChange),
    {
        for change in self.watch_once(browser)? {
            callback(&change);
        }
        Ok(())
    }
}

pub fn library_path(browser: &Browser) -> &Path {
    &browser.path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{BrowserFile, BrowserItemType};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct FakeFs(Arc<Mutex<Vec<BrowserFile>>>);
    impl BrowserFileSystem for FakeFs {
        fn entries(&self, _: &Path) -> io::Result<Vec<BrowserFile>> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn file(name: &str, modified: u64) -> BrowserFile {
        BrowserFile {
            path: PathBuf::from(name),
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(modified)),
            is_file: true,
        }
    }

    #[test]
    fn polling_reports_add_modify_and_remove_once() {
        let files = Arc::new(Mutex::new(vec![file("loop-aa-a.wav", 1)]));
        let fs = FakeFs(files.clone());
        let mut helper = LibraryHelper::with_filesystem("library", fs);
        assert_eq!(
            helper.changes().unwrap(),
            vec![LibraryChange::Added(PathBuf::from("loop-aa-a.wav"))]
        );
        assert!(helper.changes().unwrap().is_empty());
        files.lock().unwrap()[0] = file("loop-aa-a.wav", 2);
        assert_eq!(
            helper.changes().unwrap(),
            vec![LibraryChange::Modified(PathBuf::from("loop-aa-a.wav"))]
        );
        files.lock().unwrap().clear();
        assert_eq!(
            helper.changes().unwrap(),
            vec![LibraryChange::Removed(PathBuf::from("loop-aa-a.wav"))]
        );
    }

    #[test]
    fn watch_rescans_browser_and_delivers_callback_after_scan() {
        let files = Arc::new(Mutex::new(vec![file("loop-aa-a.wav", 1)]));
        let mut helper = LibraryHelper::with_filesystem("library", FakeFs(files.clone()));
        let mut browser = Browser::new("library", BrowserItemType::Loop);
        let mut seen = Vec::new();
        helper
            .watch_once_with_callback(&mut browser, |change| {
                seen.push(change.clone());
            })
            .unwrap();
        assert_eq!(browser.len(), 1);
        assert_eq!(seen.len(), 1);
    }
}
