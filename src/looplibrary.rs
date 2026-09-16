//! Loop-library paths, disk discovery, and the owned loop-tray data model.
//!
//! The C++ implementation received `Fweelin` and `Loop` pointers.  Rust keeps
//! those dependencies explicit: callers provide the small traits needed by
//! the operations below, while this module owns its strings, entries, and
//! tray items.

use std::fs;
use std::path::{Path, PathBuf};
use crate::core::LoopTrayItem;
use crate::block;

pub const OUTPUT_LOOP_NAME: &str = "loop";
pub const OUTPUT_STREAM_NAME: &str = "live";
pub const OUTPUT_TIMING_EXT: &str = ".wav.usx";
pub const OUTPUT_DATA_EXT: &str = ".xml";

pub trait LibraryRuntime {
    fn library_path(&self) -> &Path;
    fn audio_extensions(&self) -> &[(&str, block::Codec)];
}

pub trait LoopSource {
    fn save_hash_text(&self) -> String;
}

/// The file that backs a library stub.
///
/// A codec of `Unknown` means the caller supplied the extension itself (the
/// stub's data file), so there is nothing to interpret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFileInfo {
    pub codec: block::Codec,
    /// The matching file, or `None` when the stub has no file at all.
    pub name: Option<PathBuf>,
}

impl Default for LibraryFileInfo {
    fn default() -> Self {
        Self {
            codec: block::Codec::Unknown,
            name: None,
        }
    }
}

impl LibraryFileInfo {
    /// Whether the stub has a file on disk.
    pub fn exists(&self) -> bool {
        self.name.is_some()
    }
}

pub struct LibraryHelper;

impl LibraryHelper {
    pub fn stubname_from_loop<R: LibraryRuntime, L: LoopSource>(runtime: &R, loop_: &L) -> PathBuf {
        runtime
            .library_path()
            .join(format!("{}-{}", OUTPUT_LOOP_NAME, loop_.save_hash_text()))
    }

    /// Next free `live<n>` stream path.
    ///
    /// The returned path is *not* reserved: the caller must create it with
    /// create-new semantics (`OpenOptions::new().create_new(true)`) and treat
    /// `AlreadyExists` as "try again".
    ///
    /// Returns `None` once the numeric suffix space is exhausted instead of
    /// re-probing the last name forever.
    pub fn next_available_stream_out_filename<R: LibraryRuntime>(
        runtime: &R,
        stream_num: &mut i32,
        display_name: &mut String,
    ) -> Option<PathBuf> {
        loop {
            let timing = runtime.library_path().join(format!(
                "{}{}{}",
                OUTPUT_STREAM_NAME, *stream_num, OUTPUT_TIMING_EXT
            ));
            if !timing.exists() {
                *display_name = format!("{}{}", OUTPUT_STREAM_NAME, *stream_num);
                return Some(runtime.library_path().join(display_name.as_str()));
            }
            *stream_num = stream_num.checked_add(1)?;
        }
    }

    pub fn loop_filename_from_stub<R: LibraryRuntime>(runtime: &R, stub: &Path) -> LibraryFileInfo {
        find_file_extensions(stub, runtime.audio_extensions())
    }

    pub fn data_filename_from_stub(stub: &Path) -> LibraryFileInfo {
        find_file_extensions(stub, &[(OUTPUT_DATA_EXT, block::Codec::Unknown)])
    }
}

fn find_file_extensions(stub: &Path, exts: &[(&str, block::Codec)]) -> LibraryFileInfo {
    for &(ext, codec) in exts {
        // Append at the `OsStr` level: `display()` is lossy for non-UTF-8
        // paths, which would point at a file that does not exist.
        let mut exact = stub.as_os_str().to_os_string();
        exact.push(ext);
        let exact = PathBuf::from(exact);
        if exact.is_file() {
            return LibraryFileInfo {
                codec,
                name: Some(exact),
            };
        }
    }
    // `parent()` reports `Some("")` for a bare filename, which is not a
    // directory that can be listed.
    let parent = stub
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let prefix = stub
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    // C++ tries every extension separately and `glob(3)` supplies its
    // matches in lexical order.  `read_dir` has no ordering guarantee, so it
    // must not decide which codec/name wins when a legacy wildcard load has
    // more than one candidate.
    // A directory that cannot be listed is not "no such file", but the caller
    // only sees the empty result (`LibraryFileInfo::default()`), so the failure
    // is reported here instead of being swallowed.
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "FreeWheeling: cannot list {} while resolving {}: {error}",
                parent.display(),
                stub.display()
            );
            return LibraryFileInfo::default();
        }
    };
    let mut directory: Vec<PathBuf> = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => directory.push(entry.path()),
            Err(error) => eprintln!(
                "FreeWheeling: cannot read an entry of {} while resolving {}: {error}",
                parent.display(),
                stub.display()
            ),
        }
    }
    for &(ext, codec) in exts {
        let mut candidates: Vec<_> = directory
            .iter()
            .filter(|path| {
                path.is_file()
                    && path.file_name().and_then(|name| name.to_str()).is_some_and(
                        |name| {
                            // C++ resolves a legacy stub with a glob, i.e. the
                            // candidates are `<stub>*<ext>`. Requiring a
                            // separator after the stub keeps a longer hash
                            // (`loop-<hash>ER.wav`) or an unrelated sibling out
                            // of the match, while a user-chosen name such as
                            // `-backup` stays valid.
                            if !name.starts_with(prefix)
                                || !name.ends_with(ext)
                                || name.len() < prefix.len() + ext.len()
                            {
                                return false;
                            }
                            name[prefix.len()..name.len() - ext.len()]
                                .chars()
                                .next()
                                .is_none_or(|next| !next.is_ascii_alphanumeric())
                        },
                    )
            })
            .cloned()
            .collect();
        candidates.sort();
        if let Some(path) = candidates.into_iter().next() {
            return LibraryFileInfo {
                codec,
                name: Some(path),
            };
        }
    }
    LibraryFileInfo::default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopLibraryEntry {
    pub name: String,
    pub filename: PathBuf,
    pub modified: Option<std::time::SystemTime>,
}

