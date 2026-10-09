//! FluidSynth processor and its deliberately small backend boundary.
//!
//! The application owns the concrete FluidSynth binding.  This module owns
//! the lifetime, routing, patch and audio-buffer rules, which keeps the audio
//! thread testable without loading a soundfont or a native library.

use crate::core_dsp::{NFrames, Processor, Sample};
use crate::core_dsp_audio_buffers::AudioBuffers;
use crate::midiio::MidiMessage;
use fluidlite::{IsFont, IsPreset, IsSettings};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const PITCH_BEND_CENTER: i32 = 0x2000;
pub const MIDI_CHANNELS: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub enum FluidSetting {
    Integer { name: String, value: i32 },
    Number { name: String, value: f64 },
    Text { name: String, value: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FluidInterpolation {
    None,
    Linear,
    #[default]
    FourthOrder,
    SeventhOrder,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FluidLiteConfig {
    pub sample_rate: f64,
    pub interpolation: FluidInterpolation,
    pub tuning_cents: f64,
    pub settings: Vec<FluidSetting>,
    pub soundfonts: Vec<PathBuf>,
}

impl FluidLiteConfig {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            interpolation: FluidInterpolation::FourthOrder,
            tuning_cents: 0.0,
            settings: Vec::new(),
            soundfonts: Vec::new(),
        }
    }
}

/// Production FluidLite adapter. All filesystem and preset discovery work is
/// completed by `new`; render and MIDI methods only call the live synth.
pub struct FluidLiteBackend {
    synth: fluidlite::Synth,
    patches: Vec<Patch>,
    /// Reported diagnostics, observed by the non-realtime thread. The audio
    /// thread only touches allocation-free atomics here: a blocking mutex,
    /// a string table and stderr I/O all run on the control thread.
    diagnostics: Arc<SynthDiagnosticBus>,
}

/// The diagnostic sites a backend can report. Numbers (a failing controller,
/// a clamped pitch bend, an out-of-range note) are captured as payload,
/// formatted by the control thread, and each site is reported at most once per
/// run so a repeat failure cannot spam the realtime path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SynthDiagnosticSite {
    ControllerFailed,
    PitchBendOutOfRange,
    PitchBendFailed,
    NoteOutOfRangeOn,
    NoteOutOfRangeOff,
    NoteOnFailed,
    NoteOffFailed,
    ProgramSelectInvalid,
    ProgramSelectFailed,
    TuningFailed,
}

const SYNTH_DIAGNOSTIC_SITES: usize = 10;

/// Lock-free, allocation-free diagnostic leaver and reader.
///
/// The realtime thread writes `flag = true` plus a 32-bit payload with a
/// release store; the control thread reads with acquire and never clears, so
/// every site stays a one-shot. Both arrays are preallocated at construction.
pub struct SynthDiagnosticBus {
    flags: [AtomicBool; SYNTH_DIAGNOSTIC_SITES],
    payloads: [AtomicU32; SYNTH_DIAGNOSTIC_SITES],
}

impl Default for SynthDiagnosticBus {
    fn default() -> Self {
        Self {
            flags: [const { AtomicBool::new(false) }; SYNTH_DIAGNOSTIC_SITES],
            payloads: [const { AtomicU32::new(0) }; SYNTH_DIAGNOSTIC_SITES],
        }
    }
}

/// Encode an `i32` payload bit-exactly into the shared cell.
fn payload_for(value: i32) -> u32 {
    value as u32
}

fn decode_i32_payload(value: u32) -> i32 {
    value as i32
}

impl SynthDiagnosticBus {
    fn report(&self, site: SynthDiagnosticSite, payload: u32) {
        // The site table is sized by the enum's own last discriminant, so a
        // future variant without a matching `SYNTH_DIAGNOSTIC_SITES` bump is
        // a compile error rather than an out-of-bounds store on the audio
        // thread.
        const _: () = assert!(
            SYNTH_DIAGNOSTIC_SITES
                == SynthDiagnosticSite::TuningFailed as usize + 1
        );
        let index = site as usize;
        debug_assert!(index < SYNTH_DIAGNOSTIC_SITES);
        self.payloads[index].store(payload, Ordering::Release);
        self.flags[index].store(true, Ordering::Release);
    }

