use crate::extraction::il2cpp::attrs::Value;
use crate::extraction::il2cpp::metadata::Blob;
use crate::extraction::il2cpp::{Il2Cpp, TypeKind};
use std::collections::HashMap;

#[test]
fn compressed_integers() {
    let mut b = Blob::new(&[
        0x05, 0x81, 0x02, 0xC0, 0x01, 0x02, 0x03, 0xF0, 1, 0, 0, 0, 0xFF,
    ]);
    assert_eq!(b.compressed_u32().unwrap(), 5);
    assert_eq!(b.compressed_u32().unwrap(), 0x102);
    assert_eq!(b.compressed_u32().unwrap(), 0x010203);
    assert_eq!(b.compressed_u32().unwrap(), 1);
    assert_eq!(b.compressed_u32().unwrap(), u32::MAX);
    let mut b = Blob::new(&[0x00, 0x01, 0x02, 0x03, 0xFF]);
    assert_eq!(b.compressed_i32().unwrap(), 0);
    assert_eq!(b.compressed_i32().unwrap(), -1);
    assert_eq!(b.compressed_i32().unwrap(), 1);
    assert_eq!(b.compressed_i32().unwrap(), -2);
    assert_eq!(b.compressed_i32().unwrap(), i32::MIN);
}

#[test]
fn rejects_non_il2cpp_input() {
    let error = Il2Cpp::new(b"MZ not really a dll", b"not metadata")
        .err()
        .unwrap();
    assert!(error.is_structural(), "{error}");
}

fn kind_name(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Class => "class",
        TypeKind::Struct => "struct",
        TypeKind::Enum => "enum",
        TypeKind::Interface => "interface",
    }
}

