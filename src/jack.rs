//! JACK audio/MIDI/transport backend for Linux and macOS.

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::audioio::{
    AudioBackend, AudioCallback, AudioCallbackFn, AudioMetrics, BackendInfo, JackPosition, NFrames,
    NUM_CHANNELS, TransportState,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::midiio::{decode, encode, MidiPortMessage};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::realtime_guard::RealtimeMetrics;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use jack::{
    AsyncClient, AudioIn, AudioOut, Client, ClientOptions, Control, MidiIn, MidiOut,
    NotificationHandler, Port, ProcessHandler, ProcessScope, RawMidi,
    TransportState as JackTransportState,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use rtrb::{Consumer, Producer, RingBuffer};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::sync::Arc;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::time::Instant;

/// Configuration options for opening a JACK client.
///
/// This struct is platform-agnostic and can be constructed on any target.
pub struct JackOptions {
    pub midi_inputs: usize,
    pub midi_outputs: usize,
    pub client_name: String,
    pub connect_audio: bool,
    pub connect_midi: bool,
    pub realtime: bool,
}

impl Default for JackOptions {
    fn default() -> Self {
        Self {
            midi_inputs: 1,
            midi_outputs: 1,
            client_name: "FreeWheeling".into(),
            connect_audio: true,
            connect_midi: true,
            realtime: true,
        }
    }
}

// --- Platform-gated types ---

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const MIDI_INLINE_BYTES: usize = 256;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const DEFAULT_QUEUE_CAPACITY: usize = 1_024;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportCommand {
    Start,
    Stop,
    Relocate(NFrames),
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timebase {
    pub beats_per_minute: f64,
    pub beats_per_bar: f32,
    pub beat_type: i32,
    pub ticks_per_beat: i32,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl Default for Timebase {
    fn default() -> Self {
        Self {
            beats_per_minute: 120.0,
            beats_per_bar: 4.0,
            beat_type: 4,
            ticks_per_beat: 480,
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl Timebase {
    pub fn validate(&self) -> Result<(), String> {
        if self.beats_per_minute <= 0.0 {
            return Err("beats_per_minute must be positive".into());
        }
        if self.beats_per_bar <= 0.0 {
            return Err("beats_per_bar must be positive".into());
        }
        if self.beat_type <= 0 {
            return Err("beat_type must be positive".into());
        }
        if self.ticks_per_beat <= 0 {
            return Err("ticks_per_beat must be positive".into());
        }
        Ok(())
    }

    pub fn position(&self, frame: NFrames, sample_rate: NFrames) -> Result<JackPosition, String> {
        self.validate()?;
        if sample_rate == 0 {
            return Err("sample_rate must be non-zero".into());
        }
        let frames_per_beat = (60.0 / self.beats_per_minute * sample_rate as f64) as u64;
        let ticks_per_frame = self.ticks_per_beat as f64 / frames_per_beat as f64;
        let total_ticks = (frame as f64 * ticks_per_frame) as i32;
        let frames_per_bar = (frames_per_beat as f64 * self.beats_per_bar as f64) as u64;
        let bar = if frames_per_bar > 0 {
            (frame as u64 / frames_per_bar) as i32 + 1
        } else {
            1
        };
        let bar_start_tick =
            ((bar - 1) as f64 * self.beats_per_bar as f64 * self.ticks_per_beat as f64) as i32;
        let beat_in_bar = if frames_per_bar > 0 {
            ((frame as u64 % frames_per_bar) * self.beats_per_bar as u64 / frames_per_bar) as i32 + 1
        } else {
            1
        };
        Ok(JackPosition {
            frame,
            frame_rate: sample_rate,
            valid: 1,
            bar,
            beat: beat_in_bar,
            tick: (total_ticks - bar_start_tick) % self.ticks_per_beat,
            beats_per_minute: self.beats_per_minute,
            beats_per_bar: self.beats_per_bar,
            beat_type: self.beat_type,
            ticks_per_beat: self.ticks_per_beat,
            bar_start_tick,
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InlineMidi {
    port: u16,
    frame_offset: u32,
    len: u16,
    bytes: [u8; MIDI_INLINE_BYTES],
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl InlineMidi {
    fn new(port: usize, frame_offset: u32, bytes: &[u8]) -> Result<Self, String> {
        if port > usize::from(u16::MAX) {
            return Err("JACK MIDI port index is out of range".into());
        }
        if bytes.is_empty() || bytes.len() > MIDI_INLINE_BYTES {
            return Err(format!(
                "JACK MIDI message must contain 1..={MIDI_INLINE_BYTES} bytes"
            ));
        }
        let mut inline = Self {
            port: port as u16,
            frame_offset,
            len: bytes.len() as u16,
            bytes: [0; MIDI_INLINE_BYTES],
        };
        inline.bytes[..bytes.len()].copy_from_slice(bytes);
        Ok(inline)
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[derive(Default)]
struct Shared {
    xruns: AtomicU64,
    callbacks: AtomicU64,
    frames: AtomicU64,
    peak_nanos: AtomicU64,
    total_nanos: AtomicU64,
    midi_input_drops: AtomicU64,
    midi_output_drops: AtomicU64,
    stream_errors: AtomicU64,
    rolling: AtomicBool,
    frame: AtomicU32,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct Notifications {
    shared: Arc<Shared>,
    realtime_metrics: Option<Arc<RealtimeMetrics>>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl NotificationHandler for Notifications {
    unsafe fn shutdown(&mut self, _: jack::ClientStatus, _: &str) {
        self.shared.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    fn xrun(&mut self, _: &Client) -> Control {
        self.shared.xruns.fetch_add(1, Ordering::Relaxed);
        if let Some(metrics) = &self.realtime_metrics {
            metrics.record_unexplained_xrun();
        }
        Control::Continue
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct JackProcess {
    audio_in: [Port<AudioIn>; NUM_CHANNELS],
    audio_out: [Port<AudioOut>; NUM_CHANNELS],
    midi_in: Vec<Port<MidiIn>>,
    midi_out: Vec<Port<MidiOut>>,
    midi_rx: Producer<InlineMidi>,
    midi_tx: Consumer<InlineMidi>,
    transport_rx: Consumer<TransportCommand>,
    callback: AudioCallbackFn,
    shared: Arc<Shared>,
    realtime_metrics: Option<Arc<RealtimeMetrics>>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl ProcessHandler for JackProcess {
    fn process(&mut self, client: &Client, ps: &ProcessScope) -> Control {
        let _guard = self
            .realtime_metrics
            .as_ref()
            .map(|metrics| metrics.enter_callback());
        let started = Instant::now();
        while let Ok(command) = self.transport_rx.pop() {
            let result = match command {
                TransportCommand::Start => client.transport().start(),
                TransportCommand::Stop => client.transport().stop(),
                TransportCommand::Relocate(frame) => client.transport().locate(frame),
            };
            if result.is_err() {
                self.shared.stream_errors.fetch_add(1, Ordering::Relaxed);
            }
        }

        let transport = client.transport().query().ok();
        let (rolling, position) = transport.as_ref().map_or(
            (false, JackPosition::default()),
            |t| {
                let pos = &t.pos;
                let mut p = JackPosition {
                    frame: pos.frame(),
                    frame_rate: pos.frame_rate().unwrap_or(client.sample_rate()),
                    ..JackPosition::default()
                };
                if let Some(bbt) = pos.bbt() {
                    p.valid = 1;
                    p.bar = bbt.bar.min(i32::MAX as usize) as i32;
                    p.beat = bbt.beat.min(i32::MAX as usize) as i32;
                    p.tick = bbt.tick.min(i32::MAX as usize) as i32;
                    p.beats_per_minute = bbt.bpm;
                    p.beats_per_bar = bbt.sig_num;
                    p.beat_type = bbt.sig_denom as i32;
                    p.ticks_per_beat = bbt.ticks_per_beat as i32;
                    p.bar_start_tick = bbt.bar_start_tick as i32;
                }
                (t.state == JackTransportState::Rolling, p)
            },
        );
        self.shared.rolling.store(rolling, Ordering::Relaxed);
        self.shared.frame.store(position.frame, Ordering::Relaxed);

        for (port_index, port) in self.midi_in.iter().enumerate() {
            for event in port.iter(ps) {
                if let Ok(message) = InlineMidi::new(port_index, event.time, event.bytes)
                    && self.midi_rx.push(message).is_err()
                {
                    self.shared.midi_input_drops.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let nframes = ps.n_frames();
        while let Ok(event) = self.midi_tx.pop() {
            if let Some(port) = self.midi_out.get_mut(usize::from(event.port)) {
                let mut writer = port.writer(ps);
                if writer
                    .write(&RawMidi {
                        time: event.frame_offset.min(nframes.saturating_sub(1)),
                        bytes: event.bytes(),
                    })
                    .is_err()
                {
                    self.shared
                        .midi_output_drops
                        .fetch_add(1, Ordering::Relaxed);
                }
            } else {
                self.shared
                    .midi_output_drops
                    .fetch_add(1, Ordering::Relaxed);
            }
        }

        let input_l = self.audio_in[0].as_slice(ps);
        let input_r = self.audio_in[1].as_slice(ps);
        let (out_l, out_r) = self.audio_out.split_at_mut(1);
        let output_l = out_l[0].as_mut_slice(ps);
        let output_r = out_r[0].as_mut_slice(ps);
        let mut callback = AudioCallback {
            inputs: [input_l, input_r],
            outputs: [output_l, output_r],
            nframes,
            position,
            transport_rolling: rolling,
        };
        (self.callback)(&mut callback);

        let elapsed = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        self.shared.callbacks.fetch_add(1, Ordering::Relaxed);
        self.shared
            .frames
            .fetch_add(u64::from(nframes), Ordering::Relaxed);
        self.shared
            .total_nanos
            .fetch_add(elapsed, Ordering::Relaxed);
        self.shared.peak_nanos.fetch_max(elapsed, Ordering::Relaxed);
        Control::Continue
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
type PendingQueues = (
    Producer<InlineMidi>,
    Consumer<InlineMidi>,
    Consumer<TransportCommand>,
);

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[allow(clippy::type_complexity)]
pub struct JackAudioMidiBackend {
    client: Option<Client>,
    active: Option<AsyncClient<Notifications, JackProcess>>,
    ports: Option<(
        [Port<AudioIn>; NUM_CHANNELS],
        [Port<AudioOut>; NUM_CHANNELS],
        Vec<Port<MidiIn>>,
        Vec<Port<MidiOut>>,
    )>,
    midi_input: Option<Consumer<InlineMidi>>,
    midi_output: Option<Producer<InlineMidi>>,
    transport_output: Option<Producer<TransportCommand>>,
    pending: Option<PendingQueues>,
    midi_inputs: usize,
    midi_outputs: usize,
    shared: Arc<Shared>,
    realtime_metrics: Option<Arc<RealtimeMetrics>>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl JackAudioMidiBackend {
    pub fn new(midi_inputs: usize, midi_outputs: usize) -> Self {
        Self {
            client: None,
            active: None,
            ports: None,
            midi_input: None,
            midi_output: None,
            transport_output: None,
            pending: None,
            midi_inputs,
            midi_outputs,
            shared: Arc::new(Shared::default()),
            realtime_metrics: None,
        }
    }

    /// Create a JACK backend from a `JackOptions` config struct.
    pub fn from_options(opts: JackOptions) -> Self {
        Self::new(opts.midi_inputs, opts.midi_outputs)
    }

    /// Attach acceptance instrumentation before activating the client.
    pub fn set_realtime_metrics(&mut self, metrics: Arc<RealtimeMetrics>) {
        self.realtime_metrics = Some(metrics);
    }

    fn transport_push(&mut self, command: TransportCommand) -> Result<(), String> {
        self.transport_output
            .as_mut()
            .ok_or("JACK backend is not open")?
            .push(command)
            .map_err(|_| "JACK transport command queue is full".to_string())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl Default for JackAudioMidiBackend {
    fn default() -> Self {
        Self::new(1, 1)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl AudioBackend for JackAudioMidiBackend {
    fn open(&mut self, client_name: &str) -> Result<BackendInfo, String> {
        self.close();
        let (client, _) = Client::new(client_name, ClientOptions::NO_START_SERVER)
            .map_err(|error| format!("cannot open JACK client: {error}"))?;
        let audio_in = [
            client.register_port("audio_in_l", AudioIn::default()),
            client.register_port("audio_in_r", AudioIn::default()),
        ];
        let audio_out = [
            client.register_port("audio_out_l", AudioOut::default()),
            client.register_port("audio_out_r", AudioOut::default()),
        ];
        let audio_in = audio_in
            .map(|port| port.map_err(|e| format!("cannot register JACK audio input: {e}")))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| "invalid JACK input port count")?;
        let audio_out = audio_out
            .map(|port| port.map_err(|e| format!("cannot register JACK audio output: {e}")))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| "invalid JACK output port count")?;
        let midi_in = (0..self.midi_inputs)
            .map(|i| {
                client
                    .register_port(&format!("midi_in_{i}"), MidiIn::default())
                    .map_err(|e| format!("cannot register JACK MIDI input: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let midi_out = (0..self.midi_outputs)
            .map(|i| {
                client
                    .register_port(&format!("midi_out_{i}"), MidiOut::default())
                    .map_err(|e| format!("cannot register JACK MIDI output: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (midi_rx_producer, midi_rx_consumer) = RingBuffer::new(DEFAULT_QUEUE_CAPACITY);
        let (midi_tx_producer, midi_tx_consumer) = RingBuffer::new(DEFAULT_QUEUE_CAPACITY);
        let (transport_producer, transport_consumer) = RingBuffer::new(64);
        self.midi_input = Some(midi_rx_consumer);
        self.midi_output = Some(midi_tx_producer);
        self.transport_output = Some(transport_producer);
        self.ports = Some((audio_in, audio_out, midi_in, midi_out));
        self.client = Some(client);
        // Producers/consumers crossing into the callback are installed at activation.
        self.pending = Some((midi_rx_producer, midi_tx_consumer, transport_consumer));
        Ok(BackendInfo {
            sample_rate: self.client.as_ref().unwrap().sample_rate(),
            buffer_size: self.client.as_ref().unwrap().buffer_size(),
        })
    }

    fn activate(&mut self, callback: AudioCallbackFn) -> Result<(), String> {
        let client = self
            .client
            .take()
            .ok_or("JACK backend must be opened before activation")?;
        let (audio_in, audio_out, midi_in, midi_out) =
            self.ports.take().ok_or("JACK ports are not open")?;
        let (midi_rx, midi_tx, transport_rx) =
            self.pending.take().ok_or("JACK queues are not open")?;
        let process = JackProcess {
            audio_in,
            audio_out,
            midi_in,
            midi_out,
            midi_rx,
            midi_tx,
            transport_rx,
            callback,
            shared: Arc::clone(&self.shared),
            realtime_metrics: self.realtime_metrics.clone(),
        };
        self.active = Some(
            client
                .activate_async(
                    Notifications {
                        shared: Arc::clone(&self.shared),
                        realtime_metrics: self.realtime_metrics.clone(),
                    },
                    process,
                )
                .map_err(|e| format!("cannot activate JACK client: {e}"))?,
        );
        Ok(())
    }

    fn close(&mut self) {
        if let Some(active) = self.active.take() {
            let _ = active.deactivate();
        }
        self.client = None;
        self.ports = None;
        self.pending = None;
        self.midi_input = None;
        self.midi_output = None;
        self.transport_output = None;
    }

    fn relocate(&mut self, frame: NFrames) {
        let _ = self.transport_push(TransportCommand::Relocate(frame));
    }

    fn metrics(&self) -> AudioMetrics {
        AudioMetrics {
            capture_overruns: self.shared.midi_input_drops.load(Ordering::Relaxed),
            capture_underruns: self.shared.midi_output_drops.load(Ordering::Relaxed),
            xruns: self.shared.xruns.load(Ordering::Relaxed),
            stream_errors: self.shared.stream_errors.load(Ordering::Relaxed)
                + self.shared.xruns.load(Ordering::Relaxed),
            callbacks: self.shared.callbacks.load(Ordering::Relaxed),
            callback_frames: self.shared.frames.load(Ordering::Relaxed),
            callback_peak_nanos: self.shared.peak_nanos.load(Ordering::Relaxed),
            callback_total_nanos: self.shared.total_nanos.load(Ordering::Relaxed),
            recovery_requests: self.shared.xruns.load(Ordering::Relaxed),
        }
    }

    fn transport_state(&self) -> TransportState {
        TransportState {
            rolling: self.shared.rolling.load(Ordering::Acquire),
            frame: self.shared.frame.load(Ordering::Acquire),
            ..TransportState::default()
        }
    }

    fn receive_midi(&mut self) -> Option<MidiPortMessage> {
        let event = self.midi_input.as_mut()?.pop().ok()?;
        decode(event.bytes()).map(|message| MidiPortMessage {
            port: usize::from(event.port),
            message,
        })
    }

    fn send_midi(
        &mut self,
        event: MidiPortMessage,
        frame_offset: NFrames,
    ) -> Result<(), String> {
        let bytes = encode(event.message);
        let event = InlineMidi::new(event.port, frame_offset, &bytes)?;
        self.midi_output
            .as_mut()
            .ok_or("JACK backend is not open")?
            .push(event)
            .map_err(|_| "JACK MIDI output queue is full".to_string())
    }
}
