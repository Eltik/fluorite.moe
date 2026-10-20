use crate::extraction::memorypack::{self, Schema, SchemaSet, json_files};
use crate::extraction::memorypack_schema::{gzip, memorypack_source};
use crate::extraction::memorypack_store::{
    self, Decision, SyncInput, SyncOutcome, Validation, decide, install, load_set, load_store,
    store_dir, store_file_name, validate,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn temp_dir(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fluorite-mpstore-{label}-{}-{nonce}",
        std::process::id()
    ))
}

/// A schema whose one root reads a single member `name` of node `kind` (JSON).
fn schema_json(version: &str, generated_at: &str, name: &str, kind: &str) -> Vec<u8> {
    let kind: serde_json::Value = serde_json::from_str(kind).unwrap();
    serde_json::to_vec(&serde_json::json!({
        "format": 1,
        "game_version": version,
        "generated_at": generated_at,
        "client_crc32": "0000abcd",
        "nodes": [{"k": "obj", "m": [[name, 1]]}, kind],
        "roots": {"R": 0},
        "dir_roots": {},
    }))
    .unwrap()
}

#[test]
fn store_sits_beside_the_state_file() {
    assert_eq!(
        store_dir(Path::new("./data/update-state.json")),
        Path::new("./data/memorypack-schema")
    );
    assert_eq!(
        store_dir(Path::new("update-state.json")),
        Path::new("./memorypack-schema")
    );
    assert_eq!(store_file_name("1.6.4", 0xabcd), "1.6.4-0000abcd.json.gz");
    assert_eq!(
        store_file_name("1.7/beta", 0xdead_beef),
        "1.7_beta-deadbeef.json.gz"
    );
}

