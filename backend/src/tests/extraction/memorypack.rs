use crate::extraction::memorypack::{
    Schema, SchemaSet, StringPool, TreeDecodeResult, decode_bytes, decode_tree_set,
    decode_tree_with, json_files, newest_first,
};
use crate::extraction::memorypack_schema::memorypack_source;
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn temp_dir(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fluorite-mp-{label}-{}-{nonce}",
        std::process::id()
    ))
}

/// One string pool file: optional 4-byte file count, the `USPC` header, then the block.
fn pool_file(strings: &[&str], first: bool, compress: bool) -> Vec<u8> {
    let mut offsets = Vec::new();
    let mut body = Vec::new();
    for s in strings {
        offsets.push(body.len() as i32);
        body.extend_from_slice(&(s.len() as i32).to_le_bytes());
        body.extend_from_slice(s.as_bytes());
    }
    let mut raw = (strings.len() as i32).to_le_bytes().to_vec();
    offsets
        .iter()
        .for_each(|o| raw.extend_from_slice(&o.to_le_bytes()));
    raw.extend_from_slice(&body);
    let stored = if compress {
        lz4_flex::block::compress(&raw)
    } else {
        raw.clone()
    };
    let mut file = Vec::new();
    if first {
        file.extend_from_slice(&2i32.to_le_bytes());
    }
    let header = [
        0x4350_5355,
        1,
        i32::from(compress),
        strings.len() as i32,
        raw.len() as i32,
        stored.len() as i32,
    ];
    header
        .iter()
        .for_each(|v| file.extend_from_slice(&v.to_le_bytes()));
    file.extend_from_slice(&stored);
    file
}

#[test]
fn loads_plain_and_lz4_pool_files() {
    let long = "repeated ".repeat(40);
    let files = [
        pool_file(&["alpha", "", "βeta"], true, false),
        pool_file(&[&long, "gamma"], false, true),
    ];
    let pool = StringPool::from_files(&files).unwrap();
    assert_eq!(pool.len(), 5);
    assert_eq!(pool.get(0), Some("alpha"));
    assert_eq!(pool.get(1), Some(""));
    assert_eq!(pool.get(2), Some("βeta"));
    assert_eq!(pool.get(3), Some(long.as_str()));
    assert_eq!(pool.get(4), Some("gamma"));

    let dir = temp_dir("pool");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("StringPool0.byte"), &files[0]).unwrap();
    fs::write(dir.join("StringPool1.byte"), &files[1]).unwrap();
    assert_eq!(StringPool::load(&dir).unwrap(), pool);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rejects_malformed_pool() {
    let mut file = pool_file(&["alpha"], true, false);
    assert!(StringPool::from_files(&[&file[..file.len() - 1]]).is_err());
    file[4] ^= 1;
    assert!(StringPool::from_files(&[&file]).is_err());
}

/// Every node kind but gradient/rect_offset, rooted at `Root`; `A` and `B` are one-member
/// objects told apart only by `dir_roots`.
const SCHEMA: &str = r#"{
  "format": 1,
  "game_version": "test",
  "nodes": [
    {"k": "p", "t": "i32"},
    {"k": "str"},
    {"k": "p", "t": "f32"},
    {"k": "p", "t": "bool"},
    {"k": "obj", "m": [["v", 0]]},
    {"k": "union", "c": {"0": ["CaseA", 4], "300": ["Wide", 0]}},
    {"k": "list", "e": 5},
    {"k": "raw", "size": 8, "f": [[4, "y", 2], [0, "x", 0]]},
    {"k": "nullable", "off": 4, "v": 0},
    {"k": "dict", "hdr": false, "key": 1, "val": 0},
    {"k": "dict", "hdr": true, "key": 3, "val": 0},
    {"k": "list", "e": 0},
    {"k": "curve"},
    {"k": "ref", "to": 0},
    {"k": "obj", "m": [["a", 13], ["s", 1], ["us", 6], ["r", 7], ["n", 8], ["nn", 8],
                       ["d1", 9], ["d2", 10], ["l", 11], ["c", 12]]},
    {"k": "unsupported", "why": "test"},
    {"k": "obj", "m": [["s", 1]]},
    {"k": "obj", "m": [["bad", 15]]}
  ],
  "roots": {"Root": 14, "A": 4, "B": 16, "Bad": 17, "U": 5},
  "dir_roots": {"dirB": ["B"]}
}"#;

