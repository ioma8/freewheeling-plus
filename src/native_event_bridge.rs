//! Bounded translation from native SDL/MIDI callbacks into FreeWheeling events.

use crate::core::CoreEvent;
use crate::event::{Event, EventManager};
use crate::midiio::{MidiEventSink, MidiMessage, MidiPortMessage};
use crate::sdlio::InputEvent;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

pub fn input_event(event: InputEvent) -> Result<Event, CoreEvent> {
    match event {
        InputEvent::Quit => Err(CoreEvent::ExitSession),
        InputEvent::JoystickButton {
            joystick,
            button,
            down,
        } => Ok(Event::JoystickButtonInput {
            down,
            button,
            joystick,
        }),
        InputEvent::MouseMotion { x, y } => Ok(Event::MouseMotionInput { x, y }),
        InputEvent::MouseButton {
            button,
            x,
            y,
            down,
        } => Ok(Event::MouseButtonInput {
            down,
            button,
            x,
            y,
        }),
        InputEvent::Key {
            down,
            keysym,
            unicode,
        } => Ok(Event::KeyInput {
            down,
            keysym,
            unicode,
        }),
        // The legacy event API has room for one Unicode scalar only. Keep
        // this adapter for existing callers; new integration should use
        // `text_input_events` so no committed text is discarded.
        InputEvent::Text(text) => {
            let keysym = text.chars().next().map_or(0, |ch| ch as i32);
            let unicode = text.chars().next().map_or(0, |ch| ch as i32);
            Ok(Event::KeyInput {
                down: true,
                keysym,
                unicode,
            })
        }
    }
}

/// Translate one native input into the event stream consumed by the core.
///
/// SDL text input is a UTF-8 payload and may contain more than one Unicode
/// scalar, so it must be expanded before posting. Other input kinds retain
/// the legacy one-input/one-event behavior.
pub fn input_events(event: InputEvent) -> Result<Vec<Event>, CoreEvent> {
    match event {
        InputEvent::Text(text) => Ok(text_input_events(text)),
        event => input_event(event).map(|event| vec![event]),
    }
}

/// Expand committed SDL text into the legacy key-event stream. This is the
/// bridge boundary until the native event model gains a text payload.
pub fn text_input_events(text: String) -> Vec<Event> {
    text.chars()
        .map(|ch| {
            let unicode = ch as i32;
            Event::KeyInput {
                down: true,
                keysym: unicode,
                unicode,
            }
        })
        .collect()
}
pub fn midi_event(event: MidiPortMessage) -> Option<Event> {
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
        } => Some(Event::MIDIKeyInput {
            outport,
            channel,
            notenum: note,
            vel: velocity,
            down: true,
            echo: false,
        }),
        MidiMessage::NoteOff {
            channel,
            note,
            velocity,
        } => Some(Event::MIDIKeyInput {
            outport,
            channel,
            notenum: note,
            vel: velocity,
            down: false,
            echo: false,
        }),
        MidiMessage::Controller {
            channel,
            control,
            value,
        } => Some(Event::MIDIControllerInput {
            outport,
            channel,
            ctrl: control,
            val: value,
            echo: false,
        }),
        MidiMessage::ProgramChange { channel, program } => {
            Some(Event::MIDIProgramChangeInput {
                outport,
                channel,
                val: program,
                echo: false,
            })
        }
        MidiMessage::ChannelPressure { channel, value } => {
            Some(Event::MIDIChannelPressureInput {
                outport,
                channel,
                val: value,
                echo: false,
            })
        }
        MidiMessage::PitchBend { channel, value } => Some(Event::MIDIPitchBendInput {
            outport,
            channel,
            val: i32::from(value),
            echo: false,
        }),
        MidiMessage::Clock => Some(Event::MIDIClockInput { outport }),
        MidiMessage::Start | MidiMessage::Continue => {
            Some(Event::MIDIStartStopInput {
                outport,
                start: true,
            })
        }
        MidiMessage::Stop => Some(Event::MIDIStartStopInput {
            outport,
            start: false,
        }),
        MidiMessage::PolyphonicPressure {
            channel,
            note,
            value,
        } => Some(Event::MIDIPolyphonicPressureInput {
            channel,
            notenum: note,
            val: value,
        }),
        MidiMessage::SystemExclusive(bytes) => {
            Some(Event::MIDISystemExclusiveInput { bytes })
        }
        MidiMessage::TimeCodeQuarterFrame(value) => {
            Some(Event::MIDITimeCodeQuarterFrameInput {
                value: u16::from(value),
            })
        }
        MidiMessage::SongPosition(value) => {
            Some(Event::MIDISongPositionInput { value })
        }
        MidiMessage::SongSelect(value) => {
            Some(Event::MIDISongSelectInput {
                value: u16::from(value),
            })
        }
        MidiMessage::TuneRequest => Some(Event::MIDITuneRequestInput),
        MidiMessage::ActiveSensing => Some(Event::MIDIActiveSensingInput),
        MidiMessage::Reset => Some(Event::MIDIResetInput),
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
        let Event::MIDIPitchBendInput { val, .. } = &bend else {
            panic!("expected MIDIPitchBendInput");
        };
        assert_eq!(*val, 0);

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
        let Event::MIDISystemExclusiveInput { bytes: sysex_bytes } = &sysex else {
            panic!("expected MIDISystemExclusiveInput");
        };
        assert_eq!(*sysex_bytes, bytes);
}

    #[test]
    fn expands_text_without_dropping_scalars() {
        let events = input_events(InputEvent::Text("a🙂é".into())).unwrap();
        assert_eq!(events.len(), 3);
        let unicode: Vec<i32> = events
            .iter()
            .map(|event| {
                let Event::KeyInput { unicode, .. } = event else {
                    panic!("expected KeyInput");
                };
                *unicode
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
                let Event::KeyInput { down, keysym, unicode } = event else {
                    panic!("expected KeyInput");
                };
                (*down, *keysym, *unicode)
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
        let Event::KeyInput { down, keysym, unicode } = event else {
            panic!("expected KeyInput");
        };
        assert_eq!((down, keysym, unicode), (false, 304, 0));
    }
}
