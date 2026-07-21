/*
   Browser module for navigating loop/scene libraries.
   Ported from fweelin_browser.h/cc
*/

use crate::event::Event;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Filesystem boundary used by library scanning.  Applications can provide a
/// virtual filesystem (and tests do not need to touch the host filesystem).
pub trait BrowserFileSystem: Send + Sync {
    fn entries(&self, directory: &Path) -> io::Result<Vec<BrowserFile>>;
}

#[derive(Debug, Clone)]
pub struct BrowserFile {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
    pub is_file: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct OsBrowserFileSystem;

impl BrowserFileSystem for OsBrowserFileSystem {
    fn entries(&self, directory: &Path) -> io::Result<Vec<BrowserFile>> {
        fs::read_dir(directory)?
            .map(|entry| {
                let entry = entry?;
                let metadata = entry.metadata()?;
                Ok(BrowserFile {
                    path: entry.path(),
                    modified: metadata.modified().ok(),
                    is_file: metadata.is_file(),
                })
            })
            .collect()
    }
}

/// Runtime boundary for time-dependent browser behavior.
pub trait BrowserRuntime: Send + Sync {
    fn now(&self) -> SystemTime;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemBrowserRuntime;

impl BrowserRuntime for SystemBrowserRuntime {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Thread-safe owner for a browser.  The C++ implementation protects list
/// mutation with a mutex; this wrapper provides the same boundary for Rust
/// callers crossing worker/UI threads.
pub type SharedBrowser = Arc<Mutex<Browser>>;

/// Browser item types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BrowserItemType {
    Undefined,
    Loop,
    Scene,
    LoopTray,
    SceneTray,
    Patch,
    Division,
    Last,
}

/// Browser item
#[derive(Debug, Clone)]
pub struct BrowserItem {
    pub name: String,
    pub default_name: bool,
    pub item_type: BrowserItemType,
    pub match_id: Option<i32>,
    pub filename: Option<PathBuf>,
    pub modified: Option<SystemTime>,
}

/// Separator inserted between groups of browser entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserDivision;

impl BrowserDivision {
    pub fn item() -> BrowserItem {
        BrowserItem::new("", true, BrowserItemType::Division)
    }
}

/// State shared with the rename widget renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenameUIVars {
    pub rename_cursor_blinktime: f64,
    pub rename_cursor_toggle: bool,
}

impl Default for RenameUIVars {
    fn default() -> Self {
        Self {
            rename_cursor_blinktime: 0.0,
            rename_cursor_toggle: false,
        }
    }
}

/// Bounded, event-oriented item renamer matching the legacy key handling.
pub struct ItemRenamer {
    name: String,
    pub ui: RenameUIVars,
    pub renaming: bool,
}

impl ItemRenamer {
    pub const RENAME_BUF_SIZE: usize = 512;
    pub const BLINK_DELAY: f64 = 0.5;

    pub fn new(old_name: Option<&str>) -> Self {
        // C++ copies at most `RENAME_BUF_SIZE - 1` raw bytes.  The Rust UI
        // boundary is UTF-8, so retain the longest valid UTF-8 prefix instead
        // of calling `String::truncate` at an arbitrary byte offset (which
        // panics when the boundary falls in a multi-byte scalar).
        let old_name = old_name.unwrap_or_default();
        let limit = Self::RENAME_BUF_SIZE - 1;
        let end = old_name
            .char_indices()
            .map(|(idx, _)| idx)
            .chain(std::iter::once(old_name.len()))
            .take_while(|&idx| idx <= limit)
            .last()
            .unwrap_or(0);
        let name = old_name[..end].to_owned();
        Self {
            name,
            ui: RenameUIVars::default(),
            renaming: true,
        }
    }
    pub fn current_name(&self) -> &str {
        &self.name
    }
    pub fn update_ui_vars(&mut self, now: f64) -> RenameUIVars {
        if now - self.ui.rename_cursor_blinktime >= Self::BLINK_DELAY {
            self.ui.rename_cursor_blinktime = now;
            self.ui.rename_cursor_toggle = !self.ui.rename_cursor_toggle;
        }
        self.ui
    }
    pub fn append(&mut self, c: char) {
        if self.name.len() + c.len_utf8() < Self::RENAME_BUF_SIZE {
            self.name.push(c);
        }
    }
    pub fn backspace(&mut self) {
        self.name.pop();
    }
    pub fn finish(&mut self, accept: bool) -> Option<String> {
        self.renaming = false;
        accept.then(|| self.name.clone())
    }
}

impl BrowserItem {
    pub fn new(name: &str, default_name: bool, item_type: BrowserItemType) -> Self {
        BrowserItem {
            name: name.to_string(),
            default_name,
            item_type,
            match_id: None,
            filename: None,
            modified: None,
        }
    }