/// Cross-check the reader against the type table the dump-based pipeline parsed out of
/// Il2CppDumper's `dump.cs` (`scripts/memorypack-schema/parse_dump.py` plus `sizes.py`).
///
/// `IL2CPP_DLL`, `IL2CPP_METADATA` are the 1.6.4 client files; `IL2CPP_TYPES_JSON` is the
/// `types.json` table.
#[test]
#[ignore]
fn matches_dump_type_table() {
    let (Ok(dll), Ok(meta), Ok(types)) = (
        std::env::var("IL2CPP_DLL"),
        std::env::var("IL2CPP_METADATA"),
        std::env::var("IL2CPP_TYPES_JSON"),
    ) else {
        panic!("set IL2CPP_DLL, IL2CPP_METADATA and IL2CPP_TYPES_JSON");
    };
    let (dll, meta) = (std::fs::read(dll).unwrap(), std::fs::read(meta).unwrap());
    let started = std::time::Instant::now();
    let il = Il2Cpp::new(&dll, &meta).unwrap();
    println!("loaded in {:?}", started.elapsed());
    assert_eq!(il.typedef_count(), 91478);
    assert_eq!(il.meta.typedef_stride, 92);
    assert_eq!(il.reg.va, 0x1_8da6_0e20);

    let types: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&std::fs::read(types).unwrap()).unwrap();
    let mut mismatches: HashMap<&str, Vec<String>> = HashMap::new();
    let mut note = |what: &'static str, msg: String| mismatches.entry(what).or_default().push(msg);
    let (mut checked, mut fields_checked, mut props_checked) = (0, 0, 0);
    for (key, t) in &types {
        let def = t["idx"].as_u64().unwrap() as u32;
        checked += 1;
        if il.full_name(def) != *key {
            // `parse_dump.py` loses the namespace of some types nested in generic types.
            // It also guesses a nested type's outer type by name and index distance, and
            // some guesses land on a same-named type in another namespace (or keep the C#
            // keyword of `string.TrimType`); the reader follows the exact `declaringTypeIndex`.
            if il.dump_name(def) == *key {
                note("dump key without namespace", key.clone());
            } else if il.declaring(def).is_some()
                && il
                    .dump_name(def)
                    .rsplit_once('.')
                    .is_some_and(|(_, own)| key.ends_with(&format!(".{own}")))
            {
                note(
                    "dump outer guessed",
                    format!("{def}: {} != {key}", il.full_name(def)),
                );
            } else {
                note("name", format!("{def}: {} != {key}", il.full_name(def)));
                continue;
            }
        }
        if kind_name(il.kind(def)) != t["kind"].as_str().unwrap() {
            note("kind", format!("{key}: {:?}", il.kind(def)));
        }
        let expected_fields: Vec<_> = t["fields"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| {
                let mods = f["mods"].as_array().unwrap();
                !mods.iter().any(|m| m == "static" || m == "const")
            })
            .collect();
        let fields: Vec<_> = il
            .fields(def)
            .unwrap()
            .into_iter()
            .filter(|f| !f.is_static())
            .collect();
        if fields.len() != expected_fields.len() {
            note(
                "field count",
                format!("{key}: {} != {}", fields.len(), expected_fields.len()),
            );
        } else {
            for (f, e) in fields.iter().zip(&expected_fields) {
                fields_checked += 1;
                if f.name != e["name"].as_str().unwrap() {
                    note("field name", format!("{key}.{}", f.name));
                }
                let offset = f.offset.map(|o| format!("0x{o:X}"));
                if offset.as_deref() != e["off"].as_str() {
                    note(
                        "field offset",
                        format!("{key}.{}: {offset:?} != {}", f.name, e["off"]),
                    );
                }
                if il.csharp_name(&f.ty) != e["type"].as_str().unwrap() {
                    note(
                        "field type",
                        format!(
                            "{key}.{}: {} != {}",
                            f.name,
                            il.csharp_name(&f.ty),
                            e["type"]
                        ),
                    );
                }
            }
        }
        let expected_props = t["props"].as_array().unwrap();
        let props = il.properties(def).unwrap();
        if props.len() != expected_props.len() {
            note(
                "property count",
                format!("{key}: {} != {}", props.len(), expected_props.len()),
            );
        } else {
            for (p, e) in props.iter().zip(expected_props) {
                props_checked += 1;
                let acc: Vec<&str> = [(p.has_getter, "get;"), (p.has_setter, "set;")]
                    .into_iter()
                    .filter_map(|(has, s)| has.then_some(s))
                    .collect();
                let expected_acc: Vec<&str> = e["acc"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| a.as_str().unwrap())
                    .collect();
                if p.name != e["name"].as_str().unwrap() || acc != expected_acc {
                    note("property", format!("{key}.{}: {acc:?}", p.name));
                }
                let ty = p.ty.as_ref().map(|t| il.csharp_name(t)).unwrap_or_default();
                if ty != e["type"].as_str().unwrap() {
                    note(
                        "property type",
                        format!("{key}.{}: {ty} != {}", p.name, e["type"]),
                    );
                }
                let allow = il
                    .attributes(def, p.token)
                    .unwrap()
                    .iter()
                    .any(|a| il.name(a.type_def) == "MemoryPackAllowSerializeAttribute");
                let expected_allow = e["attrs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|a| a == "[MemoryPackAllowSerialize]");
                if allow != expected_allow {
                    note("allow serialize", format!("{key}.{}", p.name));
                }
            }
        }
        if let Some(size) = t.get("size").and_then(serde_json::Value::as_u64)
            && il.value_size(def).map(u64::from) != Some(size)
        {
            note("size", format!("{key}: {:?} != {size}", il.value_size(def)));
        }
        // `[MemoryPackUnion(tag, typeof(X))]`, in the dump's attribute order.
        let expected_unions: Vec<String> = t["attrs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a.as_str())
            .filter(|a| a.starts_with("[MemoryPackUnion("))
            .map(str::to_owned)
            .collect();
        let unions: Vec<String> = il
            .type_attributes(def)
            .unwrap()
            .iter()
            .filter(|a| il.name(a.type_def) == "MemoryPackUnionAttribute")
            .map(|a| {
                let tag = a.args[0].as_i64().unwrap();
                let case = il.type_arg_def(&a.args[1]).unwrap();
                format!("[MemoryPackUnion({tag}, typeof({}))]", il.dump_name(case))
            })
            .collect();
        if unions != expected_unions {
            note("unions", format!("{key}: {unions:?}"));
        }
    }

    // The named samples must be exact.
    let sample = |name: &str| types[name]["idx"].as_u64().unwrap() as u32;
    assert_eq!(
        il.value_size(sample("Beyond.Montage.AnimationClipAsyncInfo")),
        Some(36)
    );
    assert_eq!(
        il.value_size(sample("Beyond.Gameplay.Core.DispelConfig")),
        Some(8)
    );
    assert_eq!(
        il.value_size(sample("Beyond.Resource.ManifestData.GUIDProxy")),
        Some(16)
    );
    let buff = sample("Beyond.MemoryPack.Beyond_Gameplay_Core_BuffDataForMemoryPack");
    let members: Vec<_> = il
        .properties(buff)
        .unwrap()
        .into_iter()
        .filter(|p| p.has_setter && !p.has_getter && p.name.starts_with("__"))
        .collect();
    assert_eq!(members.len(), 30);
    let allowed = members
        .iter()
        .filter(|p| {
            il.attributes(buff, p.token)
                .unwrap()
                .iter()
                .any(|a| il.name(a.type_def) == "MemoryPackAllowSerializeAttribute")
        })
        .count();
    assert!(allowed >= 2, "{allowed} AllowSerialize members");
    let with_id = sample("Beyond.MemoryPack.Beyond_Gameplay_GameDataWithIdForMemoryPack");
    let mut tags: Vec<i64> = il
        .type_attributes(with_id)
        .unwrap()
        .iter()
        .filter(|a| il.name(a.type_def) == "MemoryPackUnionAttribute")
        .map(|a| a.args[0].as_i64().unwrap())
        .collect();
    tags.sort_unstable();
    assert_eq!(tags, (0..=12).collect::<Vec<_>>());
    assert!(matches!(
        il.type_attributes(with_id).unwrap()[0].args.first(),
        Some(Value::Int(_) | Value::UInt(_))
    ));

    println!(
        "checked {checked} types, {fields_checked} fields, {props_checked} properties in {:?}",
        started.elapsed()
    );
    let mut kinds: Vec<_> = mismatches.iter().collect();
    kinds.sort_by_key(|(k, _)| *k);
    for (what, list) in &kinds {
        println!(
            "{what}: {} mismatches, e.g. {:?}",
            list.len(),
            &list[..list.len().min(5)]
        );
    }
    // Type spellings are informational (dump.cs has its own quirks); everything else must match.
    let hard: usize = kinds
        .iter()
        .filter(|(k, _)| {
            !matches!(
                **k,
                "field type"
                    | "property type"
                    | "dump key without namespace"
                    | "dump outer guessed"
            )
        })
        .map(|(_, l)| l.len())
        .sum();
    assert_eq!(hard, 0);
}
