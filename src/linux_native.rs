//! Linux 1.1 JACK audio/MIDI/transport and direct ALSA control.
//!
//! The contract types are platform-neutral so transport and overflow behavior can
//! be tested without a JACK server. Native handles are compiled on Linux only.

use crate::audioio::{JackPosition, NFrames};
#[cfg(test)]
use crate::midiio::MidiMessage;
#[cfg(any(target_os = "linux", test))]
use crate::midiio::{decode, encode};

#[cfg(target_os = "linux")]
use crate::audioio::AudioMetrics;
#[cfg(target_os = "linux")]
use crate::midiio::MidiPortMessage;

#[cfg(any(target_os = "linux", test))]
const MIDI_INLINE_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportCommand {
    Start,
    Stop,
    Relocate(NFrames),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timebase {
    pub beats_per_minute: f64,
    pub beats_per_bar: u16,
    pub beat_type: u16,
    pub ticks_per_beat: u16,
}

impl Default for Timebase {
    fn default() -> Self {
        Self {
            beats_per_minute: 120.0,
            beats_per_bar: 4,
            beat_type: 4,
            ticks_per_beat: 1_920,
        }
    }
}

impl Timebase {
    pub fn validate(self) -> Result<Self, String> {
        if !self.beats_per_minute.is_finite() || self.beats_per_minute <= 0.0 {
            return Err("JACK timebase tempo must be positive and finite".into());
        }
        if self.beats_per_bar == 0 || self.beat_type == 0 || self.ticks_per_beat == 0 {
            return Err("JACK timebase signature and tick resolution must be nonzero".into());
        }
        Ok(self)
    }

    /// Deterministically derive JACK BBT fields from an absolute audio frame.
    pub fn position(self, frame: u64, sample_rate: u32) -> Result<JackPosition, String> {
        let tb = self.validate()?;
        if sample_rate == 0 {
            return Err("JACK sample rate must be nonzero".into());
        }
        let ticks_per_second = tb.beats_per_minute * f64::from(tb.ticks_per_beat) / 60.0;
        let total_ticks = frame as f64 * ticks_per_second / f64::from(sample_rate);
        let beat_index = (total_ticks / f64::from(tb.ticks_per_beat)).floor() as u64;
        let bar_index = beat_index / u64::from(tb.beats_per_bar);
        let tick = (total_ticks - beat_index as f64 * f64::from(tb.ticks_per_beat)).floor();
        Ok(JackPosition {
            frame: frame.min(u64::from(u32::MAX)) as u32,
            valid: 1,
            bar: (bar_index + 1).min(i32::MAX as u64) as i32,
            beat: (beat_index % u64::from(tb.beats_per_bar) + 1) as i32,
            beats_per_minute: tb.beats_per_minute,
            beats_per_bar: f32::from(tb.beats_per_bar),
            beat_type: i32::from(tb.beat_type),
            ticks_per_beat: i32::from(tb.ticks_per_beat),
            tick: tick.min(f64::from(i32::MAX)) as i32,
            bar_start_tick: (bar_index
                .saturating_mul(u64::from(tb.beats_per_bar))
                .saturating_mul(u64::from(tb.ticks_per_beat)))
            .min(i32::MAX as u64) as i32,
            frame_rate: sample_rate,
        })
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InlineMidi {
    port: u16,
    frame_offset: u32,
    len: u16,
    bytes: [u8; MIDI_INLINE_BYTES],
}

#[cfg(any(target_os = "linux", test))]
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

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use crate::amixer::MixerBackend;
    use crate::audioio::{AudioBackend, AudioCallback, AudioCallbackFn, BackendInfo, NUM_CHANNELS};
    use crate::realtime_guard::RealtimeMetrics;
    use alsa::ctl::{ElemId, ElemIface};
    use alsa::hctl::HCtl;
    use jack::{
        AsyncClient, AudioIn, AudioOut, Client, ClientOptions, Control, MidiIn, MidiOut,
        NotificationHandler, Port, ProcessHandler, ProcessScope, RawMidi, TransportState,
    };
    use rtrb::{Consumer, Producer, RingBuffer};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::time::Instant;

    const DEFAULT_QUEUE_CAPACITY: usize = 1_024;

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

    struct Notifications {
        shared: Arc<Shared>,
        realtime_metrics: Option<Arc<RealtimeMetrics>>,
    }

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
            let (rolling, position) =
                transport
                    .as_ref()
                    .map_or((false, JackPosition::default()), |t| {
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
                        (t.state == TransportState::Rolling, p)
                    });
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

        /// Attach acceptance instrumentation before activating the client.
        pub fn set_realtime_metrics(&mut self, metrics: Arc<RealtimeMetrics>) {
            self.realtime_metrics = Some(metrics);
        }

        pub fn receive_midi(&mut self) -> Option<MidiPortMessage> {
            let event = self.midi_input.as_mut()?.pop().ok()?;
            decode(event.bytes()).map(|message| MidiPortMessage {
                port: usize::from(event.port),
                message,
            })
        }

        pub fn send_midi(
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

        pub fn transport(&mut self, command: TransportCommand) -> Result<(), String> {
            self.transport_output
                .as_mut()
                .ok_or("JACK backend is not open")?
                .push(command)
                .map_err(|_| "JACK transport command queue is full".to_string())
        }

        pub fn is_rolling(&self) -> bool {
            self.shared.rolling.load(Ordering::Acquire)
        }
    }

    impl Default for JackAudioMidiBackend {
        fn default() -> Self {
            Self::new(1, 1)
        }
    }

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
            let _ = self.transport(TransportCommand::Relocate(frame));
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
    }

    // Kept outside the public shape: both queue endpoints must move together.
    type PendingQueues = (
        Producer<InlineMidi>,
        Consumer<InlineMidi>,
        Consumer<TransportCommand>,
    );

    #[derive(Default)]
    pub struct DirectAlsaMixerBackend {
        ctl: Option<HCtl>,
    }
    impl MixerBackend for DirectAlsaMixerBackend {
        fn open(&mut self, card: &str) -> Result<(), String> {
            self.close();
            let ctl = HCtl::new(card, false)
                .map_err(|e| format!("cannot open ALSA control {card}: {e}"))?;
            ctl.load()
                .map_err(|e| format!("cannot load ALSA controls for {card}: {e}"))?;
            self.ctl = Some(ctl);
            Ok(())
        }
        fn set_control(&mut self, numid: i32, values: &[i32]) -> Result<(), String> {
            let ctl = self.ctl.as_ref().ok_or("ALSA mixer is not open")?;
            let mut id = ElemId::new(ElemIface::Mixer);
            id.set_numid(numid as u32);
            let elem = ctl
                .find_elem(&id)
                .ok_or_else(|| format!("ALSA numid {numid} does not exist"))?;
            let mut value = elem
                .read()
                .map_err(|e| format!("cannot read ALSA numid {numid}: {e}"))?;
            for (index, raw) in values.iter().enumerate() {
                value
                    .set_integer(index as u32, *raw)
                    .ok_or_else(|| format!("ALSA numid {numid} value {index} is not integer"))?;
            }
            elem.write(&value)
                .map(|_| ())
                .map_err(|e| format!("cannot write ALSA numid {numid}: {e}"))
        }
        fn close(&mut self) {
            self.ctl = None;
        }
    }
}

#[cfg(target_os = "linux")]
pub use native::{DirectAlsaMixerBackend, JackAudioMidiBackend};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timebase_maps_frames_to_one_based_bbt() {
        let tb = Timebase::default();
        assert_eq!(tb.position(0, 48_000).unwrap().bar, 1);
        let beat_two = tb.position(24_000, 48_000).unwrap();
        assert_eq!((beat_two.bar, beat_two.beat, beat_two.tick), (1, 2, 0));
        let bar_two = tb.position(96_000, 48_000).unwrap();
        assert_eq!((bar_two.bar, bar_two.beat), (2, 1));
    }

    #[test]
    fn invalid_timebase_is_rejected() {
        assert!(
            Timebase {
                beats_per_minute: 0.0,
                ..Timebase::default()
            }
            .validate()
            .is_err()
        );
        assert!(Timebase::default().position(0, 0).is_err());
    }

    #[test]
    fn inline_midi_is_bounded_and_round_trips() {
        let bytes = encode(MidiMessage::NoteOn {
            channel: 3,
            note: 64,
            velocity: 127,
        });
        let event = InlineMidi::new(2, 17, &bytes).unwrap();
        assert_eq!(event.bytes(), bytes);
        assert_eq!(
            decode(event.bytes()),
            Some(MidiMessage::NoteOn {
                channel: 3,
                note: 64,
                velocity: 127
            })
        );
        assert!(InlineMidi::new(0, 0, &[0; MIDI_INLINE_BYTES + 1]).is_err());
    }
}
