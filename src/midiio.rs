//! MIDI protocol handling and the backend-neutral worker.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

pub const MIDI_CLOCK_FREQUENCY: u32 = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiMessage {
    NoteOff { channel: u8, note: u8, velocity: u8 },
    NoteOn { channel: u8, note: u8, velocity: u8 },
    PolyphonicPressure { channel: u8, note: u8, value: u8 },
    Controller { channel: u8, control: u8, value: u8 },
    ProgramChange { channel: u8, program: u8 },
    ChannelPressure { channel: u8, value: u8 },
    PitchBend { channel: u8, value: u16 },
    TimeCodeQuarterFrame(u8),
    SongPosition(u16),
    SongSelect(u8),
    TuneRequest,
    Clock,
    Start,
    Continue,
    Stop,
    ActiveSensing,
    Reset,
    SystemExclusive(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiPortMessage {
    pub port: usize,
    pub message: MidiMessage,
}

pub type PatchRef = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchMidiRoute {
    pub port: u8,
    pub channel: u8,
    pub bank: u8,
    pub program: u8,
}
pub fn clamp7(value: i32) -> u8 {
    value.clamp(0, 127) as u8
}

/// Decode exactly one complete MIDI message. Running-status fragments are not
/// accepted because native backends promise complete callback messages.
pub fn decode(bytes: &[u8]) -> Option<MidiMessage> {
    let status = *bytes.first()?;
    let data7 = |index: usize| bytes.get(index).copied().filter(|v| *v < 0x80);
    let channel = status & 0x0f;
    match status {
        0x80..=0x8f if bytes.len() == 3 => Some(MidiMessage::NoteOff {
            channel,
            note: data7(1)?,
            velocity: data7(2)?,
        }),
        0x90..=0x9f if bytes.len() == 3 => {
            let note = data7(1)?;
            let velocity = data7(2)?;
            Some(if velocity == 0 {
                MidiMessage::NoteOff {
                    channel,
                    note,
                    velocity,
                }
            } else {
                MidiMessage::NoteOn {
                    channel,
                    note,
                    velocity,
                }
            })
        }
        0xa0..=0xaf if bytes.len() == 3 => Some(MidiMessage::PolyphonicPressure {
            channel,
            note: data7(1)?,
            value: data7(2)?,
        }),
        0xb0..=0xbf if bytes.len() == 3 => Some(MidiMessage::Controller {
            channel,
            control: data7(1)?,
            value: data7(2)?,
        }),
        0xc0..=0xcf if bytes.len() == 2 => Some(MidiMessage::ProgramChange {
            channel,
            program: data7(1)?,
        }),
        0xd0..=0xdf if bytes.len() == 2 => Some(MidiMessage::ChannelPressure {
            channel,
            value: data7(1)?,
        }),
        0xe0..=0xef if bytes.len() == 3 => Some(MidiMessage::PitchBend {
            channel,
            // FreeWheeling historically treated pitch bend as two bytes, not
            // as the packed 14-bit value prescribed by MIDI.
            value: data7(1)? as u16 | ((data7(2)? as u16) << 8),
        }),
        0xf0 if bytes.len() >= 2 && bytes.last() == Some(&0xf7) => {
            Some(MidiMessage::SystemExclusive(bytes.to_vec()))
        }
        0xf1 if bytes.len() == 2 => Some(MidiMessage::TimeCodeQuarterFrame(data7(1)?)),
        0xf2 if bytes.len() == 3 => Some(MidiMessage::SongPosition(
            data7(1)? as u16 | ((data7(2)? as u16) << 7),
        )),
        0xf3 if bytes.len() == 2 => Some(MidiMessage::SongSelect(data7(1)?)),
        0xf6 if bytes.len() == 1 => Some(MidiMessage::TuneRequest),
        0xf8 if bytes.len() == 1 => Some(MidiMessage::Clock),
        0xfa if bytes.len() == 1 => Some(MidiMessage::Start),
        0xfb if bytes.len() == 1 => Some(MidiMessage::Continue),
        0xfc if bytes.len() == 1 => Some(MidiMessage::Stop),
        0xfe if bytes.len() == 1 => Some(MidiMessage::ActiveSensing),
        0xff if bytes.len() == 1 => Some(MidiMessage::Reset),
        _ => None,
    }
}

pub fn encode(message: impl std::borrow::Borrow<MidiMessage>) -> Vec<u8> {
    match message.borrow() {
        MidiMessage::NoteOff {
            channel,
            note,
            velocity,
        } => vec![
            0x80 | (*channel).min(15),
            (*note).min(127),
            (*velocity).min(127),
        ],
        MidiMessage::NoteOn {
            channel,
            note,
            velocity,
        } => vec![
            0x90 | (*channel).min(15),
            (*note).min(127),
            (*velocity).min(127),
        ],
        MidiMessage::PolyphonicPressure {
            channel,
            note,
            value,
        } => vec![
            0xa0 | (*channel).min(15),
            (*note).min(127),
            (*value).min(127),
        ],
        MidiMessage::Controller {
            channel,
            control,
            value,
        } => vec![
            0xb0 | (*channel).min(15),
            (*control).min(127),
            (*value).min(127),
        ],
        MidiMessage::ProgramChange { channel, program } => {
            vec![0xc0 | (*channel).min(15), (*program).min(127)]
        }
        MidiMessage::ChannelPressure { channel, value } => {
            vec![0xd0 | (*channel).min(15), (*value).min(127)]
        }
        MidiMessage::PitchBend { channel, value } => vec![
            0xe0 | (*channel).min(15),
            (*value as u8).min(127),
            ((*value >> 8) as u8).min(127),
        ],
        MidiMessage::TimeCodeQuarterFrame(v) => vec![0xf1, v & 127],
        MidiMessage::SongPosition(v) => vec![0xf2, (v & 127) as u8, ((v >> 7) & 127) as u8],
        MidiMessage::SongSelect(v) => vec![0xf3, v & 127],
        MidiMessage::TuneRequest => vec![0xf6],
        MidiMessage::Clock => vec![0xf8],
        MidiMessage::Start => vec![0xfa],
        MidiMessage::Continue => vec![0xfb],
        MidiMessage::Stop => vec![0xfc],
        MidiMessage::ActiveSensing => vec![0xfe],
        MidiMessage::Reset => vec![0xff],
        MidiMessage::SystemExclusive(bytes) => bytes.clone(),
    }
}

pub trait MidiBackend: Send + 'static {
    fn open(&mut self, inputs: usize, outputs: usize) -> Result<(), String>;
    fn receive(&mut self) -> Result<Option<MidiPortMessage>, String>;
    fn send(&mut self, message: MidiPortMessage) -> Result<(), String>;
    fn close(&mut self);
}
pub trait MidiEventSink: Send + Sync + 'static {
    fn midi_event(&self, event: MidiPortMessage);
}