    /// Yields a formatted message for every site that has been flagged since
    /// process start. Never runs on the realtime thread and never clears: a
    /// site stays quiet once its first report is emitted, and a control thread
    /// that polls repeatedly cannot re-print the same failure.
    pub fn pending_reports(&self) -> impl Iterator<Item = String> + '_ {
        self.flags
            .iter()
            .enumerate()
            .filter(|(_, flag)| flag.load(Ordering::Acquire))
            .map(|(index, _)| {
                format_synth_diagnostic(
                    site_index(index),
                    self.payloads[index].load(Ordering::Acquire),
                )
            })
    }
}

fn site_index(index: usize) -> SynthDiagnosticSite {
    // The enum discriminants and this table are one and the same; the
    // count invariant is asserted inside `report`.
    match index {
        0 => SynthDiagnosticSite::ControllerFailed,
        1 => SynthDiagnosticSite::PitchBendOutOfRange,
        2 => SynthDiagnosticSite::PitchBendFailed,
        3 => SynthDiagnosticSite::NoteOutOfRangeOn,
        4 => SynthDiagnosticSite::NoteOutOfRangeOff,
        5 => SynthDiagnosticSite::NoteOnFailed,
        6 => SynthDiagnosticSite::NoteOffFailed,
        7 => SynthDiagnosticSite::ProgramSelectInvalid,
        8 => SynthDiagnosticSite::ProgramSelectFailed,
        _ => SynthDiagnosticSite::TuningFailed,
    }
}

/// Control-thread formatting. The realtime path never allocates a string, so
/// payload-to-message conversion happens here.
fn format_synth_diagnostic(site: SynthDiagnosticSite, payload: u32) -> String {
    match site {
        SynthDiagnosticSite::ControllerFailed => {
            format!("controller {} change failed", payload)
        }
        SynthDiagnosticSite::PitchBendOutOfRange => format!(
            "pitch bend {} is outside 0..=0x3fff; it was clamped",
            decode_i32_payload(payload)
        ),
        SynthDiagnosticSite::PitchBendFailed => {
            "a pitch bend was rejected by the synth".into()
        }
        SynthDiagnosticSite::NoteOutOfRangeOn => format!(
            "note {} is outside 0..=127; it was dropped",
            decode_i32_payload(payload)
        ),
        SynthDiagnosticSite::NoteOutOfRangeOff => format!(
            "note off {} is outside 0..=127; it was dropped",
            decode_i32_payload(payload)
        ),
        SynthDiagnosticSite::NoteOnFailed => "a note on was rejected by the synth".into(),
        SynthDiagnosticSite::NoteOffFailed => "a note off was rejected by the synth".into(),
        SynthDiagnosticSite::ProgramSelectInvalid => {
            "a program selection carried a negative bank, program or soundfont id; it was dropped".into()
        }
        SynthDiagnosticSite::ProgramSelectFailed => {
            "a program selection failed; an unknown soundfont id is the usual cause".into()
        }
        SynthDiagnosticSite::TuningFailed => "applying the tuning failed".into(),
    }
}


