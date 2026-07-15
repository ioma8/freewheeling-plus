//! Native `midir` backend and a deterministic registry backend for tests.

use crate::midiio::{MidiBackend, MidiMessage, MidiPortMessage, decode, encode};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

pub const DEFAULT_MIDI_QUEUE_CAPACITY: usize = 1024;
const CLIENT_NAME: &str = "FreeWheeling";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiPort {
    pub name: String,
    pub input: bool,
    pub output: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputSelection {
    Index(usize),
    ExactName(String),
}

/// Production MIDI backend. Callback delivery is bounded and never blocks:
/// on saturation the newest packet is dropped and `dropped_input_messages`
/// is incremented for diagnostics.
pub struct MidirMidiBackend {
    selection: Option<InputSelection>,
    queue_capacity: usize,
    receiver: Option<mpsc::Receiver<MidiPortMessage>>,
    inputs: Vec<midir::MidiInputConnection<()>>,
    outputs: Vec<midir::MidiOutputConnection>,
    dropped: Arc<AtomicU64>,
    open: bool,
}

impl MidirMidiBackend {
    pub fn new(selection: Option<InputSelection>) -> Self {
        Self::with_queue_capacity(selection, DEFAULT_MIDI_QUEUE_CAPACITY)
    }
    pub fn with_queue_capacity(selection: Option<InputSelection>, capacity: usize) -> Self {
        Self {
            selection,
            queue_capacity: capacity.max(1),
            receiver: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            dropped: Arc::new(AtomicU64::new(0)),
            open: false,
        }
    }
    pub fn discover_inputs() -> Result<Vec<String>, String> {
        let input = midir::MidiInput::new(CLIENT_NAME)
            .map_err(|e| format!("MIDI: input discovery failed: {e}"))?;
        let mut names = input
            .ports()
            .iter()
            .map(|p| {
                input
                    .port_name(p)
                    .map_err(|e| format!("MIDI: cannot read input name: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        names.dedup();
        Ok(names)
    }
    pub fn dropped_input_messages(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
    fn selected_names(&self, available: &[String], count: usize) -> Result<Vec<String>, String> {
        if count == 0 {
            return Ok(Vec::new());
        }
        if available.is_empty() {
            return Err("MIDI: no input sources discovered".into());
        }
        if count > available.len() {
            return Err(format!(
                "MIDI: requested {count} inputs but only {} discovered",
                available.len()
            ));
        }
        let first = match &self.selection {
            None => 0,
            Some(InputSelection::Index(i)) if *i < available.len() => *i,
            Some(InputSelection::Index(i)) => {
                return Err(format!("MIDI: selected input index {i} is out of range"));
            }
            Some(InputSelection::ExactName(name)) => available
                .iter()
                .position(|n| n == name)
                .ok_or_else(|| format!("MIDI: selected input {name:?} was not found"))?,
        };
        if first + count > available.len() {
            return Err("MIDI: not enough inputs after selected source".into());
        }
        Ok(available[first..first + count].to_vec())
    }
}

impl Default for MidirMidiBackend {
    fn default() -> Self {
        Self::new(None)
    }
}

fn enqueue_packet(
    sender: &mpsc::SyncSender<MidiPortMessage>,
    dropped: &AtomicU64,
    port: usize,
    bytes: &[u8],
) {
    if let Some(message) = decode(bytes)
        && sender.try_send(MidiPortMessage { port, message }).is_err()
    {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
}

impl MidiBackend for MidirMidiBackend {
    fn open(&mut self, input_count: usize, output_count: usize) -> Result<(), String> {
        if self.open {
            return Err("MIDI backend is already open".into());
        }
        let available = Self::discover_inputs()?;
        let selected = self.selected_names(&available, input_count)?;
        let (sender, receiver) = mpsc::sync_channel(self.queue_capacity);
        self.dropped.store(0, Ordering::Relaxed);

        for (logical_port, wanted_name) in selected.into_iter().enumerate() {
            let mut input = midir::MidiInput::new(CLIENT_NAME)
                .map_err(|e| format!("MIDI: cannot create input: {e}"))?;
            input.ignore(midir::Ignore::None);
            let port = input
                .ports()
                .into_iter()
                .find(|p| input.port_name(p).ok().as_deref() == Some(wanted_name.as_str()))
                .ok_or_else(|| {
                    format!("MIDI: input {wanted_name:?} disappeared during activation")
                })?;
            let tx = sender.clone();
            let dropped = Arc::clone(&self.dropped);
            let connection = input
                .connect(
                    &port,
                    CLIENT_NAME,
                    move |_, bytes, _| {
                        enqueue_packet(&tx, &dropped, logical_port, bytes);
                    },
                    (),
                )
                .map_err(|e| format!("MIDI: cannot connect input {wanted_name:?}: {e}"))?;
            self.inputs.push(connection);
        }
        drop(sender);
        for index in 0..output_count {
            let output = midir::MidiOutput::new(CLIENT_NAME)
                .map_err(|e| format!("MIDI: cannot create output: {e}"))?;
            self.outputs.push(create_virtual_output(
                output,
                &format!("{CLIENT_NAME} OUT {}", index + 1),
            )?);
        }
        self.receiver = Some(receiver);
        self.open = true;
        Ok(())
    }
    fn receive(&mut self) -> Result<Option<MidiPortMessage>, String> {
        if !self.open {
            return Err("MIDI backend is not open".into());
        }
        match self
            .receiver
            .as_ref()
            .expect("open backend has receiver")
            .try_recv()
        {
            Ok(message) => Ok(Some(message)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) if self.inputs.is_empty() => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("MIDI: all input callbacks disconnected".into())
            }
        }
    }
    fn send(&mut self, message: MidiPortMessage) -> Result<(), String> {
        if !self.open {
            return Err("MIDI backend is not open".into());
        }
        let output = self
            .outputs
            .get_mut(message.port)
            .ok_or_else(|| "MIDI output port out of range".to_string())?;
        output
            .send(&encode(&message.message))
            .map_err(|e| format!("MIDI: output failed: {e}"))
    }
    fn close(&mut self) {
        self.receiver = None;
        self.inputs.clear();
        self.outputs.clear();
        self.open = false;
    }
}

#[cfg(unix)]
fn create_virtual_output(
    output: midir::MidiOutput,
    name: &str,
) -> Result<midir::MidiOutputConnection, String> {
    use midir::os::unix::VirtualOutput;
    output
        .create_virtual(name)
        .map_err(|e| format!("MIDI: cannot create virtual output {name:?}: {e}"))
}
#[cfg(not(unix))]
fn create_virtual_output(
    _output: midir::MidiOutput,
    name: &str,
) -> Result<midir::MidiOutputConnection, String> {
    Err(format!(
        "MIDI: virtual output {name:?} is not supported on this platform"
    ))
}

#[derive(Default)]
struct PortState {
    ports: Vec<MidiPort>,
    incoming: VecDeque<MidiPortMessage>,
    outgoing: VecDeque<MidiPortMessage>,
    opened: bool,
}
#[derive(Clone, Default)]
pub struct PortRegistry {
    state: Arc<(Mutex<PortState>, Condvar)>,
}
impl PortRegistry {
    pub fn new(ports: impl IntoIterator<Item = MidiPort>) -> Self {
        Self {
            state: Arc::new((
                Mutex::new(PortState {
                    ports: ports.into_iter().collect(),
                    ..Default::default()
                }),
                Condvar::new(),
            )),
        }
    }
    pub fn ports(&self) -> Vec<MidiPort> {
        self.state.0.lock().unwrap().ports.clone()
    }
    pub fn push_input(&self, packet: MidiPortMessage) -> Result<(), String> {
        let (lock, wake) = &*self.state;
        let mut s = lock.lock().unwrap();
        if packet.port >= s.ports.len() || !s.ports[packet.port].input {
            return Err("MIDI input port out of range".into());
        }
        s.incoming.push_back(packet);
        wake.notify_one();
        Ok(())
    }
    pub fn take_output(&self) -> Option<MidiPortMessage> {
        self.state.0.lock().unwrap().outgoing.pop_front()
    }
}
pub struct RegistryMidiBackend {
    registry: PortRegistry,
}
impl RegistryMidiBackend {
    pub fn new(registry: PortRegistry) -> Self {
        Self { registry }
    }
    pub fn registry(&self) -> PortRegistry {
        self.registry.clone()
    }
}
impl MidiBackend for RegistryMidiBackend {
    fn open(&mut self, inputs: usize, outputs: usize) -> Result<(), String> {
        let mut s = self.registry.state.0.lock().unwrap();
        if s.ports.iter().filter(|p| p.input).count() < inputs
            || s.ports.iter().filter(|p| p.output).count() < outputs
        {
            return Err("MIDI: insufficient discovered ports".into());
        }
        s.opened = true;
        Ok(())
    }
    fn receive(&mut self) -> Result<Option<MidiPortMessage>, String> {
        let (lock, wake) = &*self.registry.state;
        let mut s = lock.lock().unwrap();
        if !s.opened {
            return Err("MIDI backend is not open".into());
        }
        if s.incoming.is_empty() {
            s = wake.wait_timeout(s, Duration::from_millis(10)).unwrap().0;
        }
        Ok(s.incoming.pop_front())
    }
    fn send(&mut self, message: MidiPortMessage) -> Result<(), String> {
        let mut s = self.registry.state.0.lock().unwrap();
        if !s.opened {
            return Err("MIDI backend is not open".into());
        }
        if message.port >= s.ports.len() || !s.ports[message.port].output {
            return Err("MIDI output port out of range".into());
        }
        s.outgoing.push_back(message);
        Ok(())
    }
    fn close(&mut self) {
        self.registry.state.0.lock().unwrap().opened = false;
        self.registry.state.1.notify_all();
    }
}
pub fn packet_callback<S: crate::midiio::MidiEventSink>(
    sink: &S,
    port: usize,
    packet: &[u8],
) -> bool {
    decode(packet)
        .map(|message| {
            sink.midi_event(MidiPortMessage { port, message });
            true
        })
        .unwrap_or(false)
}
pub fn output_message<B: MidiBackend>(
    io: &crate::midiio::MidiIo<B>,
    port: usize,
    message: MidiMessage,
) -> Result<(), String> {
    io.send(port, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midiio::{MidiEventSink, MidiIo};
    struct Sink(Mutex<Vec<MidiPortMessage>>);
    impl MidiEventSink for Sink {
        fn midi_event(&self, e: MidiPortMessage) {
            self.0.lock().unwrap().push(e);
        }
    }
    #[test]
    fn stable_selection_is_explicit() {
        let b =
            MidirMidiBackend::with_queue_capacity(Some(InputSelection::ExactName("B".into())), 0);
        assert_eq!(b.queue_capacity, 1);
        assert_eq!(
            b.selected_names(&["A".into(), "B".into()], 1).unwrap(),
            ["B"]
        );
        assert!(b.selected_names(&["A".into()], 1).is_err());
    }
    #[test]
    fn registry_routes_and_rejects_bad_ports() {
        let r = PortRegistry::new([MidiPort {
            name: "in/out".into(),
            input: true,
            output: true,
        }]);
        let mut io = MidiIo::new(RegistryMidiBackend::new(r.clone()));
        io.activate(1, 1).unwrap();
        output_message(&io, 0, MidiMessage::Start).unwrap();
        assert_eq!(r.take_output().unwrap().message, MidiMessage::Start);
        assert!(output_message(&io, 1, MidiMessage::Stop).is_err());
        io.shutdown();
    }
    #[test]
    fn callback_delivers_complete_messages_only() {
        let s = Sink(Mutex::new(Vec::new()));
        assert!(packet_callback(&s, 2, &[0x92, 60, 0]));
        assert!(!packet_callback(&s, 2, &[0x01]));
        assert_eq!(
            s.0.lock().unwrap()[0].message,
            MidiMessage::NoteOff {
                channel: 2,
                note: 60,
                velocity: 0
            }
        );
    }

    #[test]
    fn callback_queue_drops_newest_and_counts_overflow() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let dropped = AtomicU64::new(0);
        enqueue_packet(&sender, &dropped, 0, &[0xf8]);
        enqueue_packet(&sender, &dropped, 0, &[0xfa]);
        assert_eq!(receiver.try_recv().unwrap().message, MidiMessage::Clock);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }
}
