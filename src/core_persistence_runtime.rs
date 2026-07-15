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
use std::collections::VecDeque;
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
        Ok(p.exists())
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
    saves: VecDeque<i32>,
    loads: VecDeque<LoadRequest>,
}

impl<F: PersistenceFileSystem, E: PersistenceEvents> PersistenceRuntime<F, E> {
    pub fn new(filesystem: F, events: E) -> Self {
        Self {
            filesystem,
            events,
            saves: VecDeque::new(),
            loads: VecDeque::new(),
        }
    }

    pub fn queue_save(&mut self, index: i32) {
        self.saves.push_back(index);
        self.events.queue_save(index);
    }
    pub fn queue_load(&mut self, filename: impl Into<String>, index: i32, volume: f32) {
        let request = LoadRequest {
            filename: filename.into(),
            index,
            volume,
        };
        self.loads.push_back(request.clone());
        self.events.queue_load(request.filename, index, volume);
    }

    pub fn scan_browser<B: PersistenceBrowser>(
        &self,
        browser: &mut B,
        directory: &Path,
        prefix: &str,
        extension: &str,
    ) -> Result<(), String> {
        browser.clear();
        let mut entries = self
            .filesystem
            .entries(directory)
            .map_err(|e| e.to_string())?;
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
        self.filesystem
            .write_new(&audio, source.audio_bytes())
            .map_err(|e| format!("Couldn't open file: {e}"))?;
        self.filesystem
            .write_new(
                &data,
                loop_metadata_xml(source.nbeats(), source.pulse_length()).as_bytes(),
            )
            .map_err(|error| {
                let rollback = self.filesystem.remove(&audio);
                match rollback {
                    Ok(()) => error.to_string(),
                    Err(rollback) => format!(
                        "{error}; additionally could not roll back '{}': {rollback}",
                        audio.display()
                    ),
                }
            })?;
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
        let stub_text = stub
            .to_str()
            .ok_or_else(|| format!("filename is not UTF-8: {}", stub.display()))?;
        let (base, hash, _) = split_filename(stub_text, base_len)?;
        let renamed = PathBuf::from(saveable_stub(&base, &hash, new_name, None));
        let pairs: Vec<_> = extensions
            .iter()
            .map(|extension| {
                (
                    PathBuf::from(format!("{}{extension}", stub.display())),
                    PathBuf::from(format!("{}{extension}", renamed.display())),
                )
            })
            .filter(|(from, _)| self.filesystem.exists(from).unwrap_or(false))
            .collect();
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
            for item in &scene.loops {
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
            let _ = self.filesystem.remove(&audio);
            return Err(format!(
                "could not save loop metadata '{}': {error}",
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