    pub fn with_match_id(mut self, match_id: i32) -> Self {
        self.match_id = Some(match_id);
        self
    }
}

/// Browser for loops/scenes on disk
pub struct Browser {
    pub browser_id: i32,
    pub items: Vec<BrowserItem>,
    pub path: PathBuf,
    pub item_type: BrowserItemType,
    pub current_index: Option<usize>,
    pub selected_index: Option<usize>,
    pub renaming: bool,
    pub last_browsed: bool,
}

impl Browser {
    pub fn new(path: &str, item_type: BrowserItemType) -> Self {
        Self::with_id(0, path, item_type)
    }

    pub fn with_id(browser_id: i32, path: &str, item_type: BrowserItemType) -> Self {
        Browser {
            browser_id,
            items: Vec::new(),
            path: PathBuf::from(path),
            item_type,
            current_index: None,
            selected_index: None,
            renaming: false,
            last_browsed: false,
        }
    }

    /// Scan directory for browser items
    pub fn scan(&mut self) -> Result<(), String> {
        self.scan_with(&OsBrowserFileSystem)
    }

    pub fn scan_with(&mut self, filesystem: &dyn BrowserFileSystem) -> Result<(), String> {
        self.items.clear();

        let entries = filesystem.entries(&self.path).map_err(|e| {
            format!(
                "Failed to read browser path '{}': {}",
                self.path.display(),
                e
            )
        })?;

        for entry in entries {
            let path = entry.path;
            if !entry.is_file || !self.matches_item_type(&path) {
                continue;
            }

            let modified = entry.modified;
            let display = self.display_name_for_path(&path, modified);

            let mut item =
                BrowserItem::new(&display.name, display.used_default_name, self.item_type);
            item.filename = Some(path);
            item.modified = modified;
            self.items.push(item);
        }

        // LoopBrowserItem/SceneBrowserItem::Compare sorts newest first.  It
        // deliberately does not use the display name as a tie breaker: C++
        // retains the insertion (glob) order for files with equal mtime.
        self.items.sort_by(Self::cpp_item_compare);
        self.current_index = self.first_non_division_index();
        self.selected_index = None;
        self.renaming = false;
        self.last_browsed = self.current_index.is_some();
        Ok(())
    }

    pub fn add_item(&mut self, item: BrowserItem, sort: bool) {
        let position = if sort {
            self.items
                .iter()
                .position(|candidate| Self::cpp_item_compare(&item, candidate).is_lt())
                .unwrap_or(self.items.len())
        } else {
            self.items.len()
        };
        self.items.insert(position, item);
        if self.current_index.is_none() {
            self.current_index = Some(position);
        }
    }

    pub fn remove_item(&mut self, match_id: i32) -> bool {
        let Some(index) = self
            .items
            .iter()
            .position(|item| item.match_id == Some(match_id))
        else {
            return false;
        };
        self.items.remove(index);
        self.current_index = self.current_index.and_then(|current| {
            if self.items.is_empty() {
                None
            } else {
                Some(current.min(self.items.len() - 1))
            }
        });
        true
    }

