use crate::extraction::texture::{bgra_to_rgba_flipped, resolve_stream_data};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn decoded_rows_are_flipped_and_swizzled() {
    // Unity stores the bottom row first; pixels are BGRA packed little-endian in a u32.
    let bottom = u32::from_le_bytes([3, 2, 1, 255]);
    let top = u32::from_le_bytes([30, 20, 10, 128]);
    let rgba = bgra_to_rgba_flipped(&[bottom, bottom, top, top], 2, 2);
    assert_eq!(&rgba[..8], &[10, 20, 30, 128, 10, 20, 30, 128]);
    assert_eq!(&rgba[8..], &[1, 2, 3, 255, 1, 2, 3, 255]);
}

#[test]
fn stream_data_is_sliced_from_res_s() {
    let resources = HashMap::from([("CAB-1.resS".to_string(), (0u8..10).collect::<Vec<_>>())]);
    let stream = |offset: u64, size: u64| {
        json!({ "m_StreamData": {
            "path": "archive:/CAB-1/CAB-1.resS", "offset": offset, "size": size,
        }})
    };
    assert_eq!(
        resolve_stream_data(&stream(2, 3), &resources).unwrap(),
        [2, 3, 4]
    );
    assert_eq!(
        resolve_stream_data(&stream(0, 10), &resources)
            .unwrap()
            .len(),
        10
    );
    // Truncated streams are missing data, not a shorter buffer.
    assert!(resolve_stream_data(&stream(8, 3), &resources).is_none());
    assert!(resolve_stream_data(&stream(10, 1), &resources).is_none());
    assert!(resolve_stream_data(&stream(1, u64::MAX), &resources).is_none());
}
