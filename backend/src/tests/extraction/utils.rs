use crate::extraction::utils::{format_bytes, hex_dump_string, sanitize_filename};

#[test]
fn format_bytes_zero() {
    assert_eq!(format_bytes(0), "0 B");
}

#[test]
fn format_bytes_small() {
    assert_eq!(format_bytes(512), "512.00 B");
}

#[test]
fn format_bytes_kilobytes() {
    assert_eq!(format_bytes(1024), "1.00 KB");
    assert_eq!(format_bytes(1536), "1.50 KB");
}

#[test]
fn format_bytes_megabytes() {
    assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
}

#[test]
fn format_bytes_gigabytes() {
    assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
}

#[test]
fn format_bytes_terabytes() {
    assert_eq!(format_bytes(1024u64 * 1024 * 1024 * 1024), "1.00 TB");
}

#[test]
fn format_bytes_large_terabytes_stays_tb() {
    // Should not overflow beyond TB
    assert_eq!(format_bytes(5 * 1024u64 * 1024 * 1024 * 1024), "5.00 TB");
}

#[test]
fn sanitize_filename_no_change() {
    assert_eq!(sanitize_filename("normal_file.txt"), "normal_file.txt");
}

#[test]
fn sanitize_filename_replaces_invalid_chars() {
    assert_eq!(
        sanitize_filename("a\\b:c*d?e\"f<g>h|i"),
        "a_b_c_d_e_f_g_h_i"
    );
}

#[test]
fn sanitize_filename_empty() {
    assert_eq!(sanitize_filename(""), "");
}

#[test]
fn sanitize_filename_all_invalid() {
    assert_eq!(sanitize_filename("\\:*?\"<>|"), "________");
}

#[test]
fn hex_dump_string_empty() {
    assert_eq!(hex_dump_string(&[], 16), "");
}

#[test]
fn hex_dump_string_single_row() {
    let data = [0x48, 0x65, 0x6C, 0x6C, 0x6F]; // "Hello"
    let result = hex_dump_string(&data, 16);
    assert!(result.contains("0000:"));
    assert!(result.contains("48 65 6c 6c 6f"));
    assert!(result.contains("Hello"));
}

#[test]
fn hex_dump_string_max_bytes_limits_output() {
    let data = vec![0xAA; 32];
    let limited = hex_dump_string(&data, 8);
    let full = hex_dump_string(&data, 32);
    assert!(limited.len() < full.len());
}

#[test]
fn hex_dump_string_non_printable_shown_as_dot() {
    let data = [0x00, 0x01, 0x7F]; // non-printable bytes
    let result = hex_dump_string(&data, 16);
    assert!(result.contains("..."));
}
