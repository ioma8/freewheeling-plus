//! Bounded translation from native SDL/MIDI callbacks into FreeWheeling events.

use crate::core::CoreEvent;
use crate::event::{
    Event, EventManager, JoystickButtonInputEvent, KeyInputEvent, MIDIActiveSensingInputEvent,
    MIDIChannelPressureInputEvent, MIDIClockInputEvent, MIDIControllerInputEvent,
    MIDIKeyInputEvent, MIDIPitchBendInputEvent, MIDIPolyphonicPressureInputEvent,
    MIDIProgramChangeInputEvent, MIDIResetInputEvent, MIDISongPositionInputEvent,
    MIDISongSelectInputEvent, MIDIStartStopInputEvent, MIDISystemExclusiveInputEvent,
    MIDITimeCodeQuarterFrameInputEvent, MIDITuneRequestInputEvent, MouseButtonInputEvent,
    MouseMotionInputEvent,
};
use crate::midiio::{MidiEventSink, MidiMessage, MidiPortMessage};
use crate::sdlio::InputEvent;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

pub fn input_event(event: InputEvent) -> Result<Box<dyn Event>, CoreEvent> {
    match event {
        InputEvent::Quit => Err(CoreEvent::ExitSession),
        InputEvent::JoystickButton {
            joystick,
            button,
            down,
        } => Ok(Box::new(JoystickButtonInputEvent::new(
            down, button, joystick,
        ))),
        InputEvent::MouseMotion { x, y } => Ok(Box::new(MouseMotionInputEvent::new(x, y))),
        InputEvent::MouseButton { button, x, y, down } => {
            Ok(Box::new(MouseButtonInputEvent::new(down, button, x, y)))
        }
        InputEvent::Key {
            down,
            keysym,
            unicode,
        } => Ok(Box::new(KeyInputEvent::new(down, keysym, unicode))),
        // The legacy event API has room for one Unicode scalar only. Keep
        // this adapter for existing callers; new integration should use
        // `text_input_events` so no committed text is discarded.
        InputEvent::Text(text) => Ok(Box::new(KeyInputEvent::new(
            true,
            text.chars().next().map_or(0, |ch| ch as i32),
            text.chars().next().map_or(0, |ch| ch as i32),
        ))),
    }
}

/// Translate one native input into the event stream consumed by the core.
///
/// SDL text input is a UTF-8 payload and may contain more than one Unicode
/// scalar, so it must be expanded before posting. Other input kinds retain
/// the legacy one-input/one-event behavior.
pub fn input_events(event: InputEvent) -> Result<Vec<Box<dyn Event>>, CoreEvent> {
    match event {
        InputEvent::Text(text) => Ok(text_input_events(text)),
        event => input_event(event).map(|event| vec![event]),
    }
}

/// Expand committed SDL text into the legacy key-event stream. This is the
/// bridge boundary until the native event model gains a text payload.
pub fn text_input_events(text: String) -> Vec<Box<dyn Event>> {
    text.chars()
        .map(|ch| {
            let unicode = ch as i32;
            Box::new(KeyInputEvent::new(true, unicode, unicode)) as Box<dyn Event>
        })
        .collect()
}

pub fn midi_event(event: MidiPortMessage) -> Option<Box<dyn Event>> {
    // C++ MIDI input events default to configured echo port 1.  The physical
    // input-port index is not an output route (and C++ treats outport as
    // one-based), so forwarding the zero-based capture port here previously
    // produced an invalid route for the first input device.
    let outport = 1;
    match event.message {
        MidiMessage::NoteOn {
            channel,
            note,
            velocity,
        } => Some(Box::new(MIDIKeyInputEvent::new(
            channel, note, velocity, true,
        ))),
        MidiMessage::NoteOff {
            channel,
            note,
            velocity,
        } => Some(Box::new(MIDIKeyInputEvent::new(
            channel, note, velocity, false,
        ))),
        MidiMessage::Controller {
            channel,
            control,
            value,
        } => Some(Box::new(MIDIControllerInputEvent::new(
            channel, control, value,
        ))),
        MidiMessage::ProgramChange { channel, program } => Some(Box::new(
            MIDIProgramChangeInputEvent::new(outport, channel, program, false),
        )),
        MidiMessage::ChannelPressure { channel, value } => Some(Box::new(
            MIDIChannelPressureInputEvent::new(outport, channel, value, false),
        )),
        MidiMessage::PitchBend { channel, value } => Some(Box::new(MIDIPitchBendInputEvent::new(
            channel,
            i32::from(value),
        ))),
        MidiMessage::Clock => Some(Box::new(MIDIClockInputEvent::new())),
        MidiMessage::Start | MidiMessage::Continue => {
            Some(Box::new(MIDIStartStopInputEvent::new(true)))
        }
        MidiMessage::Stop => Some(Box::new(MIDIStartStopInputEvent::new(false))),
        MidiMessage::PolyphonicPressure {
            channel,
            note,
            value,
        } => Some(Box::new(MIDIPolyphonicPressureInputEvent::new(
            channel, note, value,
        ))),
        MidiMessage::SystemExclusive(bytes) => {
            Some(Box::new(MIDISystemExclusiveInputEvent::new(bytes)))
        }
        MidiMessage::TimeCodeQuarterFrame(value) => Some(Box::new(
            MIDITimeCodeQuarterFrameInputEvent::new(u16::from(value)),
        )),
        MidiMessage::SongPosition(value) => Some(Box::new(MIDISongPositionInputEvent::new(value))),
        MidiMessage::SongSelect(value) => {
            Some(Box::new(MIDISongSelectInputEvent::new(u16::from(value))))
        }
        MidiMessage::TuneRequest => Some(Box::new(MIDITuneRequestInputEvent::new())),
        MidiMessage::ActiveSensing => Some(Box::new(MIDIActiveSensingInputEvent::new())),
        MidiMessage::Reset => Some(Box::new(MIDIResetInputEvent::new())),
    }
}

