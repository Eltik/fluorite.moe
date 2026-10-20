//! Human-readable summaries of a chunk's Unity objects, for planning and debugging.
use super::chunk::{ChunkFiles, containers};
use crate::extraction::unity_extract::class_name;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Class IDs worth counting when looking for rigs and animation.
const INTERESTING: &[i32] = &[
    1, 4, 21, 23, 28, 33, 43, 48, 74, 90, 91, 95, 111, 137, 142, 205, 221,
];

/// Per-file class counts, externals, and container paths containing `filter`.
#[must_use]
pub fn summarize(files: &ChunkFiles, filter: &str) -> String {
    let mut out = String::new();
    let mut totals: BTreeMap<i32, usize> = BTreeMap::new();
    let mut prefix_counts: BTreeMap<String, usize> = BTreeMap::new();
    let _ = writeln!(
        out,
        "{} SerializedFile(s), {} resource stream(s), {} parse failure(s)",
        files.files.len(),
        files.resources.len(),
        files.parse_failures
    );
    for file in &files.files {
        let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
        for obj in &file.sf.objects {
            *counts.entry(obj.class_id).or_default() += 1;
            *totals.entry(obj.class_id).or_default() += 1;
        }
        let entries = containers(&file.sf);
        for entry in &entries {
            let prefix: String = entry.path.split('/').take(5).collect::<Vec<_>>().join("/");
            let ext = entry.path.rsplit('.').next().unwrap_or("");
            *prefix_counts.entry(format!("{prefix} .{ext}")).or_default() += 1;
        }
        let matching: Vec<_> = entries
            .iter()
            .filter(|e| !filter.is_empty() && (filter == "*" || e.path.contains(filter)))
            .collect();
        if filter != "*" && !filter.is_empty() && matching.is_empty() {
            continue;
        }
        let interesting: Vec<String> = counts
            .iter()
            .filter(|(id, _)| INTERESTING.contains(id))
            .map(|(id, n)| format!("{}={n}", class_name(*id)))
            .collect();
        let res_size: usize = files
            .resources
            .iter()
            .filter(|(k, _)| k.starts_with(&file.cab))
            .map(|(_, v)| v.len())
            .sum();
        let _ = writeln!(
            out,
            "- {} objects={} externals={} sf={}KB res={}KB [{}]",
            file.cab,
            file.sf.objects.len(),
            file.sf.externals.len(),
            file.sf.data.len() / 1024,
            res_size / 1024,
            interesting.join(" ")
        );
        for entry in matching.iter().take(200) {
            let _ = writeln!(
                out,
                "    {} -> ({}, {})",
                entry.path, entry.file_id, entry.path_id
            );
        }
    }
    let _ = writeln!(out, "class totals:");
    for (id, n) in &totals {
        let _ = writeln!(out, "  {:>4} {:<28} {n}", id, class_name(*id));
    }
    let _ = writeln!(out, "container prefixes:");
    for (prefix, n) in &prefix_counts {
        let _ = writeln!(out, "  {n:>6} {prefix}");
    }
    out
}

