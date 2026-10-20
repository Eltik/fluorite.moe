use crate::extraction::memorypack::{Schema, StringPool, decode_bytes, json_files};
use crate::extraction::memorypack_schema::{generate_schema, gzip, memorypack_source, words};
use std::collections::HashSet;
use std::path::Path;

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn splits_words_like_the_python_learner() {
    // `[A-Z]?[a-z]+|[A-Z]+(?![a-z])|\d+`, lowercased.
    assert_eq!(words("LevelScriptData"), set(&["level", "script", "data"]));
    assert_eq!(
        words("NPCPrefabInfo2"),
        set(&["npc", "prefab", "info", "2"])
    );
    assert_eq!(
        words("LevelData base01_lv001.json"),
        set(&["level", "data", "base", "01", "lv", "001", "json"])
    );
    assert_eq!(words("AI"), set(&["ai"]));
    assert!(words("_ /.").is_empty());
}

#[test]
fn rejects_non_il2cpp_input() {
    let error = generate_schema(b"MZ", b"not metadata", "0", None)
        .err()
        .unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn gzip_round_trips() {
    let json = br#"{"format":1}"#;
    let mut out = Vec::new();
    std::io::Read::read_to_end(
        &mut flate2::read::GzDecoder::new(&gzip(json).unwrap()[..]),
        &mut out,
    )
    .unwrap();
    assert_eq!(out, json);
}

/// Generate the schema from real client files, learning `dir_roots` from an extracted tree,
/// and compare every decoded file with the Python reference decoder's output.
///
/// `IL2CPP_DLL`, `IL2CPP_METADATA`: the 1.6.4 client files. `MEMORYPACK_INPUT`: the extracted
/// `JsonData/Json` directory (only read; `.mpk` sources are preferred). `MEMORYPACK_ORACLE`:
/// the reference's `<relpath>.json` for every file it decoded.
#[test]
#[ignore]
fn generated_schema_matches_reference_decoder() {
    let (Ok(dll), Ok(meta), Ok(input), Ok(oracle)) = (
        std::env::var("IL2CPP_DLL"),
        std::env::var("IL2CPP_METADATA"),
        std::env::var("MEMORYPACK_INPUT"),
        std::env::var("MEMORYPACK_ORACLE"),
    ) else {
        panic!("set IL2CPP_DLL, IL2CPP_METADATA, MEMORYPACK_INPUT and MEMORYPACK_ORACLE");
    };
    let (dll, meta) = (std::fs::read(dll).unwrap(), std::fs::read(meta).unwrap());
    let (input, oracle) = (Path::new(&input), Path::new(&oracle));
    let (json, report) = generate_schema(&dll, &meta, "1.6.4", Some(input)).unwrap();
    assert_eq!(report.typedefs, 91478);
    assert_eq!(report.registration_va, 0x1_8da6_0e20);
    assert_eq!(report.roots, 4018);

    let schema = Schema::from_json(&json).unwrap();
    let pool = StringPool::load(&input.join("StringPool")).unwrap();
    let (mut equal, mut mismatched, mut lost, mut gained) = (0, Vec::new(), Vec::new(), 0);
    for (path, rel_dir) in json_files(input).unwrap() {
        let Some(data) = memorypack_source(&path).unwrap() else {
            continue;
        };
        let expected: Option<serde_json::Value> =
            std::fs::read(oracle.join(path.strip_prefix(input).unwrap()))
                .ok()
                .map(|b| serde_json::from_slice(&b).unwrap());
        let actual: Option<serde_json::Value> = decode_bytes(&schema, &pool, &data, &rel_dir)
            .map(|b| serde_json::from_slice(&b).unwrap());
        match (actual, expected) {
            (Some(a), Some(e)) if a == e => equal += 1,
            (Some(_), Some(_)) => mismatched.push(path.display().to_string()),
            (None, Some(_)) => lost.push(path.display().to_string()),
            (Some(_), None) => gained += 1,
            (None, None) => {}
        }
    }
    println!("{equal} equal, {gained} newly decoded");
    assert!(mismatched.is_empty(), "{mismatched:?}");
    assert!(lost.is_empty(), "{lost:?}");
    assert_eq!(equal, 19471);
}
