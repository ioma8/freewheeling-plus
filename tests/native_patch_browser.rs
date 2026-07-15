use freewheeling_plus::config::{FloConfig, PatchBankConfig};
use freewheeling_plus::native_patch_browser::{NativePatchBrowser, PatchItemKind};
use std::path::PathBuf;

fn fixture_browser() -> NativePatchBrowser {
    let mut config = FloConfig::new();
    config.patch_banks.push(PatchBankConfig {
        interface_id: 0,
        patches: PathBuf::from("../data/patches3.xml"),
        midi_port: 1,
        separate_channels: false,
        suppress_program_changes: true,
        tag: Some(7),
    });
    NativePatchBrowser::from_config(&config).unwrap()
}

#[test]
fn fixture_parses_combis_and_preserves_zone_routing() {
    let browser = fixture_browser();
    assert_eq!(browser.banks.len(), 1);
    let item = &browser.banks[0].items[0];
    assert_eq!(item.name, "Strings");
    match &item.kind {
        PatchItemKind::Combi { zones } => {
            assert_eq!(zones.len(), 5);
            assert_eq!((zones[4].key_low, zones[4].key_high), (77, 127));
            assert_eq!((zones[4].midi_port, zones[4].channel), (1, 12));
            assert_eq!((zones[4].bank, zones[4].program), (Some(32), Some(18)));
        }
        _ => panic!("fixture item is not a combi"),
    }
}

#[test]
fn movement_is_clamped_and_plan_keeps_suppression() {
    let mut browser = fixture_browser();
    assert_eq!(browser.select_item(1).unwrap().name, "Strings + Flute");
    assert!(browser.move_item(100).is_some());
    assert!(browser.move_item(-100).is_some());
    let plan = browser.action_plan().unwrap();
    assert!(plan.suppress_program_changes);
    assert_eq!(plan.external_midi.len(), 1);
    assert_eq!(plan.external_midi[0].midi_port, 1);
    assert_eq!(plan.external_midi[0].channel, 12);
    assert_eq!(plan.echo_routing[0].midi_port, 1);
    assert_eq!(plan.echo_routing[0].channel, 0);
}

#[test]
fn suppressed_program_changes_keep_routing_for_programless_combi_zones() {
    let browser = fixture_browser();
    let plan = browser.action_plan().unwrap();

    // Strings has four internal zones without program changes and one
    // external zone with a program change. All five still route echo input.
    assert_eq!(plan.echo_routing.len(), 5);
    assert_eq!(plan.echo_routing[0].midi_port, 1);
    assert_eq!(plan.echo_routing[0].channel, 0);
    assert_eq!(plan.echo_routing[4].midi_port, 1);
    assert_eq!(plan.echo_routing[4].channel, 12);
}

#[test]
fn suppression_applies_only_to_program_messages_not_echo_destinations() {
    let browser = fixture_browser();
    let plan = browser.action_plan().unwrap();

    assert!(plan.suppress_program_changes);
    assert!(!plan.external_midi.is_empty());
    assert!(plan.echo_routing.iter().any(|route| {
        route.midi_port == plan.external_midi[0].midi_port
            && Some(route.channel) == Some(plan.external_midi[0].channel)
    }));
}

#[test]
fn synth_patch_plan_preserves_fluidlite_id_and_routing() {
    use freewheeling_plus::fluidsynth::Patch;

    let mut browser = fixture_browser();
    browser.prepend_synth_patches(&[Patch {
        name: "FluidLite test".into(),
        soundfont_id: 42,
        bank: 32,
        program: 7,
        channel: 3,
    }]);
    let plan = browser.action_plan().unwrap();

    assert_eq!(plan.synth.len(), 1);
    assert_eq!(plan.synth[0].soundfont_id, 42);
    assert_eq!(plan.synth[0].bank, 32);
    assert_eq!(plan.synth[0].program, 7);
    assert_eq!(
        plan.echo_routing,
        vec![freewheeling_plus::native_patch_browser::EchoRouting {
            midi_port: 0,
            channel: 3,
            key_range: None,
        }]
    );
}

#[test]
fn selecting_an_empty_or_invalid_index_is_explicit() {
    let mut browser = fixture_browser();
    assert!(browser.select_bank(99).is_none());
    assert!(browser.select_item(999).is_none());
}