fn schema() -> Schema {
    Schema::from_json(SCHEMA.as_bytes()).unwrap()
}

fn pool() -> StringPool {
    StringPool::new(vec!["alpha".into(), "beta".into()])
}

#[derive(Default)]
struct Bytes(Vec<u8>);

impl Bytes {
    fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    fn u16(mut self, v: u16) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i32(mut self, v: i32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f32(mut self, v: f32) -> Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
}

fn root_payload() -> Vec<u8> {
    Bytes::default()
        .u8(10)
        .i32(7) // a
        .i32(1) // s
        .i32(2) // us: narrow tag, then wide tag 300
        .u8(0)
        .u8(1)
        .i32(5)
        .u8(250)
        .u16(300)
        .i32(-3)
        .i32(2) // r: x, then y at offset 4
        .f32(0.1)
        .u8(1) // n: has value, padded to offset 4
        .u8(0)
        .u8(0)
        .u8(0)
        .i32(9)
        .u8(0) // nn: no value, but the slot is still read
        .u8(0)
        .u8(0)
        .u8(0)
        .i32(9)
        .i32(2) // d1: string keys, the second empty
        .i32(0)
        .i32(1)
        .i32(-2)
        .i32(2)
        .u8(0) // d2: header, then bool keys
        .i32(2)
        .u8(1)
        .i32(1)
        .u8(0)
        .i32(2)
        .i32(-1) // l: null list
        .u8(0) // c: curve with one key
        .i32(1)
        .i32(2)
        .i32(1)
        .f32(0.5)
        .f32(f32::NAN)
        .f32(0.0)
        .f32(-0.0)
        .i32(3)
        .f32(1.0 / 3.0)
        .f32(1.0)
        .0
}

const ROOT_JSON: &str = r#"{
  "a": 7,
  "s": "beta",
  "us": [
    {
      "$type": "CaseA",
      "v": 5
    },
    -3
  ],
  "r": {
    "y": 0.1,
    "x": 2
  },
  "n": 9,
  "nn": null,
  "d1": {
    "alpha": 1,
    "": 2
  },
  "d2": [
    [
      true,
      1
    ],
    [
      false,
      2
    ]
  ],
  "l": null,
  "c": {
    "postWrapMode": 1,
    "preWrapMode": 2,
    "keys": [
      {
        "time": 0.5,
        "value": null,
        "inTangent": 0.0,
        "outTangent": -0.0,
        "weightedMode": 3,
        "inWeight": 0.33333334,
        "outWeight": 1.0
      }
    ]
  }
}"#;

#[test]
fn decodes_every_node_kind_in_schema_order() {
    let out = decode_bytes(&schema(), &pool(), &root_payload(), ".").unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), ROOT_JSON);
}

#[test]
fn rejects_partial_or_overlong_reads() {
    let (schema, pool) = (schema(), pool());
    let mut payload = root_payload();
    payload.push(0);
    assert_eq!(decode_bytes(&schema, &pool, &payload, "."), None);
    payload.truncate(payload.len() - 2);
    assert_eq!(decode_bytes(&schema, &pool, &payload, "."), None);
    assert_eq!(decode_bytes(&schema, &pool, &[], "."), None);
    // `B` fails on an out-of-pool string index, so the next candidate takes it.
    let bad_string = Bytes::default().u8(1).i32(2).0;
    assert_eq!(
        decode_bytes(&schema, &pool, &bad_string, "dirB").unwrap(),
        b"{\n  \"v\": 2\n}"
    );
}

#[test]
fn picks_roots_by_directory_then_name() {
    let (schema, pool) = (schema(), pool());
    let payload = Bytes::default().u8(1).i32(0).0;
    // `A` sorts before `B` (and `Bad` fails on its unsupported member).
    let by_name = decode_bytes(&schema, &pool, &payload, ".").unwrap();
    assert_eq!(by_name, b"{\n  \"v\": 0\n}");
    let by_dir = decode_bytes(&schema, &pool, &payload, "dirB").unwrap();
    assert_eq!(by_dir, b"{\n  \"s\": \"alpha\"\n}");
}