/// Aggregate facts about AnimationClip / Animator data across a store, to plan clip export.
#[must_use]
pub fn animation_survey(
    store: &std::path::Path,
    records: &[super::store::CabRecord],
    max_files: usize,
) -> serde_json::Value {
    use serde_json::{Value, json};
    let mut clip_files = 0usize;
    let mut clips = 0usize;
    let mut read_failures = 0usize;
    let mut legacy = 0usize;
    let mut compressed = 0usize;
    let mut muscle = 0usize;
    let mut streamed_bytes = 0u64;
    let mut dense_frames = 0u64;
    let mut constant_curves = 0u64;
    let mut binding_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut sample_rates: BTreeMap<String, usize> = BTreeMap::new();
    let mut curve_kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut name_prefixes: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: Vec<Value> = Vec::new();
    let mut acl_clips = 0usize;
    let mut acl_bytes = 0u64;
    let mut clip_tags: BTreeMap<String, usize> = BTreeMap::new();
    let mut flags: BTreeMap<String, usize> = BTreeMap::new();
    let mut storage_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut containers: BTreeMap<String, usize> = BTreeMap::new();
    let mut class_totals: BTreeMap<i32, usize> = BTreeMap::new();
    for rec in records {
        for id in [74, 90, 91, 95, 221] {
            if let Some(n) = rec.classes.get(&id) {
                *class_totals.entry(id).or_default() += n;
            }
        }
        for (path, _, _) in &rec.containers {
            let ext = path.rsplit('.').next().unwrap_or("");
            if matches!(
                ext,
                "anim" | "controller" | "overridecontroller" | "fbx" | "playable"
            ) {
                let prefix: String = path.split('/').take(6).collect::<Vec<_>>().join("/");
                *containers.entry(format!("{prefix}/… .{ext}")).or_default() += 1;
            }
        }
    }
    let mut source =
        super::store::CabSource::new(Some(store.to_path_buf()), super::ChunkFiles::default());
    for rec in records
        .iter()
        .filter(|r| r.classes.contains_key(&74))
        .take(max_files)
    {
        let Some(i) = source.get(&rec.cab) else {
            continue;
        };
        clip_files += 1;
        let sf = &source.files.files[i].sf;
        for obj in sf.objects_of_type(74) {
            clips += 1;
            let Ok(clip) = sf.read_object(obj) else {
                read_failures += 1;
                continue;
            };
            let name = clip
                .get("m_Name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let prefix: String = name.split('_').take(2).collect::<Vec<_>>().join("_");
            *name_prefixes.entry(prefix).or_default() += 1;
            if clip.get("m_Legacy").and_then(Value::as_bool) == Some(true) {
                legacy += 1;
            }
            // Endfield's ACL payload: a struct of raw ACL `compressed_tracks` buffers.
            let acl = clip
                .pointer("/m_AclCompressedBuffer/TransformBufferData")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let dense_curves = clip
                .pointer("/m_MuscleClip/m_Clip/data/m_DenseClip/m_CurveCount")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let streamed_words = clip
                .pointer("/m_MuscleClip/m_Clip/data/m_StreamedClip/data")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let constants = clip
                .pointer("/m_MuscleClip/m_Clip/data/m_ConstantClip/data")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let storage = match (
                acl > 0,
                dense_curves > 0 || streamed_words > 2 || constants > 0,
            ) {
                (true, false) => "acl-only",
                (true, true) => "acl+mecanim",
                (false, true) => "mecanim-only",
                (false, false) => "empty",
            };
            *storage_kinds
                .entry(format!(
                    "{storage} ({})",
                    if name.to_ascii_lowercase().starts_with("a_actor") {
                        "actor"
                    } else if name.to_ascii_lowercase().starts_with("a_monster") {
                        "monster"
                    } else {
                        "other"
                    }
                ))
                .or_default() += 1;
            if acl > 0 {
                acl_clips += 1;
                acl_bytes += acl as u64;
            }
            for key in [
                "m_aclType",
                "m_TransferCompressed",
                "m_Compressed",
                "m_UseHighQualityCurve",
                "m_HasGenericRootTransform",
                "m_HasMotionFloatCurves",
            ] {
                if let Some(v) = clip.get(key) {
                    *flags.entry(format!("{key}={v}")).or_default() += 1;
                }
            }
            if let Some(tag) = clip.get("m_ClipTag") {
                *clip_tags.entry(tag.to_string()).or_default() += 1;
            }
            if clip.get("m_Compressed").and_then(Value::as_bool) == Some(true) {
                compressed += 1;
            }
            for (key, label) in [
                ("m_RotationCurves", "rotation"),
                ("m_CompressedRotationCurves", "compressedRotation"),
                ("m_EulerCurves", "euler"),
                ("m_PositionCurves", "position"),
                ("m_ScaleCurves", "scale"),
                ("m_FloatCurves", "float"),
                ("m_PPtrCurves", "pptr"),
            ] {
                let n = clip.get(key).and_then(Value::as_array).map_or(0, Vec::len);
                *curve_kinds.entry(label).or_default() += n;
            }
            let rate = clip
                .get("m_SampleRate")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            *sample_rates.entry(format!("{rate}")).or_default() += 1;
            let mc = clip.pointer("/m_MuscleClip/m_Clip/data");
            if let Some(mc) = mc {
                let streamed = mc
                    .pointer("/m_StreamedClip/data")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                let frames = mc
                    .pointer("/m_DenseClip/m_FrameCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let constants = mc
                    .pointer("/m_ConstantClip/data")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                if streamed + frames as usize + constants > 0 {
                    muscle += 1;
                }
                streamed_bytes += 4 * streamed as u64;
                dense_frames += frames;
                constant_curves += constants as u64;
            }
            let bindings = clip
                .pointer("/m_ClipBindingConstant/genericBindings")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let bucket = match bindings {
                0 => "0",
                1..=50 => "1-50",
                51..=200 => "51-200",
                201..=500 => "201-500",
                _ => ">500",
            };
            *binding_counts.entry(bucket.to_string()).or_default() += 1;
            if examples.len() < 6 {
                let stop = clip
                    .pointer("/m_MuscleClip/m_StopTime")
                    .and_then(Value::as_f64);
                examples.push(json!({
                    "cab": rec.cab,
                    "name": name,
                    "aclBytes": acl,
                    "legacy": clip.get("m_Legacy"),
                    "compressed": clip.get("m_Compressed"),
                    "sampleRate": rate,
                    "stopTime": stop,
                    "genericBindings": bindings,
                    "denseFrames": mc.and_then(|m| m.pointer("/m_DenseClip/m_FrameCount")),
                    "streamedWords": mc.and_then(|m| m.pointer("/m_StreamedClip/data")).and_then(Value::as_array).map(Vec::len),
                    "constantCurves": mc.and_then(|m| m.pointer("/m_ConstantClip/data")).and_then(Value::as_array).map(Vec::len),
                    "topLevelKeys": clip.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()),
                }));
            }
        }
    }
    json!({
        "classTotals": class_totals.iter().map(|(k, v)| (crate::extraction::unity_extract::class_name(*k), *v)).collect::<BTreeMap<_, _>>(),
        "filesWithClips": records.iter().filter(|r| r.classes.contains_key(&74)).count(),
        "filesRead": clip_files,
        "clipsRead": clips,
        "readFailures": read_failures,
        "legacy": legacy,
        "compressedFlag": compressed,
        "aclCompressedClips": acl_clips,
        "aclCompressedBytes": acl_bytes,
        "clipTags": clip_tags,
        "flags": flags,
        "clipStorage": storage_kinds,
        "withMuscleClipData": muscle,
        "muscleStreamedBytes": streamed_bytes,
        "muscleDenseFrames": dense_frames,
        "muscleConstantValues": constant_curves,
        "curveCounts": curve_kinds,
        "sampleRates": sample_rates,
        "genericBindingBuckets": binding_counts,
        "clipNamePrefixes": name_prefixes.into_iter().filter(|(_, n)| *n >= 5).collect::<BTreeMap<_, _>>(),
        "animContainers": containers,
        "examples": examples,
    })
}