pub struct MidiIo<B: MidiBackend> {
    backend: Arc<Mutex<B>>,
    sink: Option<Arc<dyn MidiEventSink>>,
    stop: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
    pub inputs: usize,
    pub outputs: usize,
    /// C++ `MidiIO::echoport`: zero disables external echo; positive values
    /// are one-based MIDI output port numbers.
    pub echo_port: i32,
    pub echo_channel: Option<u8>,
    /// C++ `FloConfig::transpose`, applied only to outgoing note events.
    pub note_transpose: i32,
    /// C++ `MidiIO::bendertune`, applied to pitch-bend values before echo.
    pub bend_tune: i32,
    pub sync_transmit: bool,
    pub held_notes: Vec<(u8, u8)>,
    pub note_port: [Option<u8>; 128],
    pub note_patch: [Option<PatchRef>; 128],
    pub patch_routes: HashMap<String, PatchMidiRoute>,
}
impl<B: MidiBackend> MidiIo<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend: Arc::new(Mutex::new(backend)),
            sink: None,
            stop: None,
            worker: None,
            inputs: 0,
            outputs: 0,
            // `MidiIO::MidiIO` defaults to its first external output.
            echo_port: 1,
            echo_channel: None,
            note_transpose: 0,
            bend_tune: 0,
            sync_transmit: false,
            held_notes: Vec::new(),
            note_port: [None; 128],
            note_patch: [const { None }; 128],
            patch_routes: HashMap::new(),
        }
    }
    pub fn set_sink(&mut self, sink: Arc<dyn MidiEventSink>) {
        self.sink = Some(sink);
    }
    pub fn activate(&mut self, inputs: usize, outputs: usize) -> Result<(), String> {
        if self.worker.is_some() {
            return Err("MIDI backend is already active".into());
        }
        self.backend
            .lock()
            .map_err(|_| "MIDI backend lock poisoned")?
            .open(inputs, outputs)?;
        self.inputs = inputs;
        self.outputs = outputs;
        let (tx, rx) = mpsc::channel();
        let backend = Arc::clone(&self.backend);
        let sink = self.sink.clone();
        self.stop = Some(tx);
        let worker = thread::Builder::new()
            .name("freewheeling-midi".into())
            .spawn(move || {
                while rx.try_recv().is_err() {
                    match backend
                        .lock()
                        .map_err(|_| ())
                        .and_then(|mut b| b.receive().map_err(|_| ()))
                    {
                        Ok(Some(event)) => {
                            if let Some(s) = &sink {
                                s.midi_event(event)
                            }
                        }
                        Ok(None) => thread::park_timeout(std::time::Duration::from_millis(1)),
                        Err(()) => break,
                    }
                }
                if let Ok(mut b) = backend.lock() {
                    b.close();
                }
            })
            .map_err(|e| {
                if let Ok(mut b) = self.backend.lock() {
                    b.close();
                }
                format!("MIDI: cannot start worker: {e}")
            })?;
        self.worker = Some(worker);
        Ok(())
    }
    pub fn send(&self, port: usize, message: MidiMessage) -> Result<(), String> {
        if port >= self.outputs {
            return Err("MIDI output port out of range".into());
        }
        self.backend
            .lock()
            .map_err(|_| "MIDI backend lock poisoned".into())
            .and_then(|mut b| b.send(MidiPortMessage { port, message }))
    }

    pub fn output_clock(&self, port: usize) -> Result<(), String> {
        if self.sync_transmit {
            self.send(port, MidiMessage::Clock)
        } else {
            Ok(())
        }
    }
    pub fn output_start(&self, port: usize) -> Result<(), String> {
        if self.sync_transmit {
            self.send(port, MidiMessage::Start)
        } else {
            Ok(())
        }
    }
    pub fn output_stop(&self, port: usize) -> Result<(), String> {
        if self.sync_transmit {
            self.send(port, MidiMessage::Stop)
        } else {
            Ok(())
        }
    }

    /// Fan MIDI sync to the configured zero-based output ports. C++ routes
    /// every incoming clock/start/stop event through this exact destination
    /// list rather than through the current patch echo route.
    pub fn output_clock_to_ports(&self, ports: &[usize]) -> Result<(), String> {
        for &port in ports {
            self.output_clock(port)?;
        }
        Ok(())
    }

    pub fn output_start_to_ports(&self, ports: &[usize]) -> Result<(), String> {
        for &port in ports {
            self.output_start(port)?;
        }
        Ok(())
    }

    pub fn output_stop_to_ports(&self, ports: &[usize]) -> Result<(), String> {
        for &port in ports {
            self.output_stop(port)?;
        }
        Ok(())
    }

    /// Apply C++ `SetMidiEchoPort` validation.  Invalid values are ignored,
    /// and zero is the explicit "no external echo" route (typically the
    /// internal FluidSynth route in a patch bank).
    pub fn set_echo_port(&mut self, port: i32) -> bool {
        if (0..=i32::try_from(self.outputs).unwrap_or(i32::MAX)).contains(&port) {
            self.echo_port = port;
            true
        } else {
            false
        }
    }

    /// Apply the configured echo channel and note transposition. Messages
    /// without channel/note data pass through unchanged.
    pub fn mapped_echo(&self, message: &MidiMessage) -> MidiMessage {
        let channel = |original| self.echo_channel.unwrap_or(original).min(15);
        // `OutputNote` casts `notenum + transpose` directly to an unsigned
        // byte; do not introduce a modern saturating clamp here.
        let note = |original: u8| i32::from(original).wrapping_add(self.note_transpose) as u8;
        match *message {
            MidiMessage::NoteOn {
                channel: c,
                note: n,
                velocity,
            } => MidiMessage::NoteOn {
                channel: channel(c),
                note: note(n),
                velocity,
            },
            MidiMessage::NoteOff {
                channel: c,
                note: n,
                velocity,
            } => MidiMessage::NoteOff {
                channel: channel(c),
                note: note(n),
                velocity,
            },
            MidiMessage::Controller {
                channel: c,
                control,
                value,
            } => MidiMessage::Controller {
                channel: channel(c),
                control,
                value,
            },
            MidiMessage::ProgramChange {
                channel: c,
                program,
            } => MidiMessage::ProgramChange {
                channel: channel(c),
                program,
            },
            MidiMessage::ChannelPressure { channel: c, value } => MidiMessage::ChannelPressure {
                channel: channel(c),
                value,
            },
            MidiMessage::PitchBend { channel: c, value } => MidiMessage::PitchBend {
                channel: channel(c),
                // C++ sends the low two bytes of the adjusted signed integer
                // on CoreMIDI, so wrapping to u16 is the portable equivalent.
                value: (i32::from(value).wrapping_add(self.bend_tune)) as u16,
            },
            _ => message.clone(),
        }
    }

    pub fn echo(&self, message: &MidiMessage) -> Result<(), String> {
        let Some(port) = self
            .echo_port
            .checked_sub(1)
            .and_then(|port| usize::try_from(port).ok())
        else {
            return Ok(());
        };
        self.send(port, self.mapped_echo(message))
    }

    /// Send through a C++ patch/combi route. `output_port` is one-based and
    /// zero denotes the internal synth, which has no external MIDI packet.
    pub fn echo_to_route(
        &self,
        output_port: i32,
        channel: u8,
        message: &MidiMessage,
    ) -> Result<(), String> {
        let Some(port) = output_port
            .checked_sub(1)
            .and_then(|port| usize::try_from(port).ok())
        else {
            return Ok(());
        };
        let mut routed = self.mapped_echo(message);
        match &mut routed {
            MidiMessage::NoteOff { channel: c, .. }
            | MidiMessage::NoteOn { channel: c, .. }
            | MidiMessage::PolyphonicPressure { channel: c, .. }
            | MidiMessage::Controller { channel: c, .. }
            | MidiMessage::ProgramChange { channel: c, .. }
            | MidiMessage::ChannelPressure { channel: c, .. }
            | MidiMessage::PitchBend { channel: c, .. } => *c = channel.min(15),
            _ => {}
        }
        self.send(port, routed)
    }
    pub fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.inputs = 0;
        self.outputs = 0;
    }
    pub fn receive(&mut self) -> Option<MidiPortMessage> {
        let msg = {
            let mut backend = self.backend.lock().ok()?;
            backend.receive().ok()?
        }?;
        match &msg.message {
            MidiMessage::NoteOn { channel, note, .. } => {
                self.held_notes.push((*note, *channel));
                self.note_port[*note as usize] = Some(msg.port as u8);
            }
            MidiMessage::NoteOff { note, .. } => {
                self.held_notes.retain(|(n, _)| *n != *note);
                self.note_port[*note as usize] = None;
            }
            _ => {}
        }
        Some(msg)
    }

    pub fn release_held_notes(&mut self) {
        let held: Vec<_> = self.held_notes.drain(..).collect();
        for (note, channel) in held {
            let port = self.note_port[note as usize].unwrap_or(0) as usize;
            let _ = self.send(port, MidiMessage::NoteOff { channel, note, velocity: 0 });
        }
    }

    pub fn send_bank_program(
        &mut self,
        port: usize,
        channel: u8,
        bank: u8,
        program: u8,
    ) -> Result<(), String> {
        self.send(
            port,
            MidiMessage::Controller {
                channel,
                control: 0,
                value: bank,
            },
        )?;
        self.send(
            port,
            MidiMessage::Controller {
                channel,
                control: 32,
                value: 0,
            },
        )?;
        self.send(
            port,
            MidiMessage::ProgramChange { channel, program },
        )?;
        Ok(())
    }

    pub fn set_midi_for_patch(&mut self, patch_id: String, route: PatchMidiRoute) {
        self.patch_routes.insert(patch_id, route);
    }

    pub fn apply_patch_route(&mut self, patch_id: &str) -> Result<(), String> {
        let route = self.patch_routes.get(patch_id).cloned();
        if let Some(route) = route {
            self.send_bank_program(
                route.port as usize,
                route.channel,
                route.bank,
                route.program,
            )
        } else {
            Ok(())
        }
    }
}
impl<B: MidiBackend> Drop for MidiIo<B> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct CapturingBackend {
        sent: Arc<Mutex<Vec<MidiPortMessage>>>,
    }

    impl MidiBackend for CapturingBackend {
        fn open(&mut self, _inputs: usize, _outputs: usize) -> Result<(), String> {
            Ok(())
        }

        fn receive(&mut self) -> Result<Option<MidiPortMessage>, String> {
            Ok(None)
        }

        fn send(&mut self, message: MidiPortMessage) -> Result<(), String> {
            self.sent.lock().unwrap().push(message);
            Ok(())
        }

        fn close(&mut self) {}
    }

    #[test]
    fn round_trips_every_message_family() {
        let messages = [
            MidiMessage::PolyphonicPressure {
                channel: 3,
                note: 64,
                value: 9,
            },
            MidiMessage::TimeCodeQuarterFrame(33),
            MidiMessage::SongSelect(4),
            MidiMessage::TuneRequest,
            MidiMessage::Continue,
            MidiMessage::ActiveSensing,
            MidiMessage::Reset,
            MidiMessage::SystemExclusive(vec![0xf0, 1, 2, 0xf7]),
        ];
        for message in messages {
            assert_eq!(decode(&encode(&message)), Some(message));
        }
    }
    #[test]
    fn validates_complete_packets() {
        assert!(decode(&[0x90, 60]).is_none());
        assert!(decode(&[0x90, 0x80, 1]).is_none());
        assert!(decode(&[0xf0, 1]).is_none());
        assert_eq!(
            decode(&[0x91, 60, 0]),
            Some(MidiMessage::NoteOff {
                channel: 1,
                note: 60,
                velocity: 0
            })
        );
    }
    #[test]
    fn clamps() {
        assert_eq!(clamp7(-1), 0);
        assert_eq!(clamp7(200), 127);
    }

    #[test]
    fn decoding_pitch_bend_preserves_historical_byte_layout() {
        assert_eq!(
            decode(&[0xe2, 0x7f, 0x7f]),
            Some(MidiMessage::PitchBend {
                channel: 2,
                value: 32_639
            })
        );
    }

    #[test]
    fn echo_port_uses_cpp_one_based_and_off_convention() {
        let backend = CapturingBackend::default();
        let sent = Arc::clone(&backend.sent);
        let mut midi = MidiIo::new(backend);
        assert_eq!(midi.echo_port, 1);
        midi.activate(0, 2).unwrap();

        midi.echo(&MidiMessage::Clock).unwrap();
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            &[MidiPortMessage {
                port: 0,
                message: MidiMessage::Clock,
            }]
        );

        assert!(midi.set_echo_port(0));
        midi.echo(&MidiMessage::Start).unwrap();
        assert_eq!(sent.lock().unwrap().len(), 1);

        assert!(!midi.set_echo_port(3));
        assert_eq!(midi.echo_port, 0);
        midi.shutdown();
    }

    #[test]
    fn transpose_and_bender_tune_affect_the_cpp_message_families() {
        let mut midi = MidiIo::new(CapturingBackend::default());
        midi.note_transpose = 5;
        midi.bend_tune = -2;

        assert_eq!(
            midi.mapped_echo(&MidiMessage::NoteOn {
                channel: 1,
                note: 60,
                velocity: 90,
            }),
            MidiMessage::NoteOn {
                channel: 1,
                note: 65,
                velocity: 90,
            }
        );
        assert_eq!(
            midi.mapped_echo(&MidiMessage::PitchBend {
                channel: 1,
                value: 1,
            }),
            MidiMessage::PitchBend {
                channel: 1,
                value: u16::MAX,
            }
        );
    }

    #[test]
    fn sync_messages_fan_out_to_each_configured_port() {
        let backend = CapturingBackend::default();
        let sent = Arc::clone(&backend.sent);
        let mut midi = MidiIo::new(backend);
        midi.activate(0, 3).unwrap();
        midi.sync_transmit = true;

        midi.output_clock_to_ports(&[2, 0]).unwrap();
        midi.output_start_to_ports(&[2, 0]).unwrap();
        midi.output_stop_to_ports(&[2, 0]).unwrap();

        let messages = sent.lock().unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|message| (&message.port, &message.message))
                .collect::<Vec<_>>(),
            vec![
                (&2, &MidiMessage::Clock),
                (&0, &MidiMessage::Clock),
                (&2, &MidiMessage::Start),
                (&0, &MidiMessage::Start),
                (&2, &MidiMessage::Stop),
                (&0, &MidiMessage::Stop),
            ]
        );
        midi.shutdown();
    }
}