#[test]
fn falls_back_to_union_roots() {
    let (schema, pool) = (schema(), pool());
    // Tag 0 is not a plausible object header for any 0-member object, so `U` decodes it.
    let narrow = Bytes::default().u8(0).u8(1).i32(4).0;
    assert_eq!(
        decode_bytes(&schema, &pool, &narrow, ".").unwrap(),
        b"{\n  \"$type\": \"CaseA\",\n  \"v\": 4\n}"
    );
    let wide = Bytes::default().u8(250).u16(300).i32(8).0;
    assert_eq!(decode_bytes(&schema, &pool, &wide, ".").unwrap(), b"8");
    let unknown = Bytes::default().u8(250).u16(301).i32(8).0;
    assert_eq!(decode_bytes(&schema, &pool, &unknown, "."), None);
}

#[test]
fn dict_with_colliding_keys_becomes_pairs() {
    let schema = Schema::from_json(
        br#"{"format": 1, "nodes": [{"k": "p", "t": "i64"}, {"k": "str"},
            {"k": "dict", "hdr": false, "key": 0, "val": 1},
            {"k": "obj", "m": [["d", 2]]}],
            "roots": {"D": 3}, "dir_roots": {}}"#,
    )
    .unwrap();
    let pool = StringPool::new(vec!["x".into()]);
    let unique = Bytes::default()
        .u8(1)
        .i32(2)
        .i32(-5)
        .i32(-1)
        .i32(0)
        .i32(0)
        .i32(0)
        .i32(-1);
    assert_eq!(
        String::from_utf8(decode_bytes(&schema, &pool, &unique.0, ".").unwrap()).unwrap(),
        "{\n  \"d\": {\n    \"-5\": \"x\",\n    \"0\": null\n  }\n}"
    );
    let repeated = Bytes::default()
        .u8(1)
        .i32(2)
        .i32(3)
        .i32(0)
        .i32(0)
        .i32(3)
        .i32(0)
        .i32(-2);
    assert_eq!(
        String::from_utf8(decode_bytes(&schema, &pool, &repeated.0, ".").unwrap()).unwrap(),
        "{\n  \"d\": [\n    [\n      3,\n      \"x\"\n    ],\n    [\n      3,\n      \"\"\n    ]\n  ]\n}"
    );
}

#[test]
fn rejects_runaway_recursion() {
    let schema = Schema::from_json(
        br#"{"format": 1, "nodes": [{"k": "ref", "to": 0}], "roots": {}, "dir_roots": {"": []}}"#,
    )
    .unwrap();
    // No roots at all: nothing decodes, and nothing loops.
    assert_eq!(
        decode_bytes(&schema, &StringPool::default(), &[1], "."),
        None
    );
    let looping = Schema::from_json(
        br#"{"format": 1, "nodes": [{"k": "obj", "m": [["self", 1]]}, {"k": "ref", "to": 1}],
            "roots": {"Loop": 0}, "dir_roots": {}}"#,
    )
    .unwrap();
    assert_eq!(
        decode_bytes(&looping, &StringPool::default(), &[1], "."),
        None
    );
}

