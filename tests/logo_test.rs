use freewheeling_plus::logo;

#[test]
fn logo_public_metadata_matches_embedded_payload() {
    assert_eq!(logo::WIDTH, 223);
    assert_eq!(logo::HEIGHT, 42);
    assert_eq!(logo::BYTES_PER_PIXEL, 4);
    assert_eq!(
        logo::PIXEL_DATA.len(),
        logo::WIDTH * logo::HEIGHT * logo::BYTES_PER_PIXEL + 1
    );
    assert_eq!(logo::PIXEL_DATA.last(), Some(&0));
}

#[test]
fn logo_public_payload_has_stable_fnv1a_checksum() {
    let checksum = logo::PIXEL_DATA.iter().fold(0x811c9dc5_u32, |hash, &byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x01000193)
    });
    assert_eq!(checksum, 0x6b347292);
}