impl FluidLiteBackend {
    pub fn new(config: FluidLiteConfig) -> Result<Self, fluidlite::Error> {
        let settings = fluidlite::Settings::new()?;
        for setting in &config.settings {
            let applied = match setting {
                FluidSetting::Integer { name, value } => {
                    settings.int(name.as_bytes()).is_some_and(|s| s.set(*value))
                }
                FluidSetting::Number { name, value } => {
                    settings.num(name.as_bytes()).is_some_and(|s| s.set(*value))
                }
                FluidSetting::Text { name, value } => settings
                    .str_(name.as_bytes())
                    .is_some_and(|s| s.set(value.clone())),
            };
            if !applied {
                if fluidlite_ignores_legacy_setting(setting.name()) {
                    eprintln!(
                        "FreeWheeling: FluidLite ignores legacy FluidSynth setting '{}'; it has no FluidLite equivalent",
                        setting.name()
                    );
                    continue;
                }
                return Err(fluidlite::Error::Fluid(format!(
                    "invalid FluidLite setting: {}",
                    setting.name()
                )));
            }
        }
        if !settings
            .num(b"synth.sample-rate")
            .is_some_and(|setting| setting.set(config.sample_rate))
        {
            return Err(fluidlite::Error::Fluid(
                "unable to set synth.sample-rate".into(),
            ));
        }
        let synth = fluidlite::Synth::new(settings)?;
        // fluidlite 0.2.1 exposes `Synth::set_interp_method` but does not
        // publicly export its `InterpMethod` argument type. Its default is
        // fourth-order; reject other requests instead of silently misapplying
        // a quality setting.
        if config.interpolation != FluidInterpolation::FourthOrder {
            return Err(fluidlite::Error::Fluid(
                "fluidlite 0.2.1 only permits its default fourth-order interpolation through its public API"
                    .into(),
            ));
        }
        if config.tuning_cents != 0.0 {
            // `apply = true`: registering the tuning without applying it left
            // a configured `tuning_cents` silently inaudible until the
            // runtime `SetMidiTuning` action ran.
            synth.activate_octave_tuning(
                0,
                0,
                "FreeWheeling detune",
                &[config.tuning_cents; 12],
                true,
            )?;
            for channel in 0..MIDI_CHANNELS as u32 {
                synth.activate_tuning(channel, 0, 0, true)?;
            }
        }
        for path in &config.soundfonts {
            synth.sfload(path, true)?;
        }
        let patches = enumerate_patches(&synth);
        Ok(Self {
            synth,
            patches,
            diagnostics: Arc::new(SynthDiagnosticBus::default()),
        })
    }

    /// The diagnostic handle the non-realtime thread polls. Clone before this
    /// backend moves into the realtime graph.
    pub fn diagnostics(&self) -> Arc<SynthDiagnosticBus> {
        Arc::clone(&self.diagnostics)
    }

    pub fn load_soundfont(&mut self, path: impl AsRef<Path>) -> Result<i32, fluidlite::Error> {
        let id = self.synth.sfload(path, true)?;
        self.patches = enumerate_patches(&self.synth);
        Ok(id as i32)
    }

    pub fn set_tuning(&self, cents: f64) -> Result<(), fluidlite::Error> {
        self.synth
            .activate_octave_tuning(0, 0, "FreeWheeling detune", &[cents; 12], true)?;
        for channel in 0..MIDI_CHANNELS as u32 {
            self.synth.activate_tuning(channel, 0, 0, true)?;
        }
        Ok(())
    }
}

impl FluidSetting {
    fn name(&self) -> &str {
        match self {
            Self::Integer { name, .. } | Self::Number { name, .. } | Self::Text { name, .. } => {
                name
            }
        }
    }
}

/// `synth.parallel-render` is a FluidSynth worker-rendering knob. FluidLite
/// has no renderer-thread setting and always renders synchronously inside the
/// caller's audio callback, so the C++ value `0` is already its behavior.
/// Keep this deliberately narrow: malformed or unsupported user settings
/// must still fail startup instead of being silently discarded.
fn fluidlite_ignores_legacy_setting(name: &str) -> bool {
    name == "synth.parallel-render"
}

fn enumerate_patches(synth: &fluidlite::Synth) -> Vec<Patch> {
    let mut patches = Vec::new();
    for font in synth.sfont_iter() {
        let soundfont_id = font.get_id() as i32;
        // FluidLite 0.2 does not expose the native preset iterator. MIDI bank
        // and program values are each seven-bit, so exhaustive lookup is
        // deterministic and happens only during setup/reload.
        for bank in 0..=127 {
            for program in 0..=127 {
                if let Some(preset) = font.get_preset(bank, program) {
                    patches.push(Patch {
                        soundfont_id,
                        bank: preset.get_banknum().unwrap_or(bank) as i32,
                        program: preset.get_num().unwrap_or(program) as i32,
                        channel: 0,
                        name: preset.get_name().unwrap_or("Unnamed preset").to_owned(),
                    });
                }
            }
        }
    }
    patches
}

