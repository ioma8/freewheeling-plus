//! Loop-library paths, disk discovery, and the owned loop-tray data model.
//!
//! The C++ implementation received `Fweelin` and `Loop` pointers.  Rust keeps
//! those dependencies explicit: callers provide the small traits needed by
//! the operations below, while this module owns its strings, entries, and
//! tray items.

use std::fs;
use std::path::{Path, PathBuf};
use crate::core::LoopTrayItem;

pub const OUTPUT_LOOP_NAME: &str = "loop";
pub const OUTPUT_STREAM_NAME: &str = "live";
pub const OUTPUT_TIMING_EXT: &str = ".wav.usx";
pub const OUTPUT_DATA_EXT: &str = ".xml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codec {
    #[default]
    Unknown,
    Vorbis,
    Wav,
    Flac,
    Au,
}

pub trait LibraryRuntime {
    fn library_path(&self) -> &Path;
    fn audio_extensions(&self) -> &[(&str, Codec)];
}

pub trait LoopSource {
    fn save_hash_text(&self) -> String;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFileInfo {
    pub exists: bool,
    pub codec: Codec,
    pub name: Option<PathBuf>,
}

impl Default for LibraryFileInfo {
    fn default() -> Self {
        Self {
            exists: false,
            codec: Codec::Unknown,
            name: None,
        }
    }
}

pub struct LibraryHelper;

impl LibraryHelper {
    pub fn stubname_from_loop<R: LibraryRuntime, L: LoopSource>(runtime: &R, loop_: &L) -> PathBuf {
        runtime
            .library_path()
            .join(format!("{}-{}", OUTPUT_LOOP_NAME, loop_.save_hash_text()))
    }

    pub fn next_available_stream_out_filename<R: LibraryRuntime>(
        runtime: &R,
        stream_num: &mut i32,
        display_name: &mut String,
    ) -> PathBuf {
        loop {
            let timing = runtime.library_path().join(format!(
                "{}{}{}",
                OUTPUT_STREAM_NAME, *stream_num, OUTPUT_TIMING_EXT
            ));
            if !timing.exists() {
                *display_name = format!("{}{}", OUTPUT_STREAM_NAME, *stream_num);
                return runtime.library_path().join(display_name.as_str());
            }
            *stream_num = stream_num.saturating_add(1);
        }
    }

    pub fn loop_filename_from_stub<R: LibraryRuntime>(runtime: &R, stub: &Path) -> LibraryFileInfo {
        find_file_extensions(stub, runtime.audio_extensions())
    }

    pub fn data_filename_from_stub(stub: &Path) -> LibraryFileInfo {
        find_file_extensions(stub, &[(OUTPUT_DATA_EXT, Codec::Unknown)])
    }
}

fn find_file_extensions(stub: &Path, exts: &[(&str, Codec)]) -> LibraryFileInfo {
    for &(ext, codec) in exts {
        let exact = PathBuf::from(format!("{}{}", stub.display(), ext));
        if exact.is_file() {
            return LibraryFileInfo {
                exists: true,
                codec,
                name: Some(exact),
            };
        }
    }
    let parent = stub.parent().unwrap_or_else(|| Path::new("."));
    let prefix = stub
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    // C++ tries every extension separately and `glob(3)` supplies its
    // matches in lexical order.  `read_dir` has no ordering guarantee, so it
    // must not decide which codec/name wins when a legacy wildcard load has
    // more than one candidate.
    for &(ext, codec) in exts {
        let mut candidates: Vec<_> = fs::read_dir(parent)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(prefix) && name.ends_with(ext))
            })
            .collect();
        candidates.sort();
        if let Some(path) = candidates.into_iter().next() {
            return LibraryFileInfo {
                exists: true,
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
    pub fn insert(&mut self, item: LoopTrayItem) {
        self.items.push(item);
        self.items.sort_by_key(|i| i.loop_id);
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
        exts: Vec<(&'static str, Codec)>,
    }
    impl LibraryRuntime for R {
        fn library_path(&self) -> &Path {
            &self.path
        }
        fn audio_extensions(&self) -> &[(&str, Codec)] {
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
            exts: vec![(".wav", Codec::Wav)],
        };
        assert_eq!(
            LibraryHelper::stubname_from_loop(&r, &L),
            d.join("loop-ABCD")
        );
        let mut n = 0;
        let mut display = String::new();
        assert_eq!(
            LibraryHelper::next_available_stream_out_filename(&r, &mut n, &mut display),
            d.join("live0")
        );
        fs::write(d.join("loop-ABCD.wav"), b"x").unwrap();
        assert!(LibraryHelper::loop_filename_from_stub(&r, &d.join("loop-ABCD")).exists);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn wildcard_resolution_uses_cpp_codec_priority_then_glob_order() {
        let d = std::env::temp_dir().join(format!("fw-loop-library-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let r = R {
            path: d.clone(),
            exts: vec![(".wav", Codec::Wav), (".ogg", Codec::Vorbis)],
        };
        // There is no exact `loop-HASH.wav`; both are wildcard candidates.
        // C++ tries WAV before Vorbis and glob sorts `-a` before `-z`.
        fs::write(d.join("loop-HASH-z.wav"), b"x").unwrap();
        fs::write(d.join("loop-HASH-a.wav"), b"x").unwrap();
        fs::write(d.join("loop-HASH-0.ogg"), b"x").unwrap();
        let found = LibraryHelper::loop_filename_from_stub(&r, &d.join("loop-HASH"));
        assert_eq!(found.codec, Codec::Wav);
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