#[test]
fn decode_tree_is_idempotent() {
    let group = temp_dir("tree");
    let json = group.join("Json");
    fs::create_dir_all(json.join("StringPool")).unwrap();
    fs::create_dir_all(json.join("dirB")).unwrap();
    fs::write(
        json.join("StringPool/StringPool0.byte"),
        pool_file(&["alpha", "beta"], true, true),
    )
    .unwrap();
    let source = Bytes::default().u8(1).i32(1).0;
    fs::write(json.join("dirB/Foo.json"), &source).unwrap();
    fs::write(json.join("Plain.json"), b"{\"k\": 1}").unwrap();
    fs::write(json.join("Empty.json"), b"").unwrap();
    fs::write(json.join("Bad.json"), [7u8, 1, 2]).unwrap();

    let schema = schema();
    let expected = TreeDecodeResult {
        decoded: 1,
        failed: 1,
        write_errors: 0,
        by_schema: [("test".to_owned(), 1)].into(),
    };
    assert_eq!(decode_tree_with(&schema, &group).unwrap(), expected);
    let decoded = fs::read(json.join("dirB/Foo.json")).unwrap();
    assert_eq!(decoded, b"{\n  \"s\": \"beta\"\n}");
    assert_eq!(fs::read(json.join("dirB/Foo.mpk")).unwrap(), source);
    assert_eq!(fs::read(json.join("Plain.json")).unwrap(), b"{\"k\": 1}");
    assert_eq!(fs::read(json.join("Bad.json")).unwrap(), [7u8, 1, 2]);
    assert!(!json.join("Bad.mpk").exists());

    // The second run decodes from `.mpk` and lands on the same bytes.
    assert_eq!(decode_tree_with(&schema, &group).unwrap(), expected);
    assert_eq!(fs::read(json.join("dirB/Foo.json")).unwrap(), decoded);
    assert_eq!(fs::read(json.join("dirB/Foo.mpk")).unwrap(), source);
    fs::remove_dir_all(group).unwrap();
}

#[test]
fn decode_tree_without_pool_is_a_no_op() {
    let group = temp_dir("nopool");
    fs::create_dir_all(group.join("Json")).unwrap();
    fs::write(group.join("Json/Foo.json"), [1u8, 0, 0, 0, 0]).unwrap();
    assert_eq!(
        decode_tree_with(&schema(), &group).unwrap(),
        TreeDecodeResult::default()
    );
    assert_eq!(
        decode_tree_with(&schema(), &group.join("missing")).unwrap(),
        TreeDecodeResult::default()
    );
    fs::remove_dir_all(group).unwrap();
}

#[test]
fn embedded_schema_parses() {
    Schema::embedded().unwrap();
}

/// Decode a real `Json/` tree in memory and compare with the Python reference's output.
///
/// `MEMORYPACK_INPUT` is the extracted `JsonData/Json` directory (only read), and
/// `MEMORYPACK_ORACLE` holds the reference's `<relpath>.json` for every decoded file.
#[test]
#[ignore]
fn matches_reference_decoder() {
    let (Ok(input), Ok(oracle)) = (
        std::env::var("MEMORYPACK_INPUT"),
        std::env::var("MEMORYPACK_ORACLE"),
    ) else {
        panic!("set MEMORYPACK_INPUT and MEMORYPACK_ORACLE");
    };
    let (input, oracle) = (Path::new(&input), Path::new(&oracle));
    let schema = Schema::embedded().unwrap();
    let pool = StringPool::load(&input.join("StringPool")).unwrap();
    let started = std::time::Instant::now();
    let (mut ok, mut failed, mut mismatched) = (0, 0, Vec::new());
    let mut gained = 0;
    for (path, rel_dir) in json_files(input).unwrap() {
        // `.mpk` sources first, so an already decoded tree works too.
        let Some(data) = memorypack_source(&path).unwrap() else {
            continue;
        };
        let expected_path = oracle.join(path.strip_prefix(input).unwrap());
        let expected: Option<serde_json::Value> = fs::read(&expected_path)
            .ok()
            .map(|bytes| serde_json::from_slice(&bytes).unwrap());
        let actual: Option<serde_json::Value> = decode_bytes(schema, &pool, &data, &rel_dir)
            .map(|bytes| serde_json::from_slice(&bytes).unwrap());
        match &actual {
            Some(_) => ok += 1,
            None => failed += 1,
        }
        match (&actual, &expected) {
            // Files the reference could not decode (padded pairs, among others).
            (Some(_), None) => gained += 1,
            (a, e) if a != e => mismatched.push(path.display().to_string()),
            _ => {}
        }
    }
    println!(
        "decoded {ok} ({gained} beyond the reference), failed {failed}, mismatched {} in {:?}",
        mismatched.len(),
        started.elapsed()
    );
    assert!(
        mismatched.is_empty(),
        "first mismatches: {:?}",
        &mismatched[..mismatched.len().min(10)]
    );
    assert_eq!(
        (ok - gained, ok, failed),
        (19471, EMBEDDED_DECODED, EMBEDDED_FAILED)
    );
}

/// Files of the 1.6.4 JsonData group the embedded schema decodes and fails on.
const EMBEDDED_DECODED: u64 = 19485;
const EMBEDDED_FAILED: u64 = 17;