    pub fn add_divisions(&mut self, max_delta: i32) {
        let mut index = 0;
        while index + 1 < self.items.len() {
            let current = &self.items[index];
            let next = &self.items[index + 1];
            // `next->Compare(current) < maxdelta` means adjacent saved files
            // were created close enough in time.  A non-default name forces a
            // visual division on either side, exactly as Browser::AddDivisions.
            let close = Self::cpp_compare_delta(next, current) < i64::from(max_delta);
            let keep_together = (current.default_name && next.default_name)
                || current.item_type == BrowserItemType::Division
                || next.item_type == BrowserItemType::Division;
            if close && keep_together {
                index += 1;
                continue;
            }
            self.items.insert(
                index + 1,
                BrowserItem::new("", true, BrowserItemType::Division),
            );
            index += 2;
        }
    }

    /// Get item at index
    pub fn get_item(&self, index: usize) -> Option<&BrowserItem> {
        self.items.get(index)
    }

    /// Number of items
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn current_item(&self) -> Option<&BrowserItem> {
        self.current_index.and_then(|idx| self.items.get(idx))
    }

    pub fn selected_item(&self) -> Option<&BrowserItem> {
        self.selected_index.and_then(|idx| self.items.get(idx))
    }

    pub fn move_to_beginning(&mut self) {
        self.current_index = self.first_non_division_index();
        self.last_browsed = self.current_index.is_some();
    }

    pub fn move_to(&mut self, adjust: i32, jumpadjust: i32) {
        if self.items.is_empty() {
            self.current_index = None;
            return;
        }

        if self.current_index.is_none() {
            self.current_index = self.first_non_division_index();
        }
        let Some(mut cur) = self.current_index else {
            return;
        };

        if jumpadjust != 0 {
            let dir = if jumpadjust >= 0 { 1 } else { -1 };
            let magnitude = if dir == 1 {
                jumpadjust.unsigned_abs() as usize
            } else {
                jumpadjust.unsigned_abs() as usize + 1
            };

            for step in 0..magnitude {
                let prev = cur;
                let mut probe = Some(cur);
                while let Some(idx) = probe {
                    let next = if dir == 1 {
                        self.next_index(idx)
                    } else {
                        self.prev_index(idx)
                    };
                    probe = next;
                    match probe {
                        Some(next_idx)
                            if self.items[next_idx].item_type == BrowserItemType::Division =>
                        {
                            probe = if dir == 1 || step + 1 >= magnitude {
                                self.next_index(next_idx)
                            } else {
                                self.prev_index(next_idx)
                            };
                            break;
                        }
                        None => break,
                        _ => {}
                    }
                }

                cur = match probe {
                    Some(idx) => idx,
                    None if dir == -1 && step + 1 >= magnitude => {
                        self.first_non_division_index().unwrap_or(prev)
                    }
                    None => prev,
                };
            }
        }

        if adjust != 0 {
            let dir = if adjust >= 0 { 1 } else { -1 };
            let magnitude = adjust.unsigned_abs() as usize;
            let mut moved = 0usize;
            while moved < magnitude {
                let prev = cur;
                let next = if dir == 1 {
                    self.next_index(cur)
                } else {
                    self.prev_index(cur)
                };
                let Some(next_idx) = next else {
                    cur = prev;
                    break;
                };
                cur = next_idx;
                if self.items[cur].item_type != BrowserItemType::Division {
                    moved += 1;
                }
            }
        }

        if self.items[cur].item_type == BrowserItemType::Division {
            self.current_index = self.first_non_division_index();
        } else {
            self.current_index = Some(cur);
        }
        self.last_browsed = self.current_index.is_some();
    }

    pub fn select_current(&mut self) -> Option<&BrowserItem> {
        self.selected_index = self.current_index;
        self.selected_item()
    }

    pub fn begin_rename(&mut self) -> bool {
        let Some(item) = self.current_item() else {
            return false;
        };
        self.renaming = matches!(
            item.item_type,
            BrowserItemType::Loop
                | BrowserItemType::Scene
                | BrowserItemType::LoopTray
                | BrowserItemType::SceneTray
        );
        self.renaming
    }

