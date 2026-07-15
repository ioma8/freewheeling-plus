use freewheeling_plus::core_persistence_parse as parse;

#[test]
fn parser_module_is_compiled() {
    let parsed = parse::parse_scene("<scene/>").unwrap();
    assert!(parsed.loops.is_empty());
}
