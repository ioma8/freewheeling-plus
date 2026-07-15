// The production module is deliberately not wired through src/lib.rs in this
// lane, so this harness exposes its portable dependencies directly.
pub mod audioio {
    pub use freewheeling_plus::audioio::*;
}

pub mod midiio {
    pub use freewheeling_plus::midiio::*;
}

#[path = "../src/linux_native.rs"]
mod linux_native;

#[test]
fn transport_commands_retain_exact_frame_requests() {
    assert_ne!(
        linux_native::TransportCommand::Start,
        linux_native::TransportCommand::Stop
    );
    assert_eq!(
        linux_native::TransportCommand::Relocate(u32::MAX),
        linux_native::TransportCommand::Relocate(u32::MAX)
    );
}
