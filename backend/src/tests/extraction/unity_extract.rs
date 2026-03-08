use crate::extraction::unity_extract::parse_text_asset;

#[test]
fn parse_text_asset_valid() {
    // Build: u32 name_len + name + padding + u32 script_len + script
    let name = b"test.json";
    let script = b"{\"key\": \"value\"}";

    let name_len = name.len() as u32;
    let aligned = (4 + name.len() + 3) & !3; // align to 4
    let script_len = script.len() as u32;
    let total = aligned + 4 + script.len();

    let mut data = vec![0u8; total];
    data[0..4].copy_from_slice(&name_len.to_le_bytes());
    data[4..4 + name.len()].copy_from_slice(name);
    data[aligned..aligned + 4].copy_from_slice(&script_len.to_le_bytes());
    data[aligned + 4..aligned + 4 + script.len()].copy_from_slice(script);

    let (parsed_name, parsed_script) = parse_text_asset(&data).unwrap();
    assert_eq!(parsed_name, "test.json");
    assert_eq!(parsed_script, script);
}

#[test]
fn parse_text_asset_too_short() {
    assert!(parse_text_asset(&[0; 4]).is_none());
    assert!(parse_text_asset(&[]).is_none());
}

#[test]
fn parse_text_asset_name_too_long() {
    // name_len > 10_000 is rejected
    let mut data = vec![0u8; 20];
    data[0..4].copy_from_slice(&10_001u32.to_le_bytes());
    assert!(parse_text_asset(&data).is_none());
}

#[test]
fn parse_text_asset_name_extends_beyond_data() {
    let mut data = vec![0u8; 10];
    data[0..4].copy_from_slice(&100u32.to_le_bytes()); // name_len=100, data only 10 bytes
    assert!(parse_text_asset(&data).is_none());
}

#[test]
fn parse_text_asset_empty_name() {
    // name_len = 0, aligned = 4, then script
    let script = b"content";
    let script_len = script.len() as u32;
    let total = 4 + 4 + script.len();
    let mut data = vec![0u8; total];
    data[0..4].copy_from_slice(&0u32.to_le_bytes()); // name_len=0
    data[4..8].copy_from_slice(&script_len.to_le_bytes());
    data[8..8 + script.len()].copy_from_slice(script);

    let (name, content) = parse_text_asset(&data).unwrap();
    assert_eq!(name, "");
    assert_eq!(content, script);
}

#[test]
fn parse_text_asset_empty_script() {
    let name = b"file";
    let name_len = name.len() as u32;
    let aligned = (4 + name.len() + 3) & !3;
    let total = aligned + 4;
    let mut data = vec![0u8; total];
    data[0..4].copy_from_slice(&name_len.to_le_bytes());
    data[4..4 + name.len()].copy_from_slice(name);
    data[aligned..aligned + 4].copy_from_slice(&0u32.to_le_bytes()); // script_len=0

    let (parsed_name, script) = parse_text_asset(&data).unwrap();
    assert_eq!(parsed_name, "file");
    assert!(script.is_empty());
}

#[test]
fn parse_text_asset_script_too_large() {
    let name = b"x";
    let name_len = name.len() as u32;
    let aligned = (4 + name.len() + 3) & !3;
    let total = aligned + 4;
    let mut data = vec![0u8; total];
    data[0..4].copy_from_slice(&name_len.to_le_bytes());
    data[4..4 + name.len()].copy_from_slice(name);
    // script_len = 100_000_001 (exceeds 100M limit)
    data[aligned..aligned + 4].copy_from_slice(&100_000_001u32.to_le_bytes());
    assert!(parse_text_asset(&data).is_none());
}