    pub fn commit_rename(&mut self, new_name: Option<&str>) -> bool {
        let Some(idx) = self.current_index else {
            return false;
        };
        if !self.renaming {
            return false;
        }
        let display_fallback = self
            .items
            .get(idx)
            .and_then(|item| {
                item.filename
                    .as_ref()
                    .map(|path| (path.clone(), item.modified))
            })
            .map(|(path, modified)| self.display_name_for_path(&path, modified));

        let Some(item) = self.items.get_mut(idx) else {
            return false;
        };

        match new_name {
            Some(name) => {
                item.name = name.to_string();
                if item.name.is_empty() {
                    if let Some(display) = display_fallback {
                        item.name = display.name;
                        item.default_name = true;
                    } else {
                        item.default_name = true;
                    }
                } else {
                    item.default_name = false;
                }
                self.renaming = false;
                true
            }
            None => {
                self.renaming = false;
                true
            }
        }
    }

    pub fn item_renamed_on_disk(
        &mut self,
        old_filename: &Path,
        new_filename: &Path,
        new_name: &str,
    ) -> bool {
        let modified = self
            .items
            .iter()
            .find(|item| {
                item.filename
                    .as_ref()
                    .map(|path| path == old_filename)
                    .unwrap_or(false)
            })
            .and_then(|item| item.modified);
        let display_fallback = self.display_name_for_path(new_filename, modified);

        let Some(item) = self.items.iter_mut().find(|item| {
            item.filename
                .as_ref()
                .map(|path| path == old_filename)
                .unwrap_or(false)
        }) else {
            return false;
        };

        item.filename = Some(new_filename.to_path_buf());
        item.name = new_name.to_string();
        if item.name.is_empty() {
            item.name = display_fallback.name;
            item.default_name = true;
        } else {
            item.default_name = false;
        }
        true
    }

    pub fn item_browsed(&mut self) -> bool {
        self.last_browsed = self.current_index.is_some();
        self.last_browsed
    }

    pub fn move_patch_bank(&mut self, direction: i32) -> bool {
        if self.item_type != BrowserItemType::Patch || self.items.is_empty() {
            return false;
        }
        if self.current_index.is_none() {
            self.current_index = self.first_non_division_index();
        }
        let Some(cur) = self.current_index else {
            return false;
        };

        let step_forward = direction >= 0;
        let mut probe = Some(cur);
        while let Some(idx) = probe {
            probe = if step_forward {
                self.next_index(idx)
            } else {
                self.prev_index(idx)
            };
            let Some(next_idx) = probe else { break };
            if self.items[next_idx].item_type == BrowserItemType::Division {
                probe = if step_forward {
                    self.next_index(next_idx)
                } else {
                    self.prev_index(next_idx)
                };
                break;
            }
        }

        if let Some(idx) = probe {
            self.current_index = Some(idx);
            self.last_browsed = true;
            true
        } else {
            false
        }
    }

    pub fn move_patch_bank_to_index(&mut self, target: i32) -> bool {
        if self.item_type != BrowserItemType::Patch || self.items.is_empty() {
            return false;
        }
        let Some(target_bank) = usize::try_from(target).ok() else {
            return false;
        };
        let mut bank_index = 0usize;
        let mut first_patch_in_bank = None;
        for (idx, item) in self.items.iter().enumerate() {
            if item.item_type == BrowserItemType::Division {
                bank_index += 1;
                first_patch_in_bank = None;
                continue;
            }
            if first_patch_in_bank.is_none() {
                first_patch_in_bank = Some(idx);
                if bank_index == target_bank {
                    self.current_index = Some(idx);
                    self.last_browsed = true;
                    return true;
                }
            }
        }
        false
    }

