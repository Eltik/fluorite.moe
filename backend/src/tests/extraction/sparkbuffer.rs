use crate::extraction::sparkbuffer::parse_sparkbuffer;

#[test]
fn parse_sparkbuffer_too_small() {
    let result = parse_sparkbuffer(&[0; 8]);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn parse_sparkbuffer_exactly_12_bytes_invalid() {
    // 12 bytes is minimum size but the offsets will be garbage
    let result = parse_sparkbuffer(&[0; 12]);
    assert!(result.is_err());
}

#[test]
fn parse_sparkbuffer_empty() {
    let result = parse_sparkbuffer(&[]);
    assert!(result.is_err());
}
