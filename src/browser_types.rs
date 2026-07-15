//! Concrete browser item and browser types from `fweelin_browser.h`.
//!
//! These are deliberately value types.  `Browser` owns the common navigation
//! behavior; the types here retain the state that made the C++ subclasses
//! distinct.

use crate::browser::{Browser, BrowserItem, BrowserItemType};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub struct CombiZone {
    pub key_low: i32,
    pub key_high: i32,
    pub port_redirect: bool,
    pub port: i32,
    pub bank: i32,
    pub program: i32,
    pub channel: i32,
    pub bypass_cc: bool,
    pub bypass_channel: i32,
    pub bypass_time1: f32,
    pub bypass_time2: f32,
}

#[derive(Debug, Clone)]
pub struct PatchItem {
    pub item: BrowserItem,
    pub id: i32,
    pub bank: i32,
    pub program: i32,
    pub channel: i32,
    pub bypass_cc: bool,
    pub bypass_channel: i32,
    pub bypass_time1: f32,
    pub bypass_time2: f32,
    pub zones: Vec<CombiZone>,
}

impl PatchItem {
    pub fn new(id: i32, bank: i32, program: i32, channel: i32, name: &str) -> Self {
        Self {
            item: BrowserItem::new(name, false, BrowserItemType::Patch),
            id,
            bank,
            program,
            channel,
            bypass_cc: false,
            bypass_channel: -1,
            bypass_time1: 0.0,
            bypass_time2: 10.0,
            zones: Vec::new(),
        }
    }
    pub fn setup_zones(&mut self, count: usize) {
        self.zones.resize_with(count, || CombiZone {
            key_low: 0,
            key_high: 0,
            port_redirect: false,
            port: 0,
            bank: -1,
            program: -1,
            channel: 0,
            bypass_cc: false,
            bypass_channel: -1,
            bypass_time1: 0.0,
            bypass_time2: 10.0,
        });
    }
    pub fn is_combi(&self) -> bool {
        !self.zones.is_empty()
    }
    pub fn zone(&self, index: usize) -> Option<&CombiZone> {
        self.zones.get(index)
    }
    pub fn into_item(self) -> BrowserItem {
        self.item
    }
}

#[derive(Debug, Clone)]
pub struct PatchBank {
    pub port: i32,
    pub tag: i32,
    pub suppress_change: bool,
    pub items: Vec<PatchItem>,
    pub current_index: Option<usize>,
}
impl PatchBank {
    pub fn new(port: i32, tag: i32, suppress_change: bool) -> Self {
        Self {
            port,
            tag,
            suppress_change,
            items: Vec::new(),
            current_index: None,
        }
    }
    pub fn add(&mut self, item: PatchItem) {
        if self.current_index.is_none() {
            self.current_index = Some(0);
        }
        self.items.push(item);
    }
    pub fn current(&self) -> Option<&PatchItem> {
        self.current_index.and_then(|i| self.items.get(i))
    }
}

pub struct PatchBrowser {
    pub browser: Browser,
    pub banks: Vec<PatchBank>,
    pub current_bank: Option<usize>,
}
impl PatchBrowser {
    pub fn new(path: &str) -> Self {
        Self {
            browser: Browser::new(path, BrowserItemType::Patch),
            banks: Vec::new(),
            current_bank: None,
        }
    }
    pub fn add_bank(&mut self, bank: PatchBank) {
        self.banks.push(bank);
        if self.current_bank.is_none() {
            self.current_bank = Some(0);
        }
        self.refresh();
    }
    pub fn move_to_bank(&mut self, direction: i32) -> bool {
        let Some(i) = self.current_bank else {
            return false;
        };
        let n = i as i32 + direction;
        if n < 0 || n as usize >= self.banks.len() {
            return false;
        };
        self.current_bank = Some(n as usize);
        self.refresh();
        true
    }
    pub fn move_to_bank_index(&mut self, index: usize) -> bool {
        if index >= self.banks.len() {
            return false;
        };
        self.current_bank = Some(index);
        self.refresh();
        true
    }
    pub fn current_bank(&self) -> Option<&PatchBank> {
        self.current_bank.and_then(|i| self.banks.get(i))
    }
    fn refresh(&mut self) {
        self.browser.items = self
            .current_bank()
            .map(|b| b.items.iter().map(|p| p.item.clone()).collect())
            .unwrap_or_default();
        self.browser.current_index = self.browser.items.first().map(|_| 0);
    }
}

#[derive(Debug, Clone)]
pub struct LoopBrowserItem {
    pub item: BrowserItem,
    pub modified: Option<SystemTime>,
    pub filename: Option<String>,
}
impl LoopBrowserItem {
    pub fn new(
        time: Option<SystemTime>,
        name: &str,
        default_name: bool,
        filename: Option<&str>,
    ) -> Self {
        Self {
            item: BrowserItem::new(name, default_name, BrowserItemType::Loop),
            modified: time,
            filename: filename.map(strip_extension),
        }
    }
    pub fn into_item(self) -> BrowserItem {
        self.item
    }
}

#[derive(Debug, Clone)]
pub struct SceneBrowserItem {
    pub item: BrowserItem,
    pub modified: Option<SystemTime>,
    pub filename: Option<String>,
}
impl SceneBrowserItem {
    pub fn new(
        time: Option<SystemTime>,
        name: &str,
        default_name: bool,
        filename: Option<&str>,
    ) -> Self {
        Self {
            item: BrowserItem::new(name, default_name, BrowserItemType::Scene),
            modified: time,
            filename: filename.map(strip_extension),
        }
    }
    pub fn into_item(self) -> BrowserItem {
        self.item
    }
}

fn strip_extension(path: &str) -> String {
    path.rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(path)
        .to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotBrowser {
    pub names: Vec<String>,
    pub first_index: usize,
    pub displayed: Option<usize>,
}
impl SnapshotBrowser {
    pub fn new(names: Vec<String>) -> Self {
        Self {
            names,
            first_index: 0,
            displayed: None,
        }
    }
    pub fn rename(&mut self, index: usize, name: impl Into<String>) -> bool {
        if let Some(slot) = self.names.get_mut(index) {
            *slot = name.into();
            true
        } else {
            false
        }
    }
    pub fn display_range(&mut self, first: usize, count: usize) {
        self.first_index = first.min(self.names.len());
        self.displayed = Some(count.min(self.names.len().saturating_sub(self.first_index)));
    }
}

pub type FloDisplaySnapshots = SnapshotBrowser;
