/* Embedded verbatim from src/fweelin_logo.h. */
pub const WIDTH: usize = 223;
pub const HEIGHT: usize = 42;
pub const BYTES_PER_PIXEL: usize = 4;
pub const PIXEL_DATA: &[u8] = include_bytes!("../data/logo.raw");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shape() {
        assert_eq!(PIXEL_DATA.len(), WIDTH * HEIGHT * BYTES_PER_PIXEL + 1);
        assert_eq!(PIXEL_DATA.last(), Some(&0));
    }
    #[test]
    fn checksum() {
        let h = PIXEL_DATA.iter().fold(0x811c9dc5_u32, |h, &b| {
            (h ^ u32::from(b)).wrapping_mul(0x01000193)
        });
        assert_eq!(h, 0x6b347292);
    }
}