/// Decode a copy of a real JsonData group in place twice; the second run must change nothing.
///
/// `MEMORYPACK_GROUP` is the group folder (the parent of `Json/`) and is rewritten, so point
/// it at a copy.
#[test]
#[ignore]
fn decodes_real_group_idempotently() {
    let Ok(group) = std::env::var("MEMORYPACK_GROUP") else {
        panic!("set MEMORYPACK_GROUP to a copy of a JsonData group");
    };
    let group = Path::new(&group);
    let snapshot = || -> Vec<(PathBuf, Vec<u8>)> {
        let mut files: Vec<_> = json_files(&group.join("Json"))
            .unwrap()
            .into_iter()
            .map(|(path, _)| {
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        files.sort();
        files
    };
    let schema = Schema::embedded().unwrap();
    for run in 1..=2 {
        let before = (run == 2).then(snapshot);
        let started = std::time::Instant::now();
        let result = decode_tree_with(schema, group).unwrap();
        println!("run {run}: {result:?} in {:?}", started.elapsed());
        assert_eq!(
            (result.decoded, result.failed, result.write_errors),
            (EMBEDDED_DECODED, EMBEDDED_FAILED, 0)
        );
        if let Some(before) = before {
            assert!(before == snapshot(), "second run changed the tree");
        }
    }
}

#[test]
fn decodes_padded_key_value_pairs() {
    // `Dictionary<ulong, int>`: raw 16-byte entries, the value at 8, then 4 padding bytes.
    let schema = Schema::from_json(
        br#"{"format": 1, "nodes": [{"k": "p", "t": "u64"}, {"k": "p", "t": "i32"},
            {"k": "dict", "hdr": false, "key": 0, "val": 1, "kv": [8, 16]},
            {"k": "obj", "m": [["d", 2]]}],
            "roots": {"D": 3}, "dir_roots": {}}"#,
    )
    .unwrap();
    let pool = StringPool::default();
    let mut payload = Bytes::default().u8(1).i32(2).0;
    for (key, value) in [(5u64, 7i32), (u64::MAX, -1)] {
        payload.extend_from_slice(&key.to_le_bytes());
        payload.extend_from_slice(&value.to_le_bytes());
        payload.extend_from_slice(&[0xAA; 4]);
    }
    assert_eq!(
        String::from_utf8(decode_bytes(&schema, &pool, &payload, ".").unwrap()).unwrap(),
        "{\n  \"d\": {\n    \"5\": 7,\n    \"18446744073709551615\": -1\n  }\n}"
    );
    // The last entry's padding is part of the data.
    payload.truncate(payload.len() - 1);
    assert_eq!(decode_bytes(&schema, &pool, &payload, "."), None);

    // A value offset of 0 or past the entry cannot be a padded pair.
    for kv in ["[0, 16]", "[16, 16]", "[8]", "\"x\""] {
        let json = format!(
            r#"{{"format": 1, "nodes": [{{"k": "p", "t": "u64"}},
                {{"k": "dict", "hdr": false, "key": 0, "val": 0, "kv": {kv}}}],
                "roots": {{}}, "dir_roots": {{}}}}"#
        );
        assert!(Schema::from_json(json.as_bytes()).is_err(), "kv {kv}");
    }
}

/// A one-root schema whose root object has a single member `name` of node `kind`.
fn versioned(version: Option<&str>, generated_at: Option<&str>, name: &str, kind: &str) -> Schema {
    let kind: serde_json::Value = serde_json::from_str(kind).unwrap();
    let mut doc = serde_json::json!({
        "format": 1,
        "nodes": [{"k": "obj", "m": [[name, 1]]}, kind],
        "roots": {"R": 0},
        "dir_roots": {},
    });
    if let Some(version) = version {
        doc["game_version"] = version.into();
    }
    if let Some(at) = generated_at {
        doc["generated_at"] = at.into();
    }
    Schema::from_json(&serde_json::to_vec(&doc).unwrap()).unwrap()
}