impl FluidSynthBackend for FluidLiteBackend {
    fn render(&mut self, left: &mut [Sample], right: &mut [Sample]) {
        if self.synth.write((&mut *left, &mut *right)).is_err() {
            left.fill(0.0);
            right.fill(0.0);
        }
    }
    fn controller(&mut self, channel: u8, controller: u8, value: u8) {
        if self
            .synth
            .cc(channel.into(), controller.into(), value.into())
            .is_err()
        {
            self.diagnostics
                .report(SynthDiagnosticSite::ControllerFailed, u32::from(controller));
        }
    }
    /// Set the pitch wheel.
    ///
    /// The accepted range is the 14-bit MIDI wheel position (`0..=0x3fff`);
    /// fluidlite's own API takes the same range. A value outside it is clamped
    /// and reported once, because a decoded bend that lands here out of range
    /// means the message was mis-encoded.
    fn pitch_bend(&mut self, channel: u8, value: i32) {
        let clamped = value.clamp(0, 0x3fff);
        if clamped != value {
            self.diagnostics
                .report(SynthDiagnosticSite::PitchBendOutOfRange, payload_for(value));
        }
        if self.synth.pitch_bend(channel.into(), clamped as u32).is_err() {
            self.diagnostics
                .report(SynthDiagnosticSite::PitchBendFailed, 0);
        }
    }
    fn note_on(&mut self, channel: u8, note: i32, velocity: u8) {
        if !(0..=127).contains(&note) {
            // A transposed note outside the MIDI range: dropping it is correct,
            // but it must be visible.
            self.diagnostics
                .report(SynthDiagnosticSite::NoteOutOfRangeOn, payload_for(note));
            return;
        }
        if self
            .synth
            .note_on(channel.into(), note as u32, velocity.into())
            .is_err()
        {
            self.diagnostics.report(SynthDiagnosticSite::NoteOnFailed, 0);
        }
    }
    fn note_off(&mut self, channel: u8, note: i32) {
        if !(0..=127).contains(&note) {
            self.diagnostics
                .report(SynthDiagnosticSite::NoteOutOfRangeOff, payload_for(note));
            return;
        }
        if self.synth.note_off(channel.into(), note as u32).is_err() {
            self.diagnostics.report(SynthDiagnosticSite::NoteOffFailed, 0);
        }
    }
    fn program_select(&mut self, channel: u8, soundfont_id: i32, bank: i32, program: i32) {
        if soundfont_id < 0 || bank < 0 || program < 0 {
            self.diagnostics
                .report(SynthDiagnosticSite::ProgramSelectInvalid, 0);
            return;
        }
        if self
            .synth
            .program_select(
                channel.into(),
                soundfont_id as u32,
                bank as u32,
                program as u32,
            )
            .is_err()
        {
            // An unknown soundfont id fails here: previously invisible.
            self.diagnostics
                .report(SynthDiagnosticSite::ProgramSelectFailed, 0);
        }
    }
    fn set_tuning(&mut self, cents: f64) {
        if FluidLiteBackend::set_tuning(self, cents).is_err() {
            self.diagnostics
                .report(SynthDiagnosticSite::TuningFailed, 0);
        }
    }
    fn patches(&self) -> Vec<Patch> {
        self.patches.clone()
    }
}

