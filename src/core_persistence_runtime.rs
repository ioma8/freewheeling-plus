//! Runtime persistence orchestration.
//!
//! This layer deliberately owns the boundaries which are normally supplied by
//! the application: files, browsers, and the event queues.  It is therefore
//! usable by the real runtime as well as by deterministic adapters in tests.

use crate::core_persistence::{
    AudioLoopSource, LoopSource, Scene, encode_hash, loop_metadata_xml, saveable_path,
    saveable_stub, scene_xml, split_filename,
};
use crate::core_persistence_parse::{SceneLoad, parse_loop_metadata_xml, parse_scene_xml};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub trait PersistenceFileSystem {
    fn entries(&self, directory: &Path) -> io::Result<Vec<PersistenceFile>>;
    fn exists(&self, path: &Path) -> io::Result<bool>;
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn write_new(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove(&self, _path: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "filesystem does not support removal",
        ))
    }
}

#[derive(Debug, Clone)]
pub struct PersistenceFile {
    pub path: PathBuf,
    pub modified: Option<std::time::SystemTime>,
    pub is_file: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct OsPersistenceFileSystem;
impl PersistenceFileSystem for OsPersistenceFileSystem {
    fn entries(&self, directory: &Path) -> io::Result<Vec<PersistenceFile>> {
        fs::read_dir(directory)?
            .map(|entry| {
                let entry = entry?;
                let metadata = entry.metadata()?;
                Ok(PersistenceFile {
                    path: entry.path(),
                    modified: metadata.modified().ok(),
                    is_file: metadata.is_file(),
                })
            })
            .collect()
    }
    fn exists(&self, p: &Path) -> io::Result<bool> {
        // `Path::exists` reports `false` for every failure, which would make a
        // permission or I/O error look like a missing file.
        match fs::metadata(p) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
    fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
        fs::read(p)
    }
    fn write_new(&self, p: &Path, b: &[u8]) -> io::Result<()> {
        use std::fs::OpenOptions;
        use std::io::Write;
        let mut f = OpenOptions::new().write(true).create_new(true).open(p)?;
        f.write_all(b)
    }
    fn rename(&self, a: &Path, b: &Path) -> io::Result<()> {
        fs::rename(a, b)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

pub trait PersistenceBrowser {
    fn clear(&mut self);
    fn add(&mut self, path: PathBuf, modified: Option<std::time::SystemTime>, default_name: bool);
    fn divisions(&mut self);
}

pub trait PersistenceEvents {
    type Event;
    fn queue_save(&mut self, index: i32);
    fn queue_load(&mut self, filename: String, index: i32, volume: f32);
    fn queue_scene_load(&mut self, scene: SceneLoad);
    fn emit(&mut self, event: Self::Event);
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadRequest {
    pub filename: String,
    pub index: i32,
    pub volume: f32,
}

pub struct PersistenceRuntime<F, E> {
    pub filesystem: F,
    pub events: E,
}

impl<F: PersistenceFileSystem, E: PersistenceEvents> PersistenceRuntime<F, E> {
    pub fn new(filesystem: F, events: E) -> Self {
        Self { filesystem, events }
    }

    /// Hand a save request to the event sink.
    pub fn queue_save(&mut self, index: i32) {
        self.events.queue_save(index);
    }
    pub fn queue_load(&mut self, filename: impl Into<String>, index: i32, volume: f32) {
        let request = LoadRequest {
            filename: filename.into(),
            index,
            volume,
        };
        self.events.queue_load(request.filename, index, volume);
    }

    pub fn scan_browser<B: PersistenceBrowser>(
        &self,
        browser: &mut B,
        directory: &Path,
        prefix: &str,
        extension: &str,
    ) -> Result<(), String> {
        // Read first: a failed scan must not leave the caller's browser empty.
        let mut entries = self
            .filesystem
            .entries(directory)
            .map_err(|e| e.to_string())?;
        browser.clear();
        entries.retain(|entry| {
            entry.is_file
                && entry
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(prefix) && n.ends_with(extension))
                    .unwrap_or(false)
        });
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        for entry in entries {
            let default_name = entry
                .path
                .file_stem()
                .and_then(|n| n.to_str())
                .map(|n| !n.contains('-'))
                .unwrap_or(true);
            browser.add(entry.path, entry.modified, default_name);
        }
        browser.divisions();
        Ok(())
    }

    /// Remove every path a failed save may have created.
    ///
    /// Returns one message per removal that failed for a reason other than the
    /// file already being absent.
    fn rollback_files(&self, paths: &[&Path]) -> Vec<String> {
        let mut failures = Vec::new();
        for path in paths {
            if let Err(error) = self.filesystem.remove(path)
                && error.kind() != io::ErrorKind::NotFound
            {
                failures.push(format!("{}: {error}", path.display()));
            }
        }
        failures
    }

    pub fn save_loop<S: LoopSource>(
        &self,
        source: &mut S,
        library: &str,
        audio_ext: &str,
    ) -> Result<(PathBuf, PathBuf), String> {
        if source.save_hash().is_some() {
            return Err("loop marked already saved".into());
        }
        let hash = crate::core_persistence::md5_audio(source.audio_bytes());
        let text = encode_hash(&hash);
        let name = source.object_name();
        let audio = PathBuf::from(saveable_path(library, "loop", &text, name, Some(audio_ext)));
        let data = PathBuf::from(saveable_path(library, "loop", &text, name, Some(".xml")));
        if self.filesystem.exists(&audio).map_err(|e| e.to_string())? {
            return Err("MD5 collision while saving loop- file exists!".into());
        }
        if let Err(error) = self.filesystem.write_new(&audio, source.audio_bytes()) {
            // A failed `write_all` leaves a truncated file behind; an
            // already-existing file belongs to someone else and stays.
            let mut failures = Vec::new();
            if error.kind() != io::ErrorKind::AlreadyExists {
                failures = self.rollback_files(&[audio.as_path()]);
            }
            let suffix = if failures.is_empty() {
                String::new()
            } else {
                format!(
                    "; additionally could not roll back: {}",
                    failures.join(", ")
                )
            };
            return Err(format!(
                "couldn't create loop audio '{}': {error}{suffix}",
                audio.display()
            ));
        }
        if let Err(error) = self.filesystem.write_new(
            &data,
            loop_metadata_xml(source.nbeats(), source.pulse_length()).as_bytes(),
        ) {
            // Both the audio and the partially written metadata are removed so
            // the next attempt does not hit a leftover file.
            let failures = self.rollback_files(&[data.as_path(), audio.as_path()]);
            let suffix = if failures.is_empty() {
                String::new()
            } else {
                format!(
                    "; additionally could not roll back: {}",
                    failures.join(", ")
                )
            };
            return Err(format!(
                "couldn't create loop metadata '{}': {error}{suffix}",
                data.display()
            ));
        }
        source.set_save_hash(hash);
        Ok((audio, data))
    }

    /// Rename every persisted representation of a saveable as one operation.
    /// Existing destinations are rejected and completed moves are rolled back
    /// if a later companion file cannot be moved.
    pub fn rename_saveable(
        &self,
        stub: &Path,
        base_len: usize,
        new_name: Option<&str>,
        extensions: &[&str],
    ) -> Result<PathBuf, String> {
        // The name is one filename component: `saveable_stub` sanitizes a
        // separator as a fallback, but a rename is a user-facing action, so
        // such a name is rejected visibly instead of silently rewritten.
        if let Some(name) = new_name
            && !crate::core_persistence::is_saveable_name(name)
        {
            return Err(format!(
                "rename name must not contain a path separator: '{name}'"
            ));
        }
        let stub_text = stub
            .to_str()
            .ok_or_else(|| format!("filename is not UTF-8: {}", stub.display()))?;
        let (base, hash, _) = split_filename(stub_text, base_len)?;
        let renamed = PathBuf::from(saveable_stub(&base, &hash, new_name, None));
        let mut pairs = Vec::new();
        for extension in extensions {
            let from = PathBuf::from(format!("{}{extension}", stub.display()));
            let to = PathBuf::from(format!("{}{extension}", renamed.display()));
            // A failed lookup (permission, I/O) is reported: treating it as
            // "missing" would silently skip a companion file.
            if self
                .filesystem
                .exists(&from)
                .map_err(|error| error.to_string())?
            {
                pairs.push((from, to));
            }
        }
        if pairs.is_empty() {
            return Err(format!("no persisted files found for '{}'", stub.display()));
        }
        for (_, to) in &pairs {
            if self.filesystem.exists(to).map_err(|e| e.to_string())? {
                return Err(format!(
                    "rename destination already exists: {}",
                    to.display()
                ));
            }
        }
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (from, to) in &pairs {
            // `rename` replaces an existing destination on Unix, so the
            // destination is re-checked immediately before each move. A window
            // remains in which another process can create the file, which is
            // why the check above runs before anything is moved.
            if self
                .filesystem
                .exists(to)
                .map_err(|error| error.to_string())?
            {
                return Err(format!(
                    "rename destination already exists: {}",
                    to.display()
                ));
            }
            if let Err(error) = self.filesystem.rename(from, to) {
                let mut rollback_errors = Vec::new();
                for (old, new) in moved.iter().rev() {
                    if let Err(rollback) = self.filesystem.rename(new, old) {
                        rollback_errors.push(rollback.to_string());
                    }
                }
                let suffix = if rollback_errors.is_empty() {
                    String::new()
                } else {
                    format!("; rollback failed: {}", rollback_errors.join(", "))
                };
                return Err(format!(
                    "could not rename '{}' to '{}': {error}{suffix}",
                    from.display(),
                    to.display()
                ));
            }
            moved.push((from.clone(), to.clone()));
        }
        Ok(renamed)
    }

    pub fn save_scene(&self, path: &Path, scene: &Scene) -> Result<(), String> {
        self.filesystem
            .write_new(path, scene_xml(scene).as_bytes())
            .map_err(|error| format!("could not save scene '{}': {error}", path.display()))
    }

    pub fn load_loop_metadata(
        &self,
        data: &Path,
    ) -> Result<crate::core_persistence_parse::LoopMetadata, String> {
        let bytes = self.filesystem.read(data).map_err(|e| e.to_string())?;
        parse_loop_metadata_xml(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
    }

    pub fn load_scene(&mut self, data: &Path, default_loop_id: i32) -> Result<(), String> {
        self.load_scene_from_library(data, default_loop_id, None)
    }

    pub fn load_scene_from_library(
        &mut self,
        data: &Path,
        default_loop_id: i32,
        library: Option<&Path>,
    ) -> Result<(), String> {
        let bytes = self.filesystem.read(data).map_err(|e| e.to_string())?;
        let scene = parse_scene_xml(
            std::str::from_utf8(&bytes).map_err(|e| e.to_string())?,
            default_loop_id,
        )?;
        if let Some(library) = library {
            for item in scene.loops() {
                let filename = library
                    .join(saveable_stub("loop", &item.hash, None, None))
                    .to_string_lossy()
                    .into_owned();
                self.queue_load(filename, item.loop_id, item.volume);
            }
        }
        self.events.queue_scene_load(scene);
        Ok(())
    }
}

impl<E: PersistenceEvents> PersistenceRuntime<OsPersistenceFileSystem, E> {
    /// Production loop save: writes a genuine WAV/Vorbis/FLAC/AU stream in
    /// bounded chunks and commits the companion C++-compatible XML metadata.
    pub fn save_loop_encoded<S: AudioLoopSource>(
        &self,
        source: &mut S,
        library: &Path,
        format: crate::block::Codec,
    ) -> Result<(PathBuf, PathBuf), String> {
        if source.save_hash().is_some() {
            return Err("loop marked already saved".into());
        }
        fs::create_dir_all(library).map_err(|error| error.to_string())?;
        let hash = crate::core_persistence::md5_audio(source.audio_bytes());
        let text = encode_hash(&hash);
        let extension = match format {
            crate::block::Codec::Wav => ".wav",
            crate::block::Codec::Vorbis => ".ogg",
            crate::block::Codec::Flac => ".flac",
            crate::block::Codec::Au => ".au",
            _ => return Err("unsupported loop output codec".into()),
        };
        let audio = library.join(saveable_stub(
            "loop",
            &text,
            source.object_name(),
            Some(extension),
        ));
        let data = library.join(saveable_stub(
            "loop",
            &text,
            source.object_name(),
            Some(".xml"),
        ));
        // Same MD5-collision guard as `save_loop`: without it the encoder
        // would overwrite another loop's audio before the metadata write
        // rejects the duplicate.
        for existing in [&audio, &data] {
            if self
                .filesystem
                .exists(existing)
                .map_err(|error| error.to_string())?
            {
                return Err(format!(
                    "MD5 collision while saving loop: '{}' already exists",
                    existing.display()
                ));
            }
        }
        crate::file_codecs::encode_audio_file(
            &audio,
            source.sample_rate(),
            format,
            source.left_samples(),
            source.right_samples(),
        )
        .map_err(|error| format!("could not save loop audio '{}': {error}", audio.display()))?;
        if let Err(error) = self.filesystem.write_new(
            &data,
            loop_metadata_xml(source.nbeats(), source.pulse_length()).as_bytes(),
        ) {
            // The encoder already wrote the audio; both files are removed and
            // a failed rollback is reported instead of swallowed.
            let failures = self.rollback_files(&[data.as_path(), audio.as_path()]);
            let suffix = if failures.is_empty() {
                String::new()
            } else {
                format!(
                    "; additionally could not roll back: {}",
                    failures.join(", ")
                )
            };
            return Err(format!(
                "could not save loop metadata '{}': {error}{suffix}",
                data.display()
            ));
        }
        source.set_save_hash(hash);
        Ok((audio, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_persistence::{LoopSource, Saveable};
    struct Mem {
        files: std::cell::RefCell<std::collections::HashMap<PathBuf, Vec<u8>>>,
    }
    impl PersistenceFileSystem for Mem {
        fn entries(&self, _: &Path) -> io::Result<Vec<PersistenceFile>> {
            Ok(Vec::new())
        }
        fn exists(&self, p: &Path) -> io::Result<bool> {
            Ok(self.files.borrow().contains_key(p))
        }
        fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
            self.files
                .borrow()
                .get(p)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
        fn write_new(&self, p: &Path, b: &[u8]) -> io::Result<()> {
            if self.exists(p)? {
                return Err(io::Error::from(io::ErrorKind::AlreadyExists));
            };
            self.files.borrow_mut().insert(p.into(), b.into());
            Ok(())
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            if self.files.borrow().contains_key(to) {
                return Err(io::Error::from(io::ErrorKind::AlreadyExists));
            }
            let bytes = self
                .files
                .borrow_mut()
                .remove(from)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            self.files.borrow_mut().insert(to.into(), bytes);
            Ok(())
        }
        fn remove(&self, path: &Path) -> io::Result<()> {
            self.files
                .borrow_mut()
                .remove(path)
                .map(|_| ())
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }
    struct Ev;
    impl PersistenceEvents for Ev {
        type Event = ();
        fn queue_save(&mut self, _: i32) {}
        fn queue_load(&mut self, _: String, _: i32, _: f32) {}
        fn queue_scene_load(&mut self, _: SceneLoad) {}
        fn emit(&mut self, _: ()) {}
    }
    struct L {
        hash: Option<[u8; 16]>,
    }
    impl Saveable for L {
        fn save_hash(&self) -> Option<[u8; 16]> {
            self.hash
        }
        fn set_save_hash(&mut self, h: [u8; 16]) {
            self.hash = Some(h)
        }
    }
    impl LoopSource for L {
        fn audio_bytes(&self) -> &[u8] {
            b"abc"
        }
        fn object_name(&self) -> Option<&str> {
            Some("take")
        }
        fn nbeats(&self) -> i64 {
            4
        }
        fn pulse_length(&self) -> u32 {
            12
        }
    }
    /// Memory filesystem that fails the operations named in `failing`.
    struct FailingMem {
        files: std::cell::RefCell<std::collections::HashMap<PathBuf, Vec<u8>>>,
        failing: std::collections::HashSet<std::io::ErrorKind>,
        written: std::cell::RefCell<Vec<PathBuf>>,
    }
    impl PersistenceFileSystem for FailingMem {
        fn entries(&self, _: &Path) -> io::Result<Vec<PersistenceFile>> {
            if self.failing.contains(&io::ErrorKind::PermissionDenied) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            Ok(Vec::new())
        }
        fn exists(&self, p: &Path) -> io::Result<bool> {
            if self.failing.contains(&io::ErrorKind::InvalidInput) {
                return Err(io::Error::from(io::ErrorKind::InvalidInput));
            }
            Ok(self.files.borrow().contains_key(p))
        }
        fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
            self.files
                .borrow()
                .get(p)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
        fn write_new(&self, p: &Path, b: &[u8]) -> io::Result<()> {
            if self.failing.contains(&io::ErrorKind::WriteZero) {
                // Simulate a partial write: the file exists but is truncated.
                self.files.borrow_mut().insert(p.into(), Vec::new());
                self.written.borrow_mut().push(p.into());
                return Err(io::Error::from(io::ErrorKind::WriteZero));
            }
            if self.exists(p)? {
                return Err(io::Error::from(io::ErrorKind::AlreadyExists));
            }
            self.files.borrow_mut().insert(p.into(), b.into());
            self.written.borrow_mut().push(p.into());
            Ok(())
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            let bytes = self
                .files
                .borrow_mut()
                .remove(from)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            self.files.borrow_mut().insert(to.into(), bytes);
            Ok(())
        }
        fn remove(&self, path: &Path) -> io::Result<()> {
            self.files
                .borrow_mut()
                .remove(path)
                .map(|_| ())
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    #[test]
    fn a_failed_scan_keeps_the_browser_contents() {
        let runtime = PersistenceRuntime::new(
            FailingMem {
                files: Default::default(),
                failing: [io::ErrorKind::PermissionDenied].into_iter().collect(),
                written: Default::default(),
            },
            Ev,
        );
        let mut browser = RecordingBrowser {
            cleared: 0,
            added: 1,
        };
        assert!(
            runtime
                .scan_browser(&mut browser, Path::new("lib"), "loop-", ".wav")
                .is_err()
        );
        // The caller's browser still shows what it showed before.
        assert_eq!(browser.cleared, 0);
        assert_eq!(browser.added, 1);
    }

    struct RecordingBrowser {
        cleared: u32,
        added: u32,
    }
    impl PersistenceBrowser for RecordingBrowser {
        fn clear(&mut self) {
            self.cleared += 1;
            self.added = 0;
        }
        fn add(&mut self, _: PathBuf, _: Option<std::time::SystemTime>, _: bool) {
            self.added += 1;
        }
        fn divisions(&mut self) {}
    }

    #[test]
    fn a_failed_lookup_is_reported_instead_of_skipping_a_companion_file() {
        let runtime = PersistenceRuntime::new(
            FailingMem {
                files: Default::default(),
                failing: [io::ErrorKind::InvalidInput].into_iter().collect(),
                written: Default::default(),
            },
            Ev,
        );
        let error = runtime
            .rename_saveable(
                Path::new("lib/loop-ABC"),
                8,
                Some("take"),
                &[".wav", ".xml"],
            )
            .unwrap_err();
        assert!(
            error.contains("InvalidInput") || error.contains("invalid"),
            "{error}"
        );
    }

    #[test]
    fn a_failed_audio_write_leaves_no_truncated_file() {
        let runtime = PersistenceRuntime::new(
            FailingMem {
                files: Default::default(),
                failing: [io::ErrorKind::WriteZero].into_iter().collect(),
                written: Default::default(),
            },
            Ev,
        );
        let mut loop_source = L { hash: None };
        let error = runtime
            .save_loop(&mut loop_source, "lib", ".wav")
            .unwrap_err();
        assert!(error.contains("couldn't create loop audio"), "{error}");
        assert!(runtime.filesystem.files.borrow().is_empty());
        assert!(loop_source.hash.is_none());
    }

    #[test]
    fn save_and_load_are_real_operations() {
        let r = PersistenceRuntime::new(
            Mem {
                files: Default::default(),
            },
            Ev,
        );
        let mut l = L { hash: None };
        let (_, d) = r.save_loop(&mut l, "lib", ".wav").unwrap();
        assert_eq!(r.load_loop_metadata(&d).unwrap().nbeats, Some(4));
        assert!(l.hash.is_some());
    }
}