    pub fn receive_event(&mut self, ev: &Event) -> bool {
        match ev {
            Event::BrowserMoveToItem { browserid, adjust, jump_adjust } => {
                if *browserid != self.browser_id {
                    return false;
                }
                self.move_to(*adjust, *jump_adjust);
                true
            }
            Event::BrowserMoveToItemAbsolute { browserid, index } => {
                if *browserid != self.browser_id {
                    return false;
                }
                self.move_to_beginning();
                self.move_to(*index, 0);
                true
            }
            Event::BrowserSelectItem { browserid } => {
                if *browserid != self.browser_id {
                    return false;
                }
                self.select_current();
                true
            }
            Event::BrowserRenameItem { browserid } => {
                if *browserid != self.browser_id {
                    return false;
                }
                self.begin_rename();
                true
            }
            Event::RenameLoop { loopid, in_layout } => {
                if self.item_type != BrowserItemType::LoopTray || *in_layout {
                    return false;
                }
                if let Some(idx) = self
                    .items
                    .iter()
                    .position(|item| item.match_id == Some(*loopid))
                {
                    self.current_index = Some(idx);
                    self.selected_index = Some(idx);
                    self.last_browsed = true;
                    return self.begin_rename();
                }
                false
            }
            Event::PatchBrowserMoveToBank { direction } => {
                self.move_patch_bank(*direction)
            }
            Event::PatchBrowserMoveToBankByIndex { index } => {
                self.move_patch_bank_to_index(*index)
            }
            Event::BrowserItemBrowsed { browserid } => {
                if *browserid != self.browser_id {
                    return false;
                }
                self.item_browsed()
            }
            _ => false,
        }
    }

    fn matches_item_type(&self, path: &Path) -> bool {
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        match self.item_type {
            BrowserItemType::Loop => {
                let lower = filename.to_ascii_lowercase();
                lower.starts_with("loop-")
                    && [".wav", ".ogg", ".flac", ".au"]
                        .iter()
                        .any(|extension| lower.ends_with(extension))
            }
            BrowserItemType::Scene => filename.starts_with("scene-") && filename.ends_with(".xml"),
            BrowserItemType::Patch => filename.ends_with(".pat") || filename.ends_with(".sf2"),
            _ => false,
        }
    }

    fn cpp_item_compare(left: &BrowserItem, right: &BrowserItem) -> std::cmp::Ordering {
        match (left.item_type, right.item_type) {
            (BrowserItemType::Loop, BrowserItemType::Loop)
            | (BrowserItemType::Scene, BrowserItemType::Scene) => {
                right.modified.cmp(&left.modified)
            }
            _ => std::cmp::Ordering::Equal,
        }
    }

    fn cpp_compare_delta(next: &BrowserItem, current: &BrowserItem) -> i64 {
        match (
            next.item_type,
            current.item_type,
            current.modified,
            next.modified,
        ) {
            (BrowserItemType::Loop, BrowserItemType::Loop, Some(current), Some(next))
            | (BrowserItemType::Scene, BrowserItemType::Scene, Some(current), Some(next)) => {
                match current.duration_since(next) {
                    Ok(delta) => i64::try_from(delta.as_secs()).unwrap_or(i64::MAX),
                    Err(error) => -i64::try_from(error.duration().as_secs()).unwrap_or(i64::MAX),
                }
            }
            _ => 0,
        }
    }

    fn display_name_for_path(
        &self,
        path: &Path,
        modified: Option<SystemTime>,
    ) -> DisplayNameResult {
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(filename);
        let prefix = match self.item_type {
            BrowserItemType::Loop => "loop-",
            BrowserItemType::Scene => "scene-",
            BrowserItemType::Patch => "",
            _ => "",
        };

        if let Some(rest) = stem.strip_prefix(prefix) {
            let mut parts = rest.splitn(2, '-');
            let hash = parts.next().unwrap_or_default();
            let maybe_name = parts.next().unwrap_or_default();
            if !maybe_name.is_empty() {
                return DisplayNameResult {
                    name: maybe_name.to_string(),
                    used_default_name: false,
                };
            }

            let hash_short = hash.chars().take(2).collect::<String>();
            let stamp = modified
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|| "unknown-time".to_string());
            return DisplayNameResult {
                name: format!("{} {}", hash_short, stamp),
                used_default_name: true,
            };
        }

        DisplayNameResult {
            name: stem.to_string(),
            used_default_name: true,
        }
    }

    fn first_non_division_index(&self) -> Option<usize> {
        self.items
            .iter()
            .position(|item| item.item_type != BrowserItemType::Division)
    }

    fn next_index(&self, idx: usize) -> Option<usize> {
        if idx + 1 < self.items.len() {
            Some(idx + 1)
        } else {
            None
        }
    }

    fn prev_index(&self, idx: usize) -> Option<usize> {
        idx.checked_sub(1)
    }
}

pub struct DisplayNameResult {
    pub name: String,
    pub used_default_name: bool,
}