/// MIDI callback sink whose callback performs only a bounded `try_send`.
pub struct NativeEventBridge {
    sender: Option<mpsc::SyncSender<MidiPortMessage>>,
    dropped: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl NativeEventBridge {
    pub fn new(manager: Arc<EventManager>, capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let worker = thread::Builder::new()
            .name("native-event-bridge".into())
            .spawn(move || {
                while worker_running.load(Ordering::Acquire) {
                    match receiver.recv_timeout(std::time::Duration::from_millis(20)) {
                        Ok(message) => {
                            if let Some(event) = midi_event(message) {
                                let _ = manager.try_post_event(event);
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("native event bridge thread");
        Self {
            sender: Some(sender),
            dropped,
            running,
            worker: Some(worker),
        }
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn shutdown(&mut self) {
        self.running.store(false, Ordering::Release);
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl MidiEventSink for NativeEventBridge {
    fn midi_event(&self, event: MidiPortMessage) {
        if self
            .sender
            .as_ref()
            .is_none_or(|sender| sender.try_send(event).is_err())
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Drop for NativeEventBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventType;

    #[test]
    fn maps_all_actionable_native_families() {
        assert_eq!(
            input_event(InputEvent::Key {
                down: true,
                keysym: 32,
                unicode: 0
            })
            .unwrap()
            .get_type(),
            EventType::InputKey
        );
        assert_eq!(
            input_event(InputEvent::MouseMotion { x: 1, y: 2 })
                .unwrap()
                .get_type(),
            EventType::InputMouseMotion
        );
        assert_eq!(
            midi_event(MidiPortMessage {
                port: 0,
                message: MidiMessage::Clock
            })
            .unwrap()
            .get_type(),
            EventType::InputMIDIClock
        );
        let bend = midi_event(MidiPortMessage {
            port: 0,
            message: MidiMessage::PitchBend {
                channel: 2,
                value: 0,
            },
        })
        .unwrap();
        assert_eq!(
            bend.as_any()
                .downcast_ref::<MIDIPitchBendInputEvent>()
                .unwrap()
                .val,
            0
        );

        let cases = [
            (
                MidiMessage::PolyphonicPressure {
                    channel: 3,
                    note: 64,
                    value: 91,
                },
                EventType::InputMIDIPolyphonicPressure,
            ),
            (
                MidiMessage::TimeCodeQuarterFrame(0x71),
                EventType::InputMIDITimeCodeQuarterFrame,
            ),
            (
                MidiMessage::SongPosition(0x3fff),
                EventType::InputMIDISongPosition,
            ),
            (MidiMessage::SongSelect(127), EventType::InputMIDISongSelect),
            (MidiMessage::TuneRequest, EventType::InputMIDITuneRequest),
            (
                MidiMessage::ActiveSensing,
                EventType::InputMIDIActiveSensing,
            ),
            (MidiMessage::Reset, EventType::InputMIDIReset),
        ];
        for (message, expected) in cases {
            assert_eq!(
                midi_event(MidiPortMessage { port: 2, message })
                    .unwrap()
                    .get_type(),
                expected
            );
        }

        let bytes = vec![0xf0, 0x00, 0x20, 0x33, 0x7f, 0xf7];
        let sysex = midi_event(MidiPortMessage {
            port: 1,
            message: MidiMessage::SystemExclusive(bytes.clone()),
        })
        .unwrap();
        assert_eq!(sysex.get_type(), EventType::InputMIDISystemExclusive);
        assert_eq!(
            sysex
                .as_any()
                .downcast_ref::<MIDISystemExclusiveInputEvent>()
                .unwrap()
                .bytes,
            bytes
        );
    }

    #[test]
    fn expands_text_without_dropping_scalars() {
        let events = input_events(InputEvent::Text("a🙂é".into())).unwrap();
        assert_eq!(events.len(), 3);
        let unicode: Vec<i32> = events
            .iter()
            .map(|event| {
                event
                    .as_any()
                    .downcast_ref::<KeyInputEvent>()
                    .unwrap()
                    .unicode
            })
            .collect();
        assert_eq!(unicode, vec!['a' as i32, '🙂' as i32, 'é' as i32]);
    }

    #[test]
    fn expands_empty_and_supplementary_text_deterministically() {
        assert!(
            input_events(InputEvent::Text(String::new()))
                .unwrap()
                .is_empty()
        );

        let events = input_events(InputEvent::Text("中𐐷".into())).unwrap();
        let keys: Vec<(bool, i32, i32)> = events
            .iter()
            .map(|event| {
                let key = event.as_any().downcast_ref::<KeyInputEvent>().unwrap();
                (key.down, key.keysym, key.unicode)
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                (true, '中' as i32, '中' as i32),
                (true, '𐐷' as i32, '𐐷' as i32)
            ]
        );
    }

    #[test]
    fn preserves_legacy_key_mapping_through_input_events() {
        let event = input_events(InputEvent::Key {
            down: false,
            keysym: 304,
            unicode: 0,
        })
        .unwrap()
        .pop()
        .unwrap();
        let key = event.as_any().downcast_ref::<KeyInputEvent>().unwrap();
        assert_eq!((key.down, key.keysym, key.unicode), (false, 304, 0));
    }
}
