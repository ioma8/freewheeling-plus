/* Embedded verbatim from src/fweelin_logo.h. */

/// Logo width in pixels.
pub const WIDTH: usize = 223;
/// Logo height in pixels.
pub const HEIGHT: usize = 42;
/// Bytes per pixel (RGBA8, non-premultiplied, straight alpha).
pub const BYTES_PER_PIXEL: usize = 4;
/// Trailing sentinel byte of the on-disk asset (see [`PIXEL_DATA`]).
pub const SENTINEL_LEN: usize = 1;
/// Raw file contents: `WIDTH * HEIGHT * BYTES_PER_PIXEL` pixel bytes in
/// row-major, top-down order (stride `WIDTH * BYTES_PER_PIXEL`, RGBA8 with
/// straight alpha), followed by [`SENTINEL_LEN`] zero byte that the C++ asset
/// carries.
pub const PIXEL_DATA: &[u8] = include_bytes!("../data/logo.raw");
/// The pixel bytes without the sentinel: a valid flat RGBA buffer for
/// `WIDTH` x `HEIGHT`.
///
/// A function rather than a `const` because slicing a slice is not const
/// evaluation today.
pub fn pixels() -> &'static [u8] {
    &PIXEL_DATA[..WIDTH * HEIGHT * BYTES_PER_PIXEL]
}

/// Compile-time check: a regenerated or truncated asset must fail the build
/// rather than produce a buffer whose length disagrees with the dimensions.
const _: () = assert!(
    PIXEL_DATA.len() == WIDTH * HEIGHT * BYTES_PER_PIXEL + SENTINEL_LEN,
    "data/logo.raw does not match the declared logo dimensions"
);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shape() {
        assert_eq!(
            PIXEL_DATA.len(),
            WIDTH * HEIGHT * BYTES_PER_PIXEL + SENTINEL_LEN
        );
        assert_eq!(PIXEL_DATA.last(), Some(&0));
        assert_eq!(pixels().len(), WIDTH * HEIGHT * BYTES_PER_PIXEL);
    }
    #[test]
    fn checksum() {
        let h = PIXEL_DATA.iter().fold(0x811c9dc5_u32, |h, &b| {
            (h ^ u32::from(b)).wrapping_mul(0x01000193)
        });
        assert_eq!(h, 0x6b347292);
    }
}