#[test]
fn orders_schemas_newest_first() {
    let schema = |v, at| versioned(v, at, "x", r#"{"k": "str"}"#);
    let old = schema(Some("1.6.4"), Some("2026-10-01T00:00:00+00:00"));
    let rebuilt = schema(Some("1.6.4"), Some("2026-10-02T00:00:00+09:00"));
    let newer = schema(Some("1.10.0"), Some("2026-01-01T00:00:00Z"));
    let unknown = schema(None, None);
    // Versions compare numerically (1.10 > 1.6), then generation time.
    assert_eq!(newest_first(&newer, &old), Ordering::Less);
    assert_eq!(newest_first(&rebuilt, &old), Ordering::Less);
    assert_eq!(newest_first(&unknown, &old), Ordering::Greater);

    let set = SchemaSet::new(
        vec![
            ("old".into(), Arc::new(old)),
            ("unknown".into(), Arc::new(unknown)),
            ("newer".into(), Arc::new(newer)),
            ("rebuilt".into(), Arc::new(rebuilt)),
        ],
        Some(("fallback".into(), Arc::new(schema(Some("9.9.9"), None)))),
    );
    let labels: Vec<&str> = set.entries().map(|(label, _)| label).collect();
    // The fallback stays last whatever its version.
    assert_eq!(labels, ["newer", "rebuilt", "old", "unknown", "fallback"]);
    assert_eq!(set.newest().unwrap().0, "newer");
    assert_eq!(set.len(), 5);

    let replaced = set.with("old", Arc::new(schema(Some("2.0.0"), None)));
    let labels: Vec<&str> = replaced.entries().map(|(label, _)| label).collect();
    assert_eq!(labels, ["old", "newer", "rebuilt", "unknown", "fallback"]);
}

#[test]
fn schema_set_falls_back_to_older_schemas() {
    // The new schema reads the member as a string, the old one as an int.
    let new = versioned(Some("1.7.0"), None, "name", r#"{"k": "str"}"#);
    let old = versioned(Some("1.6.4"), None, "id", r#"{"k": "p", "t": "i32"}"#);
    let set = SchemaSet::new(
        vec![("old".into(), Arc::new(old)), ("new".into(), Arc::new(new))],
        None,
    );
    let pool = pool();
    let both = Bytes::default().u8(1).i32(1).0;
    let (label, json) = set.decode(&pool, &both, ".").unwrap();
    assert_eq!(
        (label, json.as_slice()),
        ("new", &b"{\n  \"name\": \"beta\"\n}"[..])
    );
    // Index 7 is outside the pool: only the old schema decodes it.
    let old_only = Bytes::default().u8(1).i32(7).0;
    let (label, json) = set.decode(&pool, &old_only, ".").unwrap();
    assert_eq!((label, json.as_slice()), ("old", &b"{\n  \"id\": 7\n}"[..]));
    assert_eq!(set.decode(&pool, &[2, 0], "."), None);

    // A tree counts each file under the schema that decoded it.
    let group = temp_dir("set");
    let json = group.join("Json");
    fs::create_dir_all(json.join("StringPool")).unwrap();
    fs::write(
        json.join("StringPool/StringPool0.byte"),
        pool_file(&["alpha", "beta"], true, false),
    )
    .unwrap();
    fs::write(json.join("Both.json"), &both).unwrap();
    fs::write(json.join("OldOnly.json"), &old_only).unwrap();
    fs::write(json.join("Neither.json"), [2u8, 0]).unwrap();
    let expected = TreeDecodeResult {
        decoded: 2,
        failed: 1,
        write_errors: 0,
        by_schema: [("new".to_owned(), 1), ("old".to_owned(), 1)].into(),
    };
    assert_eq!(decode_tree_set(&set, &group).unwrap(), expected);
    assert_eq!(decode_tree_set(&set, &group).unwrap(), expected);
    assert_eq!(fs::read(json.join("OldOnly.mpk")).unwrap(), old_only);
    fs::remove_dir_all(group).unwrap();
}

#[test]
fn embedded_set_holds_only_the_embedded_schema() {
    let set = SchemaSet::embedded().unwrap();
    let (label, schema) = set.newest().unwrap();
    assert_eq!(
        label,
        format!("embedded-{}", schema.game_version().unwrap())
    );
    assert_eq!(set.len(), 1);
}