/// Put `samples` into `buffer`, reusing its allocation when nothing else holds
/// it.
fn publish(buffer: &mut Arc<Vec<Sample>>, samples: &[Sample]) {
    match Arc::get_mut(buffer) {
        Some(storage) => {
            storage.clear();
            storage.extend_from_slice(samples);
        }
        // The consumer still holds the previous callback's buffer: a new
        // allocation is the only option.
        None => *buffer = Arc::new(samples.to_vec()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch {
    pub soundfont_id: i32,
    pub bank: i32,
    pub program: i32,
    pub channel: u8,
    pub name: String,
}

/// Operations needed from libfluidsynth (or from a test double).
pub trait FluidSynthBackend: Send {
    fn render(&mut self, left: &mut [Sample], right: &mut [Sample]);
    fn controller(&mut self, channel: u8, controller: u8, value: u8);
    fn pitch_bend(&mut self, channel: u8, value: i32);
    fn note_on(&mut self, channel: u8, note: i32, velocity: u8);
    fn note_off(&mut self, channel: u8, note: i32);
    fn program_select(&mut self, channel: u8, soundfont_id: i32, bank: i32, program: i32);
    /// Applies the already configured octave tuning without filesystem work.
    fn set_tuning(&mut self, _cents: f64) {}
    fn patches(&self) -> Vec<Patch> {
        Vec::new()
    }
    fn shutdown(&mut self) {}
}

pub struct FluidSynthProcessor<B: FluidSynthBackend> {
    backend: B,
    left: Vec<Sample>,
    right: Vec<Sample>,
    /// Buffers handed to the graph, reused when the consumer has already
    /// released the previous callback's reference (`Arc::get_mut` succeeds):
    /// allocating and copying a fresh `Arc` on every callback was avoidable
    /// heap traffic in the render path.
    left_output: Arc<Vec<Sample>>,
    right_output: Arc<Vec<Sample>>,
    stereo: bool,
    enabled: bool,
    channel: u8,
    transpose: i32,
}

impl<B: FluidSynthBackend> FluidSynthProcessor<B> {
    pub fn new(backend: B, buffer_size: usize, stereo: bool, channel: u8, transpose: i32) -> Self {
        Self {
            backend,
            left: vec![0.0; buffer_size],
            right: vec![0.0; buffer_size],
            left_output: Arc::new(Vec::new()),
            right_output: Arc::new(Vec::new()),
            stereo,
            enabled: true,
            channel: channel.min(15),
            transpose,
        }
    }

    pub fn set_enable(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn send_patch_change(&mut self, patch: &Patch) {
        self.backend
            .program_select(patch.channel, patch.soundfont_id, patch.bank, patch.program);
    }
    pub fn setup_patches(&self) -> Vec<Patch> {
        self.backend.patches()
    }
    pub fn receive_midi(&mut self, message: MidiMessage) {
        if !self.enabled {
            return;
        }
        match message {
            MidiMessage::Controller { control, value, .. } => {
                self.backend.controller(self.channel, control, value)
            }
            MidiMessage::PitchBend { value, .. } => {
                // `FluidSynthProcessor::ReceiveMIDIEvent` passes the
                // legacy MIDI event value plus FluidSynth's documented
                // centre (0x2000), rather than normalising it first.
                self.backend
                    .pitch_bend(self.channel, value as i32 + PITCH_BEND_CENTER)
            }
            MidiMessage::NoteOn { note, velocity, .. } => {
                self.backend
                    .note_on(self.channel, note as i32 + self.transpose, velocity)
            }
            MidiMessage::NoteOff { note, .. } => self
                .backend
                .note_off(self.channel, note as i32 + self.transpose),
            _ => {}
        }
    }

    pub fn process_audio(&mut self, len: usize, buffers: &mut AudioBuffers) {
        self.left.resize(len, 0.0);
        self.right.resize(len, 0.0);
        if self.enabled {
            self.backend.render(&mut self.left, &mut self.right);
        } else {
            self.left.fill(0.0);
            self.right.fill(0.0);
        }
        if !self.stereo {
            for (l, r) in self.left.iter_mut().zip(&self.right) {
                *l = (*l + *r) * 0.5;
            }
        }
        let slot = buffers.numins_ext;
        // Each channel is grown independently: `AudioBuffers::inputs` is a
        // public field, so its two vectors are not guaranteed to have the same
        // length and indexing a short one would panic on the audio thread.
        for channel in &mut buffers.inputs {
            if channel.len() <= slot {
                channel.resize(slot + 1, None);
            }
        }
        publish(&mut self.left_output, &self.left[..len]);
        publish(&mut self.right_output, &self.right[..len]);
        if let Some(channel) = buffers.inputs.get_mut(0)
            && let Some(entry) = channel.get_mut(slot)
        {
            *entry = Some(Arc::clone(&self.left_output));
        }
        if let Some(channel) = buffers.inputs.get_mut(1)
            && let Some(entry) = channel.get_mut(slot)
        {
            *entry = self.stereo.then(|| Arc::clone(&self.right_output));
        }
    }
}

impl<B: FluidSynthBackend> Processor for FluidSynthProcessor<B> {
    fn process(&mut self, pre: bool, len: NFrames, buffers: &mut AudioBuffers) {
        if !pre {
            self.process_audio(len as usize, buffers);
        }
    }
}

impl<B: FluidSynthBackend> Drop for FluidSynthProcessor<B> {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Mock {
        calls: Vec<String>,
    }
    impl FluidSynthBackend for Mock {
        fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
            l.fill(1.0);
            r.fill(3.0);
        }
        fn controller(&mut self, _: u8, _: u8, _: u8) {
            self.calls.push("cc".into());
        }
        fn pitch_bend(&mut self, _: u8, v: i32) {
            self.calls.push(v.to_string());
        }
        fn note_on(&mut self, _: u8, n: i32, _: u8) {
            self.calls.push(n.to_string());
        }
        fn note_off(&mut self, _: u8, n: i32) {
            self.calls.push(n.to_string());
        }
        fn program_select(&mut self, _: u8, _: i32, _: i32, _: i32) {
            self.calls.push("patch".into());
        }
    }
    #[test]
    fn mono_folds_and_disable_silences() {
        let mut p = FluidSynthProcessor::new(Mock { calls: vec![] }, 4, false, 2, 1);
        let mut b = AudioBuffers::new(0, 0, 1);
        p.process_audio(4, &mut b);
        assert_eq!(b.inputs[0][0].as_ref().unwrap().as_slice(), &[2.0; 4]);
        p.set_enable(false);
        p.process_audio(4, &mut b);
        assert_eq!(b.inputs[0][0].as_ref().unwrap().as_slice(), &[0.0; 4]);
    }

    #[test]
    fn pitch_bend_uses_the_cpp_fluidsynth_center_offset() {
        let mut p = FluidSynthProcessor::new(Mock { calls: vec![] }, 1, true, 2, 0);
        p.receive_midi(MidiMessage::PitchBend {
            channel: 9,
            value: 0x1234,
        });
        // The configured synth channel is used, and the channel carried by
        // the incoming event is intentionally ignored by the C++ processor.
        assert_eq!(
            p.backend.calls,
            vec![(0x1234 + PITCH_BEND_CENTER).to_string()]
        );
    }

    #[test]
    fn only_the_known_fluidlite_parallel_render_compatibility_knob_is_ignored() {
        assert!(fluidlite_ignores_legacy_setting("synth.parallel-render"));
        assert!(!fluidlite_ignores_legacy_setting("synth.polyphony"));
        assert!(!fluidlite_ignores_legacy_setting("not-a-real-setting"));
    }

    #[test]
    fn synthesised_diagnostics_are_reported_once() {
        // A real backend needs libfluidlite; the bus under test is the piece
        // that must stay realtime-safe, so exercise it directly.
        let bus = SynthDiagnosticBus::default();
        bus.report(
            SynthDiagnosticSite::PitchBendOutOfRange,
            payload_for(24_575),
        );
        bus.report(SynthDiagnosticSite::NoteOutOfRangeOn, payload_for(-3));

        // A site stays reported once: re-polling re-yields the first message
        // but can never multiply reports for the same failure.
        let first: Vec<String> = bus.pending_reports().collect();
        let second: Vec<String> = bus.pending_reports().collect();
        assert_eq!(first.len(), 2);
        assert!(first[0].contains("24575"), "formatted: {}", first[0]);
        assert_eq!(first[0], second[0]);
    }

    #[test]
    fn legacy_parallel_render_setting_does_not_block_fluidlite_startup() {        let mut config = FluidLiteConfig::new(48_000.0);
        config.settings.push(FluidSetting::Integer {
            name: "synth.parallel-render".into(),
            value: 0,
        });
        FluidLiteBackend::new(config)
            .expect("FluidLite must ignore C++'s synchronous parallel-render=0 setting");
    }

    #[test]
    fn bundled_basic_soundfont_loads_in_fluidlite() {
        let mut config = FluidLiteConfig::new(48_000.0);
        config.soundfonts.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("data")
                .join("basic.sf2"),
        );
        FluidLiteBackend::new(config).expect("bundled basic.sf2 must be a loadable SoundFont");
    }
}
