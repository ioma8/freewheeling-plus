use freewheeling_plus::event::Event;
use freewheeling_plus::midiio::MidiIo;
use freewheeling_plus::midiio::{MidiMessage, MidiPortMessage, decode, encode};
use freewheeling_plus::midiio_platform::{MidiPort, PortRegistry, RegistryMidiBackend};
use freewheeling_plus::native_event_bridge::midi_event;

const GOLDEN: &str = include_str!("../fixtures/cpp-golden/midi/messages.log");

fn event(message: MidiMessage) -> Event {
    midi_event(MidiPortMessage { port: 0, message }).expect("actionable MIDI message")
}

#[test]
fn genuine_cpp_input_fixture_matches_codec_and_event_mapping() {
    assert!(GOLDEN.contains("input pitch-bend channel=6 data1=4660"));
    let cases = [
        (
            vec![0x80, 60, 45],
            MidiMessage::NoteOff {
                channel: 0,
                note: 60,
                velocity: 45,
            },
        ),
        (
            vec![0x91, 61, 99],
            MidiMessage::NoteOn {
                channel: 1,
                note: 61,
                velocity: 99,
            },
        ),
        (
            vec![0x92, 62, 0],
            MidiMessage::NoteOff {
                channel: 2,
                note: 62,
                velocity: 0,
            },
        ),
        (
            vec![0xb3, 7, 100],
            MidiMessage::Controller {
                channel: 3,
                control: 7,
                value: 100,
            },
        ),
        (
            vec![0xc4, 42],
            MidiMessage::ProgramChange {
                channel: 4,
                program: 42,
            },
        ),
        (
            vec![0xd5, 77],
            MidiMessage::ChannelPressure {
                channel: 5,
                value: 77,
            },
        ),
        (
            vec![0xe6, 0x34, 0x12],
            MidiMessage::PitchBend {
                channel: 6,
                value: 0x1234,
            },
        ),
    ];
    for (bytes, expected) in cases {
        assert_eq!(decode(&bytes), Some(expected.clone()));
        let encoded = encode(expected);
        if bytes == [0x92, 62, 0] {
            // A velocity-zero note-on is normalized to canonical note-off.
            assert_eq!(encoded, [0x82, 62, 0]);
        } else {
            assert_eq!(encoded, bytes);
        }
    }

    let key = event(decode(&[0x92, 62, 0]).unwrap());
    let (notenum, vel) = match &key {
        Event::MIDIKeyInput { notenum, vel, .. } => (*notenum, *vel),
        _ => unreachable!(),
    };
    assert_eq!(
        (notenum, vel),
        (62, 0)
    );
    let cc = event(decode(&[0xb3, 7, 100]).unwrap());
    let (channel, ctrl, val) = match &cc {
        Event::MIDIControllerInput { channel, ctrl, val, .. } => (*channel, *ctrl, *val),
        _ => unreachable!(),
    };
    assert_eq!((channel, ctrl, val), (3, 7, 100));
    let program = event(decode(&[0xc4, 42]).unwrap());
    let (ch, pv) = match &program {
        Event::MIDIProgramChangeInput { channel, val, .. } => (*channel, *val),
        _ => unreachable!(),
    };
    assert_eq!((ch, pv), (4, 42));
    let pressure = event(decode(&[0xd5, 77]).unwrap());
    let (pch, pval) = match &pressure {
        Event::MIDIChannelPressureInput { channel, val, .. } => (*channel, *val),
        _ => unreachable!(),
    };
    assert_eq!((pch, pval), (5, 77));
    let bend = event(decode(&[0xe6, 0x34, 0x12]).unwrap());
    let (bch, bval) = match &bend {
        Event::MIDIPitchBendInput { channel, val, .. } => (*channel, *val),
        _ => unreachable!(),
    };
    assert_eq!((bch, bval), (6, 0x1234));
}

#[test]
fn genuine_cpp_output_fixture_matches_clamping_sync_and_runtime_send() {
    for line in [
        "output port=0 bytes=92 40 7F",
        "output port=0 bytes=82 40 20",
        "output port=0 bytes=B3 4A 7F",
        "output port=0 bytes=B3 4B 00",
        "output port=0 bytes=C4 7F",
        "output port=0 bytes=D5 00",
        "output port=0 bytes=E6 34 12",
        "output port=0 bytes=F8",
        "output port=0 bytes=FA",
        "output port=0 bytes=FC",
    ] {
        assert!(GOLDEN.lines().any(|fixture| fixture == line));
    }
    let cases = [
        (
            MidiMessage::NoteOn {
                channel: 2,
                note: 64,
                velocity: 255,
            },
            vec![0x92, 0x40, 0x7f],
        ),
        (
            MidiMessage::NoteOff {
                channel: 2,
                note: 64,
                velocity: 32,
            },
            vec![0x82, 0x40, 0x20],
        ),
        (
            MidiMessage::Controller {
                channel: 3,
                control: 74,
                value: 255,
            },
            vec![0xb3, 0x4a, 0x7f],
        ),
        (
            MidiMessage::Controller {
                channel: 3,
                control: 75,
                value: 0,
            },
            vec![0xb3, 0x4b, 0],
        ),
        (
            MidiMessage::ProgramChange {
                channel: 4,
                program: 255,
            },
            vec![0xc4, 0x7f],
        ),
        (
            MidiMessage::ChannelPressure {
                channel: 5,
                value: 0,
            },
            vec![0xd5, 0],
        ),
        (
            MidiMessage::PitchBend {
                channel: 6,
                value: 0x1234,
            },
            vec![0xe6, 0x34, 0x12],
        ),
        (MidiMessage::Clock, vec![0xf8]),
        (MidiMessage::Start, vec![0xfa]),
        (MidiMessage::Stop, vec![0xfc]),
    ];
    for (message, bytes) in &cases {
        assert_eq!(encode(message), *bytes);
    }
    assert_eq!(
        encode(MidiMessage::NoteOn {
            channel: 255,
            note: 255,
            velocity: 255
        }),
        [0x9f, 0x7f, 0x7f]
    );

    let registry = PortRegistry::new([MidiPort {
        name: "out".into(),
        input: false,
        output: true,
    }]);
    let mut io = MidiIo::new(RegistryMidiBackend::new(registry.clone()));
    io.echo_channel = Some(255);
    io.bend_tune = 100;
    assert_eq!(
        io.mapped_echo(&MidiMessage::NoteOn {
            channel: 2,
            note: 64,
            velocity: 127,
        }),
        MidiMessage::NoteOn {
            channel: 15,
            // C++ applies `bendertune` in ReceivePitchBendEvent only; note
            // transposition is a separate config value.
            note: 64,
            velocity: 127,
        }
    );
    assert_eq!(
        io.mapped_echo(&MidiMessage::PitchBend {
            channel: 2,
            value: 0x1234,
        }),
        MidiMessage::PitchBend {
            channel: 15,
            value: 0x1298,
        }
    );
    io.activate(0, 1).unwrap();
    for (message, bytes) in cases {
        io.send(0, message).unwrap();
        assert_eq!(encode(registry.take_output().unwrap().message), bytes);
    }
    io.shutdown();

    for (wire, start) in [([0xfa], true), ([0xfc], false)] {
        let mapped = event(decode(&wire).unwrap());
        assert_eq!(
            match &mapped {
                Event::MIDIStartStopInput { start, .. } => *start,
                _ => unreachable!(),
            },
            start
        );
    }
}