/// Callback trait for browser events
pub trait BrowserCallback {
    fn item_browsed(&mut self, item: &BrowserItem);
    fn item_selected(&mut self, item: &BrowserItem);
    fn item_renamed(&mut self, item: &BrowserItem);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeFs(Vec<BrowserFile>);
    impl BrowserFileSystem for FakeFs {
        fn entries(&self, _: &Path) -> io::Result<Vec<BrowserFile>> {
            Ok(self.0.clone())
        }
    }

    fn file(name: &str) -> BrowserFile {
        BrowserFile {
            path: PathBuf::from(name),
            modified: None,
            is_file: true,
        }
    }

    fn dated_file(name: &str, seconds: u64) -> BrowserFile {
        BrowserFile {
            path: PathBuf::from(name),
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)),
            is_file: true,
        }
    }

    #[test]
    fn scans_filters_and_sorts_without_host_io() {
        let mut browser = Browser::new("virtual", BrowserItemType::Loop);
        browser
            .scan_with(&FakeFs(vec![
                dated_file("loop-xx-Zulu.wav", 1),
                file("scene-nope.xml"),
                dated_file("loop-aa-Alpha-take.flac", 4),
                dated_file("loop-bb-Beta.ogg", 3),
                dated_file("loop-cc-Gamma.au", 2),
            ]))
            .unwrap();
        assert_eq!(
            browser
                .items
                .iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha-take", "Beta", "Gamma", "Zulu"]
        );
    }

    #[test]
    fn navigation_skips_divisions_and_selection_is_stable() {
        let mut browser = Browser::new(".", BrowserItemType::Loop);
        browser.add_item(BrowserItem::new("a", false, BrowserItemType::Loop), false);
        browser.add_item(
            BrowserItem::new("division", true, BrowserItemType::Division),
            false,
        );
        browser.add_item(BrowserItem::new("b", false, BrowserItemType::Loop), false);
        browser.move_to(1, 0);
        assert_eq!(browser.current_item().unwrap().name, "b");
        assert_eq!(browser.select_current().unwrap().name, "b");
    }

    #[test]
    fn rename_empty_restores_generated_display_name() {
        let mut browser = Browser::new(".", BrowserItemType::Loop);
        let mut item = BrowserItem::new("old", false, BrowserItemType::Loop);
        item.filename = Some(PathBuf::from("loop-ab-.wav"));
        browser.add_item(item, false);
        assert!(browser.begin_rename());
        assert!(browser.commit_rename(Some("")));
        assert!(browser.current_item().unwrap().default_name);
    }

    #[test]
    fn renamer_truncates_non_ascii_initial_name_on_a_utf8_boundary() {
        let name = "a".repeat(ItemRenamer::RENAME_BUF_SIZE - 2) + "é";
        let renamer = ItemRenamer::new(Some(&name));

        assert_eq!(
            renamer.current_name(),
            "a".repeat(ItemRenamer::RENAME_BUF_SIZE - 2)
        );
        assert_eq!(
            renamer.current_name().len(),
            ItemRenamer::RENAME_BUF_SIZE - 2
        );
    }

    #[test]
    fn divisions_follow_cpp_time_gaps_and_named_item_boundaries() {
        let mut browser = Browser::new(".", BrowserItemType::Loop);
        let mut newest = BrowserItem::new("AB recent", true, BrowserItemType::Loop);
        newest.modified = Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100));
        let mut adjacent = BrowserItem::new("CD adjacent", true, BrowserItemType::Loop);
        adjacent.modified = Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(95));
        let mut named = BrowserItem::new("named", false, BrowserItemType::Loop);
        named.modified = Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(94));
        browser.add_item(newest, true);
        browser.add_item(adjacent, true);
        browser.add_item(named, true);
        browser.add_divisions(10);
        assert_eq!(
            browser
                .items
                .iter()
                .map(|item| item.item_type)
                .collect::<Vec<_>>(),
            [
                BrowserItemType::Loop,
                BrowserItemType::Loop,
                BrowserItemType::Division,
                BrowserItemType::Loop,
            ]
        );
    }
}