// LoopTrayItem is defined in core.rs — reuse instead of duplicating.
// See `crate::core::LoopTrayItem`.

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LoopTray {
    items: Vec<LoopTrayItem>,
}

impl LoopTray {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn items(&self) -> &[LoopTrayItem] {
        &self.items
    }
    /// Insert `item` at its sorted position, replacing an entry with the same
    /// loop id so the tray cannot hold ambiguous duplicates.
    pub fn insert(&mut self, item: LoopTrayItem) {
        if let Some(existing) = self.items.iter_mut().find(|i| i.loop_id == item.loop_id) {
            *existing = item;
            return;
        }
        let index = self
            .items
            .binary_search_by_key(&item.loop_id, |i| i.loop_id)
            .unwrap_or_else(|index| index);
        self.items.insert(index, item);
    }
    pub fn remove(&mut self, loop_id: i32) -> Option<LoopTrayItem> {
        self.items
            .iter()
            .position(|i| i.loop_id == loop_id)
            .map(|i| self.items.remove(i))
    }
    pub fn rename(&mut self, loop_id: i32, name: impl Into<String>) -> bool {
        if let Some(i) = self.items.iter_mut().find(|i| i.loop_id == loop_id) {
            i.name = name.into();
            i.default_name = false;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct R {
        path: PathBuf,
        exts: Vec<(&'static str, block::Codec)>,
    }
    impl LibraryRuntime for R {
        fn library_path(&self) -> &Path {
            &self.path
        }
        fn audio_extensions(&self) -> &[(&str, block::Codec)] {
            &self.exts
        }
    }
    struct L;
    impl LoopSource for L {
        fn save_hash_text(&self) -> String {
            "ABCD".into()
        }
    }
    #[test]
    fn paths_and_streams() {
        let d = std::env::temp_dir().join(format!("fw-test-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let r = R {
            path: d.clone(),
            exts: vec![(".wav", block::Codec::Wav)],
        };
        assert_eq!(
            LibraryHelper::stubname_from_loop(&r, &L),
            d.join("loop-ABCD")
        );
        let mut n = 0;
        let mut display = String::new();
        assert_eq!(
            LibraryHelper::next_available_stream_out_filename(&r, &mut n, &mut display),
            Some(d.join("live0"))
        );
        fs::write(d.join("loop-ABCD.wav"), b"x").unwrap();
        assert!(
            LibraryHelper::loop_filename_from_stub(&r, &d.join("loop-ABCD")).exists(),
            "the stub resolves its .wav companion"
        );
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn wildcard_resolution_uses_cpp_codec_priority_then_glob_order() {
        let d = std::env::temp_dir().join(format!("fw-loop-library-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let r = R {
            path: d.clone(),
            exts: vec![(".wav", block::Codec::Wav), (".ogg", block::Codec::Vorbis)],
        };
        // There is no exact `loop-HASH.wav`; both are wildcard candidates.
        // C++ tries WAV before Vorbis and glob sorts `-a` before `-z`.
        fs::write(d.join("loop-HASH-z.wav"), b"x").unwrap();
        fs::write(d.join("loop-HASH-a.wav"), b"x").unwrap();
        fs::write(d.join("loop-HASH-0.ogg"), b"x").unwrap();
        let found = LibraryHelper::loop_filename_from_stub(&r, &d.join("loop-HASH"));
        assert_eq!(found.codec, block::Codec::Wav);
        assert_eq!(found.name, Some(d.join("loop-HASH-a.wav")));
        let _ = fs::remove_dir_all(d);
    }
    impl Default for L {
        fn default() -> Self {
            Self
        }
    }
    #[test]
    fn tray_owns_sorted_items_and_renames() {
        let mut t = LoopTray::new();
        t.insert(LoopTrayItem {
            loop_id: 2,
            name: "b".into(),
            default_name: true,
            place_name: String::new(),
            x: -1,
            y: -1,
        });
        t.insert(LoopTrayItem {
            loop_id: 1,
            name: "a".into(),
            default_name: true,
            place_name: String::new(),
            x: -1,
            y: -1,
        });
        assert_eq!(t.items()[0].loop_id, 1);
        assert!(t.rename(1, "new"));
        assert!(!t.items()[0].default_name);
        assert!(t.remove(2).is_some());
    }
}
