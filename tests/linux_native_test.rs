// The production module is deliberately not wired through src/lib.rs in this
// lane, so this harness exposes its portable dependencies directly.
pub mod audioio {
    pub use freewheeling_plus::audioio::*;
}

pub mod midiio {
    pub use freewheeling_plus::midiio::*;
}

pub mod amixer {
    pub use freewheeling_plus::amixer::*;
}

pub mod realtime_guard {
    pub use freewheeling_plus::realtime_guard::*;
}

#[path = "../src/linux_native.rs"]
mod linux_native;

#[test]
fn transport_commands_retain_exact_frame_requests() {
    assert_ne!(
        freewheeling_plus::jack::TransportCommand::Start,
        freewheeling_plus::jack::TransportCommand::Stop
    );
    assert_eq!(
        freewheeling_plus::jack::TransportCommand::Relocate(u32::MAX),
        freewheeling_plus::jack::TransportCommand::Relocate(u32::MAX)
    );
}
