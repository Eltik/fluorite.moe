//! Generate a MemoryPack schema from client files and optionally check it against reference
//! decoder output.
//!
//! ```sh
//! MP_DLL=GameAssembly.dll MP_METADATA=global-metadata.dat MP_VERSION=1.6.4 \
//! MP_JSON_DIR=output/decrypted/JsonData/Json MP_OUT=schema.json.gz \
//! MP_ORACLE=oracle/ cargo run --release --example memorypack_schema
//! ```
//!
//! `MP_JSON_DIR` (a `JsonData/Json` tree) is only read; it is used to learn `dir_roots` and,
//! with `MP_ORACLE` (`<relpath>.json` per file the reference decoded), to compare every
//! decoded file with the reference.

use backend::extraction::memorypack::{self, Schema, StringPool};
use backend::extraction::memorypack_schema::{generate_schema, gzip, memorypack_source};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Path of the first difference between two values.
fn first_difference(a: &Value, b: &Value, path: String) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                match y.get(k) {
                    Some(w) => {
                        if let Some(d) = first_difference(v, w, format!("{path}.{k}")) {
                            return Some(d);
                        }
                    }
                    None => return Some(format!("{path}.{k}: only in ours")),
                }
            }
            y.keys()
                .find(|k| !x.contains_key(*k))
                .map(|k| format!("{path}.{k}: only in reference"))
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (v, w))| first_difference(v, w, format!("{path}[{i}]"))),
        _ if a == b => None,
        _ => Some(format!(
            "{path}: {} != {}",
            short(&a.to_string()),
            short(&b.to_string())
        )),
    }
}

fn short(s: &str) -> String {
    if s.len() > 120 {
        format!("{}...", &s[..s.floor_char_boundary(120)])
    } else {
        s.to_owned()
    }
}

fn main() -> std::io::Result<()> {
    let dll = std::fs::read(env("MP_DLL").expect("set MP_DLL"))?;
    let metadata = std::fs::read(env("MP_METADATA").expect("set MP_METADATA"))?;
    let version = env("MP_VERSION").unwrap_or_else(|| "unknown".into());
    let json_dir = env("MP_JSON_DIR").map(PathBuf::from);

    let started = Instant::now();
    let (json, report) = generate_schema(&dll, &metadata, &version, json_dir.as_deref())?;
    let elapsed = started.elapsed();
    println!(
        "typedefs {}, registration {:#x}, wrappers {} ({} closed generic), roots {}, nodes {}",
        report.typedefs,
        report.registration_va,
        report.wrappers,
        report.generic_wrappers,
        report.roots,
        report.nodes
    );
    println!("node kinds {:?}", report.node_kinds);
    println!("compile {:?}, total {elapsed:?}", report.compile_time);
    for (why, n) in &report.unsupported {
        println!("unsupported x{n}: {why}");
    }
    for note in &report.notes {
        println!("note: {note}");
    }
    if let Some(learn) = &report.learn {
        println!(
            "learned {} dirs from {} files: {} decoded, {} failed in {:?}",
            learn.dirs, learn.files, learn.decoded, learn.failed_count, learn.elapsed
        );
        for f in &learn.failed {
            println!("  undecoded: {f}");
        }
    }
    let gz = gzip(&json)?;
    println!("schema {} bytes, gzip {} bytes", json.len(), gz.len());
    if let Some(out) = env("MP_OUT") {
        std::fs::write(&out, &gz)?;
        println!("wrote {out}");
    }

    let (Some(json_dir), Some(oracle)) = (json_dir, env("MP_ORACLE")) else {
        return Ok(());
    };
    compare(&json, &json_dir, Path::new(&oracle))
}

/// Decode every file with the new schema (the production candidate search) and compare
/// with the reference output.
fn compare(json: &[u8], json_dir: &Path, oracle: &Path) -> std::io::Result<()> {
    let started = Instant::now();
    let schema = Schema::from_json(json).map_err(std::io::Error::other)?;
    let old = Schema::embedded()?;
    let pool = StringPool::load(&json_dir.join("StringPool"))?;
    let (mut equal, mut mismatched, mut lost, mut gained, mut still_failing) =
        (0, Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (path, rel_dir) in memorypack::json_files(json_dir)? {
        let Some(data) = memorypack_source(&path)? else {
            continue;
        };
        let rel = path.strip_prefix(json_dir).unwrap_or(&path).to_path_buf();
        let expected: Option<Value> = std::fs::read(oracle.join(&rel))
            .ok()
            .map(|b| serde_json::from_slice(&b).expect("oracle JSON"));
        let actual = memorypack::decode_bytes_root(&schema, &pool, &data, &rel_dir);
        let rel = rel.display().to_string();
        match (actual, expected) {
            (Some((_, bytes)), Some(expected)) => {
                let value: Value = serde_json::from_slice(&bytes).expect("decoder JSON");
                if value == expected {
                    equal += 1;
                } else {
                    let ours = memorypack::decode_bytes_root(&schema, &pool, &data, &rel_dir)
                        .map(|(r, _)| r.to_owned());
                    let theirs = memorypack::decode_bytes_root(old, &pool, &data, &rel_dir)
                        .map(|(r, _)| r.to_owned());
                    let diff = first_difference(&value, &expected, String::new());
                    mismatched.push(format!("{rel}: ours {ours:?}, old {theirs:?}: {diff:?}"));
                }
            }
            (None, Some(_)) => lost.push(rel),
            (Some((root, _)), None) => gained.push(format!("{rel} as {root}")),
            (None, None) => still_failing.push(rel),
        }
    }
    println!(
        "compared in {:?}: {equal} equal, {} mismatched, {} the reference decoded but we did \
         not, {} newly decoded, {} still undecoded",
        started.elapsed(),
        mismatched.len(),
        lost.len(),
        gained.len(),
        still_failing.len()
    );
    for (label, list) in [
        ("mismatch", &mismatched),
        ("lost", &lost),
        ("gained", &gained),
        ("still failing", &still_failing),
    ] {
        for item in list.iter().take(60) {
            println!("  {label}: {item}");
        }
    }
    Ok(())
}
