//! Native, deterministic patch-browser state and MIDI/synth selection plans.

use crate::config::{FloConfig, PatchBankConfig};
use crate::fluidsynth::Patch;
use std::fs;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchZone {
    pub key_low: u8,
    pub key_high: u8,
    pub midi_port: u32,
    pub channel: u8,
    pub bank: Option<u16>,
    pub program: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchItemKind {
    Patch {
        soundfont_id: Option<i32>,
        bank: u16,
        program: u8,
        channel: u8,
    },
    Combi {
        zones: Vec<PatchZone>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchItem {
    pub id: usize,
    pub name: String,
    pub kind: PatchItemKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchBank {
    pub midi_port: u32,
    pub tag: Option<i32>,
    pub separate_channels: bool,
    pub suppress_program_changes: bool,
    pub items: Vec<PatchItem>,
    pub cursor: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthAction {
    pub soundfont_id: i32,
    pub channel: u8,
    pub bank: u16,
    pub program: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalMidiAction {
    pub midi_port: u32,
    pub channel: u8,
    pub bank: Option<u16>,
    pub program: Option<u8>,
}

/// MIDI echo destination selected by a patch or combi zone.
///
/// This is intentionally separate from `ExternalMidiAction`: program-change
/// suppression must not suppress the routing side effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EchoRouting {
    pub midi_port: u32,
    pub channel: u8,
    /// `None` for a regular patch; combi zones retain their C++ key range so
    /// input notes can fan out only to matching zones.
    pub key_range: Option<(u8, u8)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PatchActionPlan {
    pub synth: Vec<SynthAction>,
    pub external_midi: Vec<ExternalMidiAction>,
    pub echo_routing: Vec<EchoRouting>,
    pub suppress_program_changes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePatchBrowser {
    pub banks: Vec<PatchBank>,
    pub bank_cursor: usize,
}

impl NativePatchBrowser {
    pub fn from_config(config: &FloConfig) -> Result<Self, String> {
        let mut banks = Vec::new();
        for bank in &config.patch_banks {
            banks.extend(parse_patch_bank(bank)?);
        }
        Ok(Self {
            banks,
            bank_cursor: 0,
        })
    }

    pub fn current_bank(&self) -> Option<&PatchBank> {
        self.banks.get(self.bank_cursor)
    }

    pub fn prepend_synth_patches(&mut self, patches: &[Patch]) {
        if patches.is_empty() {
            return;
        }
        let items = patches
            .iter()
            .enumerate()
            .map(|(id, patch)| PatchItem {
                id,
                name: patch.name.clone(),
                kind: PatchItemKind::Patch {
                    soundfont_id: Some(patch.soundfont_id),
                    bank: patch.bank.clamp(0, u16::MAX as i32) as u16,
                    program: patch.program.clamp(0, u8::MAX as i32) as u8,
                    channel: patch.channel.min(15),
                },
            })
            .collect();
        self.banks.insert(
            0,
            PatchBank {
                midi_port: 0,
                tag: None,
                separate_channels: false,
                suppress_program_changes: false,
                items,
                cursor: 0,
            },
        );
        self.bank_cursor = 0;
    }
    pub fn current_item(&self) -> Option<&PatchItem> {
        self.current_bank()?.items.get(self.current_bank()?.cursor)
    }
    pub fn move_bank(&mut self, delta: isize) -> Option<&PatchBank> {
        self.bank_cursor = move_cursor(self.bank_cursor, self.banks.len(), delta);
        self.current_bank()
    }
    pub fn select_bank(&mut self, index: usize) -> Option<&PatchBank> {
        if index >= self.banks.len() {
            return None;
        }
        self.bank_cursor = index;
        self.current_bank()
    }
    pub fn move_item(&mut self, delta: isize) -> Option<&PatchItem> {
        let bank = self.banks.get_mut(self.bank_cursor)?;
        bank.cursor = move_cursor(bank.cursor, bank.items.len(), delta);
        self.current_item()
    }
    pub fn select_item(&mut self, index: usize) -> Option<&PatchItem> {
        let bank = self.banks.get_mut(self.bank_cursor)?;
        if index >= bank.items.len() {
            return None;
        }
        bank.cursor = index;
        self.current_item()
    }
    pub fn action_plan(&self) -> Option<PatchActionPlan> {
        let bank = self.current_bank()?;
        let item = bank.items.get(bank.cursor)?;
        Some(plan_for(bank, item))
    }
}

fn move_cursor(cur: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as isize + delta).clamp(0, len as isize - 1) as usize
}

fn parse_patch_bank(cfg: &PatchBankConfig) -> Result<Vec<PatchBank>, String> {
    let text = fs::read_to_string(&cfg.patches)
        .map_err(|e| format!("read patches '{}': {e}", cfg.patches.display()))?;
    let doc = roxmltree::Document::parse(&text)
        .map_err(|e| format!("parse patches '{}': {e}", cfg.patches.display()))?;
    let root = doc.root_element();
    if !root.has_tag_name("patchlist") {
        return Err(format!(
            "patches '{}' must start with <patchlist>",
            cfg.patches.display()
        ));
    }
    let mut groups = Vec::new();
    let mut items = Vec::new();
    let mut channel: Option<u8> = None;
    for node in root.children().filter(|n| n.is_element()) {
        let item = if node.has_tag_name("patch") {
            let ch = attr_u8(node, "channel", 0)?;
            if cfg.separate_channels && channel != Some(ch) && !items.is_empty() {
                groups.push(make_bank(cfg, std::mem::take(&mut items)));
            }
            channel = Some(ch);
            PatchItem {
                id: items.len(),
                name: node.attribute("name").unwrap_or("").to_owned(),
                kind: PatchItemKind::Patch {
                    soundfont_id: None,
                    bank: attr_u16(node, "bank", 0)?,
                    program: attr_u8(node, "program", 0)?,
                    channel: ch,
                },
            }
        } else if node.has_tag_name("combi") {
            let mut zones = Vec::new();
            for z in node.children().filter(|n| n.has_tag_name("zone")) {
                let (lo, hi) = parse_range(z.attribute("keyrange").unwrap_or("0>127"))?;
                zones.push(PatchZone {
                    key_low: lo,
                    key_high: hi,
                    midi_port: attr_u32(z, "midiport", cfg.midi_port)?,
                    channel: attr_u8(z, "channel", 0)?,
                    bank: attr_opt_u16(z, "bank")?,
                    program: attr_opt_u8(z, "program")?,
                });
            }
            PatchItem {
                id: items.len(),
                name: node.attribute("name").unwrap_or("").to_owned(),
                kind: PatchItemKind::Combi { zones },
            }
        } else {
            continue;
        };
        items.push(item);
    }
    if !items.is_empty() {
        groups.push(make_bank(cfg, items));
    }
    Ok(groups)
}

fn make_bank(cfg: &PatchBankConfig, items: Vec<PatchItem>) -> PatchBank {
    PatchBank {
        midi_port: cfg.midi_port,
        tag: cfg.tag,
        separate_channels: cfg.separate_channels,
        suppress_program_changes: cfg.suppress_program_changes,
        items,
        cursor: 0,
    }
}
fn attr_u8(n: roxmltree::Node<'_, '_>, k: &str, d: u8) -> Result<u8, String> {
    n.attribute(k)
        .map_or(Ok(d), |v| v.parse().map_err(|_| format!("invalid {k}")))
}
fn attr_u16(n: roxmltree::Node<'_, '_>, k: &str, d: u16) -> Result<u16, String> {
    n.attribute(k)
        .map_or(Ok(d), |v| v.parse().map_err(|_| format!("invalid {k}")))
}
fn attr_u32(n: roxmltree::Node<'_, '_>, k: &str, d: u32) -> Result<u32, String> {
    n.attribute(k)
        .map_or(Ok(d), |v| v.parse().map_err(|_| format!("invalid {k}")))
}
fn attr_opt_u8(n: roxmltree::Node<'_, '_>, k: &str) -> Result<Option<u8>, String> {
    n.attribute(k)
        .map(|v| v.parse().map_err(|_| format!("invalid {k}")))
        .transpose()
}
fn attr_opt_u16(n: roxmltree::Node<'_, '_>, k: &str) -> Result<Option<u16>, String> {
    n.attribute(k)
        .map(|v| v.parse().map_err(|_| format!("invalid {k}")))
        .transpose()
}
fn parse_range(s: &str) -> Result<(u8, u8), String> {
    let mut p = s.split('>');
    let lo = p
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| "invalid keyrange".to_string())?;
    let hi = p
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| "invalid keyrange".to_string())?;
    Ok((lo, hi))
}

fn plan_for(bank: &PatchBank, item: &PatchItem) -> PatchActionPlan {
    let mut out = PatchActionPlan {
        suppress_program_changes: bank.suppress_program_changes,
        ..Default::default()
    };
    let mut add = |port, channel, key_range, soundfont_id, b, p| {
        out.echo_routing.push(EchoRouting {
            midi_port: port,
            channel,
            key_range,
        });
        if port == 0 {
            if let (Some(soundfont_id), Some(bank), Some(program)) = (soundfont_id, b, p) {
                out.synth.push(SynthAction {
                    soundfont_id,
                    channel,
                    bank,
                    program,
                });
            }
        } else if b.is_some() || p.is_some() {
            out.external_midi.push(ExternalMidiAction {
                midi_port: port,
                channel,
                bank: b,
                program: p,
            });
        }
    };
    match &item.kind {
        PatchItemKind::Patch {
            soundfont_id,
            bank: patch_bank,
            program,
            channel,
        } => add(
            bank.midi_port,
            *channel,
            None,
            *soundfont_id,
            Some(*patch_bank),
            Some(*program),
        ),
        PatchItemKind::Combi { zones } => {
            for z in zones {
                add(
                    z.midi_port,
                    z.channel,
                    Some((z.key_low, z.key_high)),
                    None,
                    z.bank,
                    z.program,
                )
            }
        }
    }
    out
}
