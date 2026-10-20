use crate::extraction::texture::bgra_to_rgba_flipped;

#[test]
fn decoded_rows_are_flipped_and_swizzled() {
    // Unity stores the bottom row first; pixels are BGRA packed little-endian in a u32.
    let bottom = u32::from_le_bytes([3, 2, 1, 255]);
    let top = u32::from_le_bytes([30, 20, 10, 128]);
    let rgba = bgra_to_rgba_flipped(&[bottom, bottom, top, top], 2, 2);
    assert_eq!(&rgba[..8], &[10, 20, 30, 128, 10, 20, 30, 128]);
    assert_eq!(&rgba[8..], &[1, 2, 3, 255, 1, 2, 3, 255]);
}