#[test]
fn loads_store_newest_first_then_embedded() {
    let dir = temp_dir("load");
    let old = schema_json("1.6.4", "2026-10-01T00:00:00Z", "x", r#"{"k": "str"}"#);
    let new = schema_json("1.7.0", "2026-09-01T00:00:00Z", "x", r#"{"k": "str"}"#);
    install(&dir, "1.6.4-00000001.json.gz", &gzip(&old).unwrap()).unwrap();
    install(&dir, "1.7.0-00000002.json.gz", &gzip(&new).unwrap()).unwrap();
    fs::write(dir.join("1.8.0-00000003.json.gz"), b"not gzip").unwrap();
    fs::write(dir.join("notes.txt"), b"ignored").unwrap();
    // `install` leaves no temporary file behind.
    let names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");

    let (schemas, warnings) = load_store(&dir).unwrap();
    assert_eq!(schemas.len(), 2);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("1.8.0-00000003.json.gz"));

    let (set, _) = load_set(&dir).unwrap();
    let labels: Vec<&str> = set.entries().map(|(label, _)| label).collect();
    let embedded = SchemaSet::embedded_entry().unwrap().0;
    assert_eq!(
        labels,
        ["1.7.0-00000002", "1.6.4-00000001", embedded.as_str()]
    );
    let (_, newest) = set.newest().unwrap();
    assert_eq!(newest.game_version(), Some("1.7.0"));
    assert_eq!(newest.client_crc32(), Some("0000abcd"));
    assert_eq!(newest.generated_at(), Some("2026-09-01T00:00:00Z"));

    // A missing store is just the embedded schema.
    let (set, warnings) = load_set(&dir.join("missing")).unwrap();
    assert_eq!((set.len(), warnings.len()), (1, 0));
    fs::remove_dir_all(dir).unwrap();
}

fn validation(candidate: usize, baseline: usize) -> Validation {
    Validation {
        files: 100,
        candidate_decoded: candidate,
        baseline: "1.6.4-00000001".into(),
        baseline_decoded: baseline,
        current_decoded: baseline,
        combined_decoded: candidate.max(baseline),
        ..Validation::default()
    }
}

#[test]
fn refuses_schemas_that_decode_fewer_files() {
    assert_eq!(decide(&validation(90, 90), false), Decision::Install);
    assert_eq!(decide(&validation(95, 90), false), Decision::Install);
    let Decision::Refuse(reason) = decide(&validation(89, 90), false) else {
        panic!("a regression was installed");
    };
    assert!(reason.contains("89 of 100"), "{reason}");
    assert!(reason.contains("90 of 1.6.4-00000001"), "{reason}");
    assert_eq!(decide(&validation(89, 90), true), Decision::Install);
    // Nothing to validate against (no JsonData yet): install.
    assert_eq!(decide(&Validation::default(), false), Decision::Install);
}

/// A minimal string pool file holding `strings` (uncompressed).
fn pool_file(strings: &[&str]) -> Vec<u8> {
    let mut offsets = Vec::new();
    let mut body = Vec::new();
    for s in strings {
        offsets.push(body.len() as i32);
        body.extend_from_slice(&(s.len() as i32).to_le_bytes());
        body.extend_from_slice(s.as_bytes());
    }
    let mut block = (strings.len() as i32).to_le_bytes().to_vec();
    for offset in offsets {
        block.extend_from_slice(&offset.to_le_bytes());
    }
    block.extend_from_slice(&body);
    let mut file = 1i32.to_le_bytes().to_vec();
    for v in [
        0x4350_5355,
        1,
        0,
        strings.len() as i32,
        block.len() as i32,
        block.len() as i32,
    ] {
        file.extend_from_slice(&v.to_le_bytes());
    }
    file.extend_from_slice(&block);
    file
}

#[test]
fn validates_candidate_against_current_set() {
    let json = temp_dir("validate");
    fs::create_dir_all(json.join("StringPool")).unwrap();
    fs::create_dir_all(json.join("sub")).unwrap();
    fs::write(json.join("StringPool/StringPool0.byte"), pool_file(&["a"])).unwrap();
    let file = |index: i32| [&[1u8][..], &index.to_le_bytes()].concat();
    fs::write(json.join("InPool.json"), file(0)).unwrap();
    fs::write(json.join("sub/OutOfPool.json"), file(9)).unwrap();
    fs::write(json.join("Plain.json"), b"{}").unwrap();

    let strings = |v: &str| {
        Schema::from_json(&schema_json(
            v,
            "2026-10-01T00:00:00Z",
            "s",
            r#"{"k": "str"}"#,
        ))
        .unwrap()
    };
    let ints = |v: &str| {
        Schema::from_json(&schema_json(
            v,
            "2026-10-01T00:00:00Z",
            "i",
            r#"{"k": "p", "t": "i32"}"#,
        ))
        .unwrap()
    };
    // In use: a newest string schema and an older int schema that decodes everything.
    let current = SchemaSet::new(
        vec![
            ("new".into(), Arc::new(strings("1.7.0"))),
            ("old".into(), Arc::new(ints("1.6.0"))),
        ],
        None,
    );
    let v = validate(&strings("1.8.0"), &current, &json).unwrap();
    assert_eq!(
        (
            v.files,
            v.candidate_decoded,
            v.baseline.as_str(),
            v.baseline_decoded,
            v.current_decoded,
            v.combined_decoded
        ),
        (2, 1, "new", 1, 2, 2)
    );
    assert!(v.new_files.is_empty());
    assert_eq!(v.lost_files, ["sub/OutOfPool.json"]);
    assert_eq!(decide(&v, false), Decision::Install);

    // An int candidate decodes both; the string-only set lost nothing to it.
    let strings_only = SchemaSet::new(vec![("new".into(), Arc::new(strings("1.7.0")))], None);
    let v = validate(&ints("1.8.0"), &strings_only, &json).unwrap();
    assert_eq!((v.candidate_decoded, v.baseline_decoded), (2, 1));
    assert_eq!(v.new_files, ["sub/OutOfPool.json"]);
    // ... while the string schema replacing it would regress.
    let ints_only = SchemaSet::new(vec![("ints".into(), Arc::new(ints("1.7.0")))], None);
    let v = validate(&strings("1.8.0"), &ints_only, &json).unwrap();
    assert!(matches!(decide(&v, false), Decision::Refuse(_)));
    fs::remove_dir_all(json).unwrap();
}

/// Every file under a group, sorted, with its bytes.
fn snapshot(group: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files: Vec<_> = json_files(&group.join("Json"))
        .unwrap()
        .into_iter()
        .flat_map(|(path, _)| {
            let mpk = path.with_extension("mpk");
            [path, mpk]
        })
        .filter(|path| path.exists())
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    files.sort();
    files
}

/// Generate a schema from real client files into an empty store, install it and re-decode a
/// copy of a JsonData group; a second run must find the schema unchanged and change nothing.
///
/// `IL2CPP_DLL`, `IL2CPP_METADATA`: the 1.6.4 client files. `SCHEMA_SYNC_GROUP`: a COPY of a
/// JsonData group folder (rewritten in place). `SCHEMA_SYNC_STORE`: a store directory that
/// does not exist yet.
#[test]
#[ignore]
fn schema_sync_installs_and_redecodes() {
    let (Ok(dll), Ok(meta), Ok(group), Ok(store)) = (
        std::env::var("IL2CPP_DLL"),
        std::env::var("IL2CPP_METADATA"),
        std::env::var("SCHEMA_SYNC_GROUP"),
        std::env::var("SCHEMA_SYNC_STORE"),
    ) else {
        panic!("set IL2CPP_DLL, IL2CPP_METADATA, SCHEMA_SYNC_GROUP and SCHEMA_SYNC_STORE");
    };
    let (dll, metadata) = (fs::read(dll).unwrap(), fs::read(meta).unwrap());
    let (group, store) = (PathBuf::from(group), PathBuf::from(store));
    assert!(!store.exists(), "SCHEMA_SYNC_STORE must not exist yet");
    let json_dir = group.join("Json");
    let groups = [group.clone()];
    let crc = crate::extraction::memorypack_schema::crc32(&dll);
    let input = SyncInput {
        dll: &dll,
        metadata: &metadata,
        game_version: "1.6.4",
        client_crc32: crc,
        store_dir: &store,
        json_dir: Some(&json_dir),
        groups: &groups,
        force: false,
    };

    let first = memorypack_store::sync(&input).unwrap();
    first.lines().iter().for_each(|l| println!("run 1: {l}"));
    let path = store.join(store_file_name("1.6.4", crc));
    assert_eq!(first.outcome, SyncOutcome::Installed(path.clone()));
    let v = &first.validation;
    assert!(v.candidate_decoded >= v.baseline_decoded);
    assert!(v.lost_files.is_empty(), "{:?}", v.lost_files);
    let redecode = first.redecode.clone().unwrap();
    assert_eq!(redecode.decoded as usize, v.combined_decoded);
    assert_eq!(redecode.write_errors, 0);
    // Every file the new schema decodes is attributed to it (it is tried first).
    assert_eq!(
        redecode.by_schema.get(&first.label).copied(),
        Some(v.candidate_decoded as u64)
    );
    assert_eq!(first.schema_version.as_deref(), Some("1.6.4"));

    // The installed schema decodes the sources exactly as the set does.
    let installed = Schema::from_gzip(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        installed.client_crc32(),
        Some(format!("{crc:08x}").as_str())
    );
    let pool = memorypack::StringPool::load(&json_dir.join("StringPool")).unwrap();
    let mut decoded = 0;
    for (file, rel_dir) in json_files(&json_dir).unwrap() {
        if let Some(data) = memorypack_source(&file).unwrap()
            && let Some(json) = memorypack::decode_bytes(&installed, &pool, &data, &rel_dir)
        {
            assert_eq!(fs::read(&file).unwrap(), json, "{}", file.display());
            decoded += 1;
        }
    }
    assert_eq!(decoded, v.candidate_decoded);

    let before = snapshot(&group);
    let second = memorypack_store::sync(&input).unwrap();
    second.lines().iter().for_each(|l| println!("run 2: {l}"));
    assert_eq!(second.outcome, SyncOutcome::Unchanged(path));
    assert_eq!(second.redecode, Some(redecode));
    assert!(
        before == snapshot(&group),
        "the second run changed the tree"
    );
    assert_eq!(fs::read_dir(&store).unwrap().count(), 1);
}
