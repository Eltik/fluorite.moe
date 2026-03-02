//! VFS Decrypt Probe — Companion for frida_vfs_decrypt_gadget.js (v17)
//!
//! Pass 1: Parse JSONL events, count event types, report decrypt func resolution,
//!         display DISASM_DUMP/DISASM_RESULT, CALLSITE_SCAN, CALLSITE_FUNC_HOOKED,
//!         REGISTRATION_CALL, XLUA_EXPORTS
//! Pass 1b: ReadFile analysis (Track F) — VFS_READ stats, return address candidates,
//!          loader detection, post-read buffer checks, loader decrypt successes
//! Pass 2: Decrypt signature analysis (arg roles, buffer patterns, call frequency)
//! Pass 3: Decrypt success analysis (entropy, formats, VFS correlation)
//! Pass 4: HGP.dll analysis (export usage patterns)
//! Pass 5: Export results (classified decrypts → JSON)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

// ══════════════════════════════════════════════════════════════
// Data structures
// ══════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize, Clone)]
#[expect(dead_code)]
struct DecryptCallEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(rename = "callNum", default)]
    call_num: u64,
    #[serde(default)]
    args: Vec<String>,
    #[serde(rename = "argTypes", default)]
    arg_types: Vec<String>,
    #[serde(rename = "preHex", default)]
    pre_hex: Vec<String>,
    #[serde(rename = "preEntropy", default)]
    pre_entropy: Vec<f64>,
    #[serde(rename = "postHex", default)]
    post_hex: Vec<String>,
    #[serde(rename = "postEntropy", default)]
    post_entropy: Vec<f64>,
    #[serde(default)]
    retval: String,
    #[serde(rename = "retType", default)]
    ret_type: String,
    #[serde(rename = "retHex", default)]
    ret_hex: String,
    #[serde(rename = "retEntropy", default)]
    ret_entropy: f64,
    #[serde(rename = "retFormat", default)]
    ret_format: Option<String>,
    #[serde(rename = "entropyDrop", default)]
    entropy_drop: bool,
    #[serde(rename = "detectedFormat", default)]
    detected_format: Option<String>,
    #[serde(rename = "matchedBufferIdx", default)]
    matched_buffer_idx: i32,
    #[serde(rename = "bufferSize", default)]
    buffer_size: u64,
    #[serde(rename = "correlatedVfsOpen", default)]
    correlated_vfs_open: Option<serde_json::Value>,
    #[serde(rename = "dumpFile", default)]
    dump_file: Option<String>,
    #[serde(rename = "elapsedMs", default)]
    elapsed_ms: u64,
    #[serde(default)]
    timestamp: u64,
}

#[derive(Debug, Serialize)]
struct ArgRoleAnalysis {
    index: usize,
    role: String, // "buffer", "size", "key", "output", "unknown"
    is_pointer_pct: f64,
    avg_int_value: f64,
    unique_values: usize,
    sample_values: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SignatureAnalysis {
    total_recon_calls: usize,
    arg_roles: Vec<ArgRoleAnalysis>,
    in_place_decryption: bool,
    returns_new_buffer: bool,
    call_frequency_hz: f64,
    avg_elapsed_ms: f64,
}

#[derive(Debug, Serialize)]
struct SuccessAnalysis {
    total_successes: usize,
    format_distribution: HashMap<String, usize>,
    avg_pre_entropy: f64,
    avg_post_entropy: f64,
    avg_entropy_drop: f64,
    size_distribution: SizeDistribution,
    correlated_with_vfs_open: usize,
    dir_hash_distribution: HashMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct SizeDistribution {
    min: u64,
    max: u64,
    avg: f64,
    median: u64,
}

#[derive(Debug, Serialize)]
struct HgpAnalysis {
    exports_called: Vec<HgpExportInfo>,
    total_calls: usize,
}

#[derive(Debug, Serialize)]
struct HgpExportInfo {
    name: String,
    call_count: usize,
    arg_patterns: Vec<String>,
    return_patterns: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DumpClassification {
    file: String,
    call_num: u64,
    format: String,
    size: u64,
    dir_hash: Option<String>,
}

#[derive(Debug, Serialize)]
struct AnalysisReport {
    // Pass 1
    event_counts: HashMap<String, usize>,
    total_events: usize,
    decrypt_func_resolved: bool,
    decrypt_func_addr: Option<String>,
    decrypt_func_module: Option<String>,
    // Pass 2
    signature: Option<SignatureAnalysis>,
    // Pass 3
    success: Option<SuccessAnalysis>,
    // Pass 4
    hgp: Option<HgpAnalysis>,
    // Pass 5
    dump_classifications: Vec<DumpClassification>,
    vfs_dll_loaded: bool,
    vfs_dll_exports: Vec<String>,
    // v2 additions
    registration_calls: Vec<serde_json::Value>,
    xlua_export_count: Option<usize>,
    hgp_loaded: bool,
    decrypt_source: Option<String>,
    // v3 additions
    disasm_discovery_count: usize,
    ga_cache_is_registration: bool,
    // v4 additions
    callsite_scan_modules: Vec<String>,
    callsite_total_matches: usize,
    callsite_read_matches: usize,
    callsite_funcs_hooked: usize,
    // v5 additions
    readfile_total_reads: usize,
    readfile_avg_entropy: f64,
    readfile_per_dir: HashMap<String, usize>,
    readfile_top_return_addrs: Vec<(String, usize)>,
    loader_detected: bool,
    loader_func_addr: Option<String>,
    loader_decrypt_count: usize,
    postcheck_entropy_drops: usize,
    // v7 additions
    readfile_diag_count: usize,
    readfile_diag_any_tracked: bool,
}

// ══════════════════════════════════════════════════════════════
// Helpers
// ══════════════════════════════════════════════════════════════

fn format_bytes(n: u64) -> String {
    if n >= 1_073_741_824 {
        format!("{:.2} GB", n as f64 / 1_073_741_824.0)
    } else if n >= 1_048_576 {
        format!("{:.2} MB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

fn median(values: &mut [u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    values[values.len() / 2]
}

// ══════════════════════════════════════════════════════════════
// Pass 1: Parse events
// ══════════════════════════════════════════════════════════════

struct Pass1Result {
    counts: HashMap<String, usize>,
    resolve_event: Option<serde_json::Value>,
    registration_calls: Vec<serde_json::Value>,
    xlua_exports_event: Option<serde_json::Value>,
    hgp_loaded_event: Option<serde_json::Value>,
    // v3 additions
    disasm_dump_event: Option<serde_json::Value>,
    disasm_result_event: Option<serde_json::Value>,
    ga_cache_is_registration: bool,
    // v4 additions
    callsite_scan_events: Vec<serde_json::Value>,
    callsite_func_hooked_events: Vec<serde_json::Value>,
    // v5 additions
    vfs_read_events: Vec<serde_json::Value>,
    vfs_read_postcheck_events: Vec<serde_json::Value>,
    vfs_loader_detected: Option<serde_json::Value>,
    vfs_loader_return_events: Vec<serde_json::Value>,
    vfs_loader_decrypt_events: Vec<serde_json::Value>,
    // v7 additions
    readfile_diag_events: Vec<serde_json::Value>,
}

fn pass1_parse(events_raw: &[serde_json::Value]) -> Pass1Result {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut resolve_event: Option<serde_json::Value> = None;
    let mut registration_calls: Vec<serde_json::Value> = Vec::new();
    let mut xlua_exports_event: Option<serde_json::Value> = None;
    let mut hgp_loaded_event: Option<serde_json::Value> = None;
    let mut disasm_dump_event: Option<serde_json::Value> = None;
    let mut disasm_result_event: Option<serde_json::Value> = None;
    let mut ga_cache_is_registration = false;
    let mut callsite_scan_events: Vec<serde_json::Value> = Vec::new();
    let mut callsite_func_hooked_events: Vec<serde_json::Value> = Vec::new();
    // v5
    let mut vfs_read_events: Vec<serde_json::Value> = Vec::new();
    let mut vfs_read_postcheck_events: Vec<serde_json::Value> = Vec::new();
    let mut vfs_loader_detected: Option<serde_json::Value> = None;
    let mut vfs_loader_return_events: Vec<serde_json::Value> = Vec::new();
    let mut vfs_loader_decrypt_events: Vec<serde_json::Value> = Vec::new();
    // v7
    let mut readfile_diag_events: Vec<serde_json::Value> = Vec::new();

    for ev in events_raw {
        let event_type = ev.get("type").and_then(|v| v.as_str()).unwrap_or("UNKNOWN");
        *counts.entry(event_type.to_string()).or_default() += 1;

        match event_type {
            "DECRYPT_FUNC_RESOLVED" if resolve_event.is_none() => {
                resolve_event = Some(ev.clone());
            }
            "REGISTRATION_CALL" => {
                registration_calls.push(ev.clone());
            }
            "XLUA_EXPORTS" if xlua_exports_event.is_none() => {
                xlua_exports_event = Some(ev.clone());
            }
            "HGP_LOADED" if hgp_loaded_event.is_none() => {
                hgp_loaded_event = Some(ev.clone());
            }
            "DISASM_DUMP" if disasm_dump_event.is_none() => {
                disasm_dump_event = Some(ev.clone());
            }
            "DISASM_RESULT" if disasm_result_event.is_none() => {
                disasm_result_event = Some(ev.clone());
            }
            "GA_CACHE_IS_REGISTRATION" => {
                ga_cache_is_registration = true;
            }
            "CALLSITE_SCAN" => {
                callsite_scan_events.push(ev.clone());
            }
            "CALLSITE_FUNC_HOOKED" => {
                callsite_func_hooked_events.push(ev.clone());
            }
            // v5
            "VFS_READ" => {
                if vfs_read_events.len() < 500 {
                    vfs_read_events.push(ev.clone());
                }
            }
            "VFS_READ_POSTCHECK" => {
                vfs_read_postcheck_events.push(ev.clone());
            }
            "VFS_LOADER_DETECTED" if vfs_loader_detected.is_none() => {
                vfs_loader_detected = Some(ev.clone());
            }
            "VFS_LOADER_RETURN" => {
                if vfs_loader_return_events.len() < 200 {
                    vfs_loader_return_events.push(ev.clone());
                }
            }
            "VFS_LOADER_DECRYPT" => {
                vfs_loader_decrypt_events.push(ev.clone());
            }
            // v7
            "READFILE_DIAG" => {
                readfile_diag_events.push(ev.clone());
            }
            _ => {}
        }
    }

    Pass1Result {
        counts,
        resolve_event,
        registration_calls,
        xlua_exports_event,
        hgp_loaded_event,
        disasm_dump_event,
        disasm_result_event,
        ga_cache_is_registration,
        callsite_scan_events,
        callsite_func_hooked_events,
        vfs_read_events,
        vfs_read_postcheck_events,
        vfs_loader_detected,
        vfs_loader_return_events,
        vfs_loader_decrypt_events,
        readfile_diag_events,
    }
}

// ══════════════════════════════════════════════════════════════
// Pass 2: Decrypt signature analysis
// ══════════════════════════════════════════════════════════════

fn pass2_signature(events_raw: &[serde_json::Value]) -> Option<SignatureAnalysis> {
    let recon_calls: Vec<&serde_json::Value> = events_raw
        .iter()
        .filter(|ev| ev.get("type").and_then(|v| v.as_str()) == Some("DECRYPT_CALL"))
        .collect();

    if recon_calls.is_empty() {
        return None;
    }

    // Analyze each arg position across all recon calls
    let mut arg_roles = Vec::new();
    for idx in 0..4 {
        let mut pointer_count = 0;
        let mut int_values: Vec<i64> = Vec::new();
        let mut unique_vals: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut samples: Vec<String> = Vec::new();

        for ev in &recon_calls {
            let arg_types = ev
                .get("argTypes")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            let arg_type = arg_types
                .get(idx)
                .and_then(|v| v.as_str())
                .unwrap_or("null");

            if arg_type == "pointer" {
                pointer_count += 1;
            } else if arg_type == "null" {
                // skip
            } else if let Some(stripped) = arg_type.strip_prefix("int:")
                && let Ok(v) = stripped.parse::<i64>()
            {
                int_values.push(v);
            }

            let args = ev
                .get("args")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if let Some(val) = args.get(idx).and_then(|v| v.as_str()) {
                unique_vals.insert(val.to_string());
                if samples.len() < 5 {
                    samples.push(val.to_string());
                }
            }
        }

        let total = recon_calls.len() as f64;
        let ptr_pct = pointer_count as f64 / total * 100.0;
        let avg_int = if int_values.is_empty() {
            0.0
        } else {
            int_values.iter().sum::<i64>() as f64 / int_values.len() as f64
        };

        // Classify role
        let role = if ptr_pct > 80.0 {
            if idx == 0 {
                "buffer"
            } else {
                "output_or_context"
            }
        } else if !int_values.is_empty() && avg_int > 16.0 && avg_int < 1_000_000_000.0 {
            "size"
        } else if ptr_pct < 10.0 && !int_values.is_empty() {
            "flags_or_key"
        } else {
            "unknown"
        };

        arg_roles.push(ArgRoleAnalysis {
            index: idx,
            role: role.to_string(),
            is_pointer_pct: (ptr_pct * 10.0).round() / 10.0,
            avg_int_value: (avg_int * 100.0).round() / 100.0,
            unique_values: unique_vals.len(),
            sample_values: samples,
        });
    }

    // Check if decryption is in-place (buffer content changes)
    let mut in_place_count = 0;
    let mut new_buffer_count = 0;
    for ev in &recon_calls {
        let pre_hex = ev
            .get("preHex")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let post_hex = ev
            .get("postHex")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // If any pointer arg's data changed, it's in-place
        let mut data_changed = false;
        for i in 0..4 {
            let pre = pre_hex.get(i).and_then(|v| v.as_str()).unwrap_or("");
            let post = post_hex.get(i).and_then(|v| v.as_str()).unwrap_or("");
            if !pre.is_empty() && !post.is_empty() && pre != post {
                data_changed = true;
                break;
            }
        }

        if data_changed {
            in_place_count += 1;
        }

        // If retval is a new pointer (not any of the args), it might return a new buffer
        let ret_type = ev.get("retType").and_then(|v| v.as_str()).unwrap_or("");
        if ret_type == "pointer" {
            let retval = ev.get("retval").and_then(|v| v.as_str()).unwrap_or("");
            let args = ev
                .get("args")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let is_arg = args.iter().any(|a| a.as_str() == Some(retval));
            if !is_arg && !retval.is_empty() {
                new_buffer_count += 1;
            }
        }
    }

    // Call frequency
    let mut timestamps: Vec<u64> = recon_calls
        .iter()
        .filter_map(|ev| ev.get("timestamp").and_then(|v| v.as_u64()))
        .collect();
    timestamps.sort_unstable();
    let freq_hz = if timestamps.len() >= 2 {
        let span_ms = timestamps.last().unwrap() - timestamps.first().unwrap();
        if span_ms > 0 {
            timestamps.len() as f64 / (span_ms as f64 / 1000.0)
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Average elapsed time
    let elapsed_values: Vec<u64> = recon_calls
        .iter()
        .filter_map(|ev| ev.get("elapsedMs").and_then(|v| v.as_u64()))
        .collect();
    let avg_elapsed = if elapsed_values.is_empty() {
        0.0
    } else {
        elapsed_values.iter().sum::<u64>() as f64 / elapsed_values.len() as f64
    };

    Some(SignatureAnalysis {
        total_recon_calls: recon_calls.len(),
        arg_roles,
        in_place_decryption: in_place_count > recon_calls.len() / 2,
        returns_new_buffer: new_buffer_count > recon_calls.len() / 2,
        call_frequency_hz: (freq_hz * 100.0).round() / 100.0,
        avg_elapsed_ms: (avg_elapsed * 100.0).round() / 100.0,
    })
}

// ══════════════════════════════════════════════════════════════
// Pass 3: Decrypt success analysis
// ══════════════════════════════════════════════════════════════

fn pass3_success(events_raw: &[serde_json::Value]) -> Option<SuccessAnalysis> {
    let success_events: Vec<&serde_json::Value> = events_raw
        .iter()
        .filter(|ev| {
            let t = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
            t == "DECRYPT_SUCCESS"
                || (t == "DECRYPT_CALL"
                    && (ev
                        .get("entropyDrop")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                        || ev.get("detectedFormat").and_then(|v| v.as_str()).is_some()))
        })
        .collect();

    if success_events.is_empty() {
        return None;
    }

    let mut format_dist: HashMap<String, usize> = HashMap::new();
    let mut pre_entropies: Vec<f64> = Vec::new();
    let mut post_entropies: Vec<f64> = Vec::new();
    let mut sizes: Vec<u64> = Vec::new();
    let mut vfs_correlated = 0;
    let mut dir_hash_dist: HashMap<String, usize> = HashMap::new();

    for ev in &success_events {
        // Format
        let fmt = ev
            .get("detectedFormat")
            .and_then(|v| v.as_str())
            .unwrap_or("entropy_drop");
        *format_dist.entry(fmt.to_string()).or_default() += 1;

        // Entropy
        let matched_idx = ev
            .get("matchedBufferIdx")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1) as i32;
        if matched_idx >= 0 {
            if let Some(pre_arr) = ev.get("preEntropy").and_then(|v| v.as_array())
                && let Some(pre) = pre_arr.get(matched_idx as usize).and_then(|v| v.as_f64())
                && pre > 0.0
            {
                pre_entropies.push(pre);
            }
            if let Some(post_arr) = ev.get("postEntropy").and_then(|v| v.as_array())
                && let Some(post) = post_arr.get(matched_idx as usize).and_then(|v| v.as_f64())
                && post >= 0.0
            {
                post_entropies.push(post);
            }
        }

        // Size
        if let Some(s) = ev.get("bufferSize").and_then(|v| v.as_u64())
            && s > 0
        {
            sizes.push(s);
        }

        // VFS correlation
        if let Some(open) = ev.get("correlatedVfsOpen")
            && !open.is_null()
        {
            vfs_correlated += 1;
            if let Some(dh) = open.get("dirHash").and_then(|v| v.as_str()) {
                *dir_hash_dist.entry(dh.to_string()).or_default() += 1;
            }
        }
    }

    let avg_pre = if pre_entropies.is_empty() {
        0.0
    } else {
        pre_entropies.iter().sum::<f64>() / pre_entropies.len() as f64
    };
    let avg_post = if post_entropies.is_empty() {
        0.0
    } else {
        post_entropies.iter().sum::<f64>() / post_entropies.len() as f64
    };

    let size_dist = if sizes.is_empty() {
        SizeDistribution {
            min: 0,
            max: 0,
            avg: 0.0,
            median: 0,
        }
    } else {
        SizeDistribution {
            min: *sizes.iter().min().unwrap(),
            max: *sizes.iter().max().unwrap(),
            avg: sizes.iter().sum::<u64>() as f64 / sizes.len() as f64,
            median: median(&mut sizes),
        }
    };

    Some(SuccessAnalysis {
        total_successes: success_events.len(),
        format_distribution: format_dist,
        avg_pre_entropy: (avg_pre * 1000.0).round() / 1000.0,
        avg_post_entropy: (avg_post * 1000.0).round() / 1000.0,
        avg_entropy_drop: ((avg_pre - avg_post) * 1000.0).round() / 1000.0,
        size_distribution: size_dist,
        correlated_with_vfs_open: vfs_correlated,
        dir_hash_distribution: dir_hash_dist,
    })
}

// ══════════════════════════════════════════════════════════════
// Pass 4: HGP.dll analysis
// ══════════════════════════════════════════════════════════════

fn pass4_hgp(events_raw: &[serde_json::Value]) -> Option<HgpAnalysis> {
    let hgp_calls: Vec<&serde_json::Value> = events_raw
        .iter()
        .filter(|ev| ev.get("type").and_then(|v| v.as_str()) == Some("HGP_CALL"))
        .collect();

    if hgp_calls.is_empty() {
        return None;
    }

    let mut by_export: HashMap<String, Vec<&serde_json::Value>> = HashMap::new();
    for ev in &hgp_calls {
        let name = ev
            .get("export")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        by_export.entry(name).or_default().push(ev);
    }

    let mut exports_info: Vec<HgpExportInfo> = Vec::new();
    for (name, calls) in &by_export {
        let mut arg_patterns: Vec<String> = Vec::new();
        let mut return_patterns: Vec<String> = Vec::new();

        for (i, ev) in calls.iter().enumerate() {
            if i >= 5 {
                break;
            }
            let at = ev
                .get("argTypes")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            arg_patterns.push(at);

            let rt = ev.get("retType").and_then(|v| v.as_str()).unwrap_or("?");
            let rv = ev.get("retval").and_then(|v| v.as_str()).unwrap_or("?");
            return_patterns.push(format!("{rt}={rv}"));
        }

        exports_info.push(HgpExportInfo {
            name: name.clone(),
            call_count: calls.len(),
            arg_patterns,
            return_patterns,
        });
    }

    exports_info.sort_by(|a, b| b.call_count.cmp(&a.call_count));

    Some(HgpAnalysis {
        exports_called: exports_info,
        total_calls: hgp_calls.len(),
    })
}

// ══════════════════════════════════════════════════════════════
// Pass 5: Export results — classify dump files
// ══════════════════════════════════════════════════════════════

fn pass5_classify_dumps(events_raw: &[serde_json::Value]) -> Vec<DumpClassification> {
    let mut results = Vec::new();

    for ev in events_raw {
        let t = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if t != "DECRYPT_CALL" && t != "DECRYPT_SUCCESS" && t != "VFS_LOADER_DECRYPT" {
            continue;
        }

        let dump_file = match ev.get("dumpFile").and_then(|v| v.as_str()) {
            Some(f) => f.to_string(),
            None => continue,
        };

        let format = ev
            .get("detectedFormat")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let call_num = ev.get("callNum").and_then(|v| v.as_u64()).unwrap_or(0);
        let size = ev.get("bufferSize").and_then(|v| v.as_u64()).unwrap_or(0);
        let dir_hash = ev
            .get("correlatedVfsOpen")
            .and_then(|v| v.get("dirHash"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        results.push(DumpClassification {
            file: dump_file,
            call_num,
            format,
            size,
            dir_hash,
        });
    }

    results
}

// ══════════════════════════════════════════════════════════════
// VFS DLL info extraction
// ══════════════════════════════════════════════════════════════

fn extract_vfs_dll_info(events_raw: &[serde_json::Value]) -> (bool, Vec<String>) {
    for ev in events_raw {
        if ev.get("type").and_then(|v| v.as_str()) == Some("VFS_DLL_LOADED") {
            let exports = ev
                .get("exports")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            return (true, exports);
        }
    }
    (false, Vec::new())
}

// ══════════════════════════════════════════════════════════════
// Main
// ══════════════════════════════════════════════════════════════

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: vfs-decrypt-probe <vfs_decrypt_events.jsonl> [--output-dir DIR]");
        eprintln!();
        eprintln!("Parses output from frida_vfs_decrypt_gadget.js and classifies results.");
        std::process::exit(1);
    }

    let input_path = &args[1];
    let output_dir = args
        .iter()
        .position(|a| a == "--output-dir")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("./output");

    println!("═══════════════════════════════════════════════════");
    println!("  VFS Decrypt Probe v12 — Summary-Centric Analysis Data");
    println!("═══════════════════════════════════════════════════");
    println!();

    // Load JSONL
    println!("Loading events from: {input_path}");
    let content = match fs::read_to_string(input_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR: Could not read {input_path}: {e}");
            std::process::exit(1);
        }
    };

    let mut events_raw: Vec<serde_json::Value> = Vec::new();
    let mut parse_errors = 0;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Handle null byte prefix (some Frida output has this)
        let clean = line.trim_start_matches('\0');
        match serde_json::from_str::<serde_json::Value>(clean) {
            Ok(val) => events_raw.push(val),
            Err(_) => {
                parse_errors += 1;
            }
        }
    }

    println!(
        "  Parsed: {} events ({} parse errors)",
        events_raw.len(),
        parse_errors
    );
    println!();

    if events_raw.is_empty() {
        println!("No events found. Make sure the gadget ran and produced output.");
        return;
    }

    // ── v12: Load summary JSON if available (contains critical data that may be missing from JSONL)
    let summary_path = {
        let p = std::path::Path::new(input_path);
        p.parent()
            .unwrap_or(std::path::Path::new("."))
            .join("vfs_decrypt_summary.json")
    };
    let summary: Option<serde_json::Value> = if summary_path.exists() {
        match fs::read_to_string(&summary_path) {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(v) => {
                    println!("  Loaded summary: {}", summary_path.display());
                    Some(v)
                }
                Err(e) => {
                    eprintln!("  WARN: Could not parse summary: {e}");
                    None
                }
            },
            Err(e) => {
                eprintln!("  WARN: Could not read summary: {e}");
                None
            }
        }
    } else {
        println!("  No summary file found at {}", summary_path.display());
        None
    };

    // Display v12 summary data if present
    if let Some(ref sum) = summary {
        let version = sum.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        let pid = sum.get("processId").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("  Summary version: {version}, PID: {pid}");

        if version >= 12 {
            println!();
            println!("── v12 Summary Data (in-memory accumulation) ─────");

            // Return addresses
            if let Some(addrs) = sum.get("readReturnAddrs").and_then(|v| v.as_object()) {
                println!("  NtReadFile return addresses ({} unique):", addrs.len());
                let mut sorted: Vec<_> = addrs
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0)))
                    .collect();
                sorted.sort_by(|a, b| b.1.cmp(&a.1));
                for (addr, count) in sorted.iter().take(20) {
                    println!("    {addr}: {count} calls");
                }
            }

            // Postcheck results
            if let Some(postchecks) = sum.get("postcheckResults").and_then(|v| v.as_array()) {
                println!();
                println!(
                    "  VFS_READ_POSTCHECK results ({} buffer changes detected):",
                    postchecks.len()
                );
                for pc in postchecks.iter().take(20) {
                    let call_num = pc.get("callNum").and_then(|v| v.as_u64()).unwrap_or(0);
                    let dir = pc.get("dirHash").and_then(|v| v.as_str()).unwrap_or("?");
                    let pre_e = pc.get("preEntropy").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let post_e = pc
                        .get("postEntropy")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let format = pc
                        .get("detectedFormat")
                        .and_then(|v| v.as_str())
                        .unwrap_or("none");
                    let delta = pc
                        .get("entropyDelta")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    println!(
                        "    #{call_num} {dir}: entropy {pre_e:.2} → {post_e:.2} (delta={delta:.3}) format={format}"
                    );
                }
            }

            // VFS read sample
            if let Some(sample) = sum.get("vfsReadSample").and_then(|v| v.as_array()) {
                println!();
                println!("  VFS_READ sample ({} captured):", sample.len());
                for r in sample.iter().take(10) {
                    let call = r.get("callNum").and_then(|v| v.as_u64()).unwrap_or(0);
                    let dir = r.get("dirHash").and_then(|v| v.as_str()).unwrap_or("?");
                    let bytes = r.get("bytesRead").and_then(|v| v.as_u64()).unwrap_or(0);
                    let ent = r.get("entropy").and_then(|v| v.as_f64()).unwrap_or(-1.0);
                    let ret = r.get("returnAddr").and_then(|v| v.as_str()).unwrap_or("?");
                    let ret_mod = r.get("returnModule").and_then(|v| v.as_str()).unwrap_or("");
                    let ret_off = r
                        .get("returnModuleOffset")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let src = r.get("source").and_then(|v| v.as_str()).unwrap_or("?");
                    println!(
                        "    #{call} [{src}] {dir} {bytes}B entropy={ent:.2} ret={ret} ({ret_mod}+{ret_off})"
                    );
                }
            }

            // Loader info
            if let Some(loader) = sum.get("loaderInfo") {
                println!();
                println!("  Loader discovery:");
                let func = loader
                    .get("funcAddr")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let rva = loader
                    .get("funcRVA")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let ga_addrs = loader
                    .get("gaReturnAddrs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let total = loader
                    .get("totalReturnAddrs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                println!("    Function: {func} (GA+{rva}), {ga_addrs}/{total} GA return addrs");
                if let Some(all) = loader.get("allReturnAddrs").and_then(|v| v.as_array()) {
                    println!("    All return addresses:");
                    for a in all {
                        let addr = a.get("addr").and_then(|v| v.as_str()).unwrap_or("?");
                        let count = a.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                        let module = a
                            .get("module")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let offset = a.get("offset").and_then(|v| v.as_str()).unwrap_or("?");
                        println!("      {addr}: {count} calls ({module}+{offset})");
                    }
                }
            }

            // Callsite functions
            if let Some(funcs) = sum.get("callsiteFunctions").and_then(|v| v.as_array()) {
                println!();
                println!("  Callsite functions hooked ({}):", funcs.len());
                for f in funcs.iter().take(10) {
                    let addr = f.get("funcAddr").and_then(|v| v.as_str()).unwrap_or("?");
                    let module = f.get("moduleName").and_then(|v| v.as_str()).unwrap_or("?");
                    let offset = f
                        .get("moduleOffset")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    println!("    {addr} ({module}+{offset})");
                }
                if funcs.len() > 10 {
                    println!("    ... and {} more", funcs.len() - 10);
                }
            }

            println!();
        }
    }

    // ── Pass 1: Event counts ────────────────────────────────
    println!("── Pass 1: Event Summary ──────────────────────────");
    let pass1 = pass1_parse(&events_raw);
    let event_counts = pass1.counts;

    let mut sorted_counts: Vec<_> = event_counts.iter().collect();
    sorted_counts.sort_by(|a, b| b.1.cmp(a.1));
    for (event_type, count) in &sorted_counts {
        println!("  {event_type:30} {count:>6}");
    }
    println!();

    let decrypt_resolved;
    let mut decrypt_addr: Option<String> = None;
    let mut decrypt_module: Option<String> = None;
    let mut decrypt_source: Option<String> = None;

    if let Some(ref ev) = pass1.resolve_event {
        decrypt_resolved = true;
        decrypt_addr = ev
            .get("funcAddr")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        decrypt_module = ev
            .get("moduleName")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        decrypt_source = ev
            .get("source")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let poll_count = ev.get("pollCount").and_then(|v| v.as_u64()).unwrap_or(0);
        let module_offset = ev
            .get("moduleOffset")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let source_str = decrypt_source.as_deref().unwrap_or("unknown");
        println!(
            "  Decrypt function RESOLVED: {} (in {} @ {}) via {} after {} polls",
            decrypt_addr.as_deref().unwrap_or("?"),
            decrypt_module.as_deref().unwrap_or("?"),
            module_offset,
            source_str,
            poll_count
        );
    } else {
        decrypt_resolved = false;
        println!("  Decrypt function: NOT RESOLVED (pointer remained NULL)");
    }
    println!();

    // Display registration calls (v2)
    if !pass1.registration_calls.is_empty() {
        println!(
            "  Registration calls (beyond_setDecryptionFunc): {}",
            pass1.registration_calls.len()
        );
        for (i, ev) in pass1.registration_calls.iter().enumerate() {
            let args = ev
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            println!("    call {}: args=[{}]", i + 1, args);

            // Show detailed arg analysis
            if let Some(details) = ev.get("argDetails").and_then(|v| v.as_array()) {
                for (j, detail) in details.iter().enumerate() {
                    let dtype = detail.get("type").and_then(|v| v.as_str()).unwrap_or("?");
                    let mod_name = detail.get("moduleName").and_then(|v| v.as_str());
                    let mod_offset = detail.get("moduleOffset").and_then(|v| v.as_str());
                    if let (Some(mn), Some(mo)) = (mod_name, mod_offset) {
                        println!("      arg{j}: {dtype} → {mn} @ {mo}");
                    } else {
                        println!("      arg{j}: {dtype}");
                    }
                }
            }

            // Show return value
            if let Some(ret_detail) = ev.get("retDetail") {
                let dtype = ret_detail
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let mod_name = ret_detail.get("moduleName").and_then(|v| v.as_str());
                let mod_offset = ret_detail.get("moduleOffset").and_then(|v| v.as_str());
                if let (Some(mn), Some(mo)) = (mod_name, mod_offset) {
                    println!("      ret:  {dtype} → {mn} @ {mo}");
                } else {
                    let retval = ev.get("retval").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("      ret:  {dtype} = {retval}");
                }
            }
        }
        println!();
    }

    // Display xlua exports (v2)
    let xlua_export_count = if let Some(ref ev) = pass1.xlua_exports_event {
        let count = ev.get("exportCount").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let base = ev.get("moduleBase").and_then(|v| v.as_str()).unwrap_or("?");
        println!("  xlua.dll: {count} exports (base={base})");

        // Show exports near the registration offset
        if let Some(exports) = ev.get("exports").and_then(|v| v.as_array()) {
            for exp in exports {
                let offset = exp.get("offset").and_then(|v| v.as_str()).unwrap_or("");
                if offset == "0x35d0" || offset == "0x35D0" {
                    let name = exp.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("    ** [{offset}] {name} ← registration offset **");
                }
            }
        }
        println!();
        Some(count)
    } else {
        None
    };

    // Display HGP loaded (v2)
    let hgp_loaded = if let Some(ref ev) = pass1.hgp_loaded_event {
        let base = ev.get("moduleBase").and_then(|v| v.as_str()).unwrap_or("?");
        let name = ev
            .get("moduleName")
            .and_then(|v| v.as_str())
            .unwrap_or("HGP.dll");
        println!("  {name} loaded @ {base}");
        println!();
        true
    } else {
        false
    };

    // Display disassembly results (v3)
    if let Some(ref ev) = pass1.disasm_dump_event {
        let addr = ev.get("address").and_then(|v| v.as_str()).unwrap_or("?");
        let size = ev.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
        let hex = ev.get("hex").and_then(|v| v.as_str()).unwrap_or("");
        println!("  DISASM_DUMP: {} bytes at {}", size, addr);

        // Pretty-print hex in 16-byte rows with offset
        let hex_bytes: Vec<u8> = (0..hex.len() / 2)
            .filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
            .collect();

        // Highlight known instruction patterns
        let patterns: &[(&[u8], &str)] = &[
            (&[0x48, 0x89, 0x0D], "MOV [rip+d32], RCX"),
            (&[0x48, 0x89, 0x15], "MOV [rip+d32], RDX"),
            (&[0x48, 0x89, 0x05], "MOV [rip+d32], RAX"),
            (&[0x4C, 0x89, 0x05], "MOV [rip+d32], R8"),
            (&[0x48, 0x89, 0x1D], "MOV [rip+d32], RBX"),
            (&[0xFF, 0x25], "JMP [rip+d32]"),
            (&[0x48, 0x8B, 0x0D], "MOV RCX, [rip+d32]"),
            (&[0x48, 0x8B, 0x05], "MOV RAX, [rip+d32]"),
            (&[0xFF, 0x15], "CALL [rip+d32]"),
        ];

        for row_start in (0..hex_bytes.len()).step_by(16) {
            let row_end = (row_start + 16).min(hex_bytes.len());
            let row = &hex_bytes[row_start..row_end];
            let hex_str: String = row.iter().map(|b| format!("{b:02x} ")).collect();
            // Check for instruction patterns in this row
            let mut annotations = Vec::new();
            for (pat, name) in patterns {
                for offset in row_start..row_end {
                    if offset + pat.len() <= hex_bytes.len()
                        && hex_bytes[offset..offset + pat.len()] == **pat
                    {
                        let rel_off = offset - row_start;
                        annotations.push(format!("+{:02x}={}", rel_off, name));
                    }
                }
            }
            let ann = if annotations.is_empty() {
                String::new()
            } else {
                format!("  ← {}", annotations.join(", "))
            };
            println!("    {:04x}: {}{}", row_start, hex_str, ann);
        }
        println!();
    }

    if let Some(ref ev) = pass1.disasm_result_event {
        let count = ev
            .get("discoveryCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        println!("  DISASM_RESULT: {} instruction patterns found", count);

        if let Some(discoveries) = ev.get("discoveries").and_then(|v| v.as_array()) {
            for d in discoveries {
                let offset = d.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
                let instr = d.get("instruction").and_then(|v| v.as_str()).unwrap_or("?");
                let reg = d.get("register").and_then(|v| v.as_str()).unwrap_or("?");
                let target = d.get("targetAddr").and_then(|v| v.as_str()).unwrap_or("?");
                let stored = d
                    .get("storedValue")
                    .and_then(|v| v.as_str())
                    .unwrap_or("NULL");
                let is_exec = d
                    .get("isExecutable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let target_mod = d.get("targetModule").and_then(|v| v.as_str());
                let target_off = d.get("targetModuleOffset").and_then(|v| v.as_str());

                let exec_marker = if is_exec { " [EXECUTABLE]" } else { "" };
                let mod_info = match (target_mod, target_off) {
                    (Some(m), Some(o)) => format!(" ({m} @ {o})"),
                    _ => String::new(),
                };

                println!(
                    "    +{:04x} {:30} reg={:10} global={} → stored={}{}{exec_marker}",
                    offset, instr, reg, target, stored, mod_info
                );
            }

            // Highlight the one identified as callback
            let callback_candidates: Vec<_> = discoveries
                .iter()
                .filter(|d| {
                    d.get("isExecutable")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                        && d.get("storedValue")
                            .and_then(|v| v.as_str())
                            .is_some_and(|s| s != "0x0")
                })
                .collect();

            if callback_candidates.is_empty() {
                println!(
                    "    ** No executable stored pointers found — callback may not be set yet **"
                );
            } else {
                println!(
                    "    ** {} executable stored pointer(s) — likely decrypt callback(s) **",
                    callback_candidates.len()
                );
            }
        }
        println!();
    }

    if pass1.ga_cache_is_registration {
        println!(
            "  GA cache (0x{:X}) resolved to xlua.dll+0x35D0 (registration func, NOT callback)",
            0x0F60CD80u32
        );
        println!("  Track B correctly deferred to Track A disassembly approach");
        println!();
    }

    // Display callsite scan results (v4)
    let mut callsite_scan_modules: Vec<String> = Vec::new();
    let mut callsite_total_matches: usize = 0;
    let mut callsite_read_matches: usize = 0;

    if !pass1.callsite_scan_events.is_empty() {
        println!("  ── v4 Callsite Scan (0x588 offset) ──");
        for scan_ev in &pass1.callsite_scan_events {
            let module = scan_ev
                .get("module")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let module_base = scan_ev
                .get("moduleBase")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let module_size = scan_ev
                .get("moduleSize")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let total = scan_ev
                .get("totalMatches")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let classified = scan_ev
                .get("classifiedCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            callsite_scan_modules.push(module.to_string());
            callsite_total_matches += classified;

            println!(
                "  Module: {} @ {} ({} bytes) — {} raw hits, {} classified",
                module,
                module_base,
                format_bytes(module_size),
                total,
                classified
            );

            if let Some(matches) = scan_ev.get("matches").and_then(|v| v.as_array()) {
                // Table header
                println!(
                    "    {:>10}  {:15}  {:8}  {:8}  {:16}",
                    "Offset", "Type", "Base", "Dst/Src", "Context"
                );
                println!("    {}", "-".repeat(65));

                for m in matches {
                    let offset = m
                        .get("moduleOffset")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let instr_type = m.get("instrType").and_then(|v| v.as_str()).unwrap_or("?");
                    let base_reg = m.get("baseReg").and_then(|v| v.as_str()).unwrap_or("?");
                    let dst_reg = m.get("dstOrSrcReg").and_then(|v| v.as_str()).unwrap_or("?");
                    let ctx = m.get("contextHex").and_then(|v| v.as_str()).unwrap_or("");
                    let ctx_short = if ctx.len() > 32 { &ctx[..32] } else { ctx };

                    let marker = match instr_type {
                        "MOV_READ" | "CALL_INDIRECT" | "JMP_INDIRECT" => " **",
                        _ => "",
                    };

                    if instr_type == "MOV_READ"
                        || instr_type == "CALL_INDIRECT"
                        || instr_type == "JMP_INDIRECT"
                    {
                        callsite_read_matches += 1;
                    }

                    println!(
                        "    {:>10}  {:15}  {:8}  {:8}  {:16}{}",
                        offset, instr_type, base_reg, dst_reg, ctx_short, marker
                    );
                }

                // Summary
                let read_count = matches
                    .iter()
                    .filter(|m| {
                        let t = m.get("instrType").and_then(|v| v.as_str()).unwrap_or("");
                        t == "MOV_READ" || t == "CALL_INDIRECT" || t == "JMP_INDIRECT"
                    })
                    .count();
                let write_count = matches
                    .iter()
                    .filter(|m| m.get("instrType").and_then(|v| v.as_str()) == Some("MOV_WRITE"))
                    .count();
                let cmp_count = matches
                    .iter()
                    .filter(|m| {
                        let t = m.get("instrType").and_then(|v| v.as_str()).unwrap_or("");
                        t.starts_with("CMP")
                    })
                    .count();

                println!();
                println!(
                    "    Summary: {} READ/CALL/JMP (callsites), {} WRITE (registration), {} CMP (checks)",
                    read_count, write_count, cmp_count
                );
            }
            println!();
        }
    }

    // Display hooked callsite functions (v4)
    let callsite_funcs_hooked = pass1.callsite_func_hooked_events.len();
    if !pass1.callsite_func_hooked_events.is_empty() {
        println!("  Callsite functions hooked: {}", callsite_funcs_hooked);
        for hook_ev in &pass1.callsite_func_hooked_events {
            let func_addr = hook_ev
                .get("funcAddr")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let module = hook_ev
                .get("moduleName")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let mod_offset = hook_ev
                .get("moduleOffset")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let cs_offset = hook_ev
                .get("callsiteOffset")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let cs_type = hook_ev
                .get("callsiteType")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let base_reg = hook_ev
                .get("baseReg")
                .and_then(|v| v.as_str())
                .unwrap_or("?");

            println!(
                "    {} @ {} ({}+{}) — callsite {} [{}, base={}]",
                func_addr, mod_offset, module, cs_offset, cs_type, base_reg, module
            );
        }
        println!();
    }

    // ── v7: ReadFile Diagnostics ──────────────────────────
    if !pass1.readfile_diag_events.is_empty() {
        println!("── v7 ReadFile Diagnostics ────────────────────────");
        println!(
            "  READFILE_DIAG events: {} (first {} unfiltered ReadFile calls)",
            pass1.readfile_diag_events.len(),
            pass1.readfile_diag_events.len()
        );
        println!(
            "    {:>5}  {:>10}  {:>12}  {:>10}  {:>14}",
            "call#", "handle", "reqSize", "tracked?", "trackedHandles"
        );
        println!("    {}", "-".repeat(60));

        let mut any_tracked = false;
        for ev in &pass1.readfile_diag_events {
            let call_num = ev.get("callNum").and_then(|v| v.as_u64()).unwrap_or(0);
            let handle = ev.get("handle").and_then(|v| v.as_i64()).unwrap_or(0);
            let req_size = ev
                .get("requestedSize")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let is_tracked = ev
                .get("isTrackedVfs")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let tracked_count = ev
                .get("trackedHandleCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            if is_tracked {
                any_tracked = true;
            }

            println!(
                "    {:>5}  {:>10}  {:>12}  {:>10}  {:>14}",
                call_num,
                handle,
                format_bytes(req_size as u64),
                if is_tracked { "YES" } else { "no" },
                tracked_count
            );
        }
        println!();

        // Diagnosis
        if !any_tracked {
            let vfs_open_count = *event_counts.get("VFS_FILE_OPEN").unwrap_or(&0);
            if vfs_open_count > 0 {
                println!("  DIAGNOSIS: ReadFile IS being called but NO handles match VFS opens!");
                println!(
                    "    → Game likely uses memory-mapped I/O (CreateFileMapping + MapViewOfFile)"
                );
                println!("    → Or NtReadFile directly, or handle duplication (DuplicateHandle)");
                println!("    → Check NtReadFile hook results below for low-level captures");
            } else {
                println!(
                    "  DIAGNOSIS: No VFS_FILE_OPEN events either — CreateFileW hook may have issues"
                );
            }
        } else {
            println!("  DIAGNOSIS: Some ReadFile handles match VFS opens — handle tracking works");
        }
        println!();
    } else {
        let readfile_diag_count = *event_counts.get("READFILE_DIAG").unwrap_or(&0);
        if readfile_diag_count == 0 {
            println!("── v7 ReadFile Diagnostics ────────────────────────");
            println!("  No READFILE_DIAG events — ReadFile may not be called at all");
            println!("  → Game likely uses memory-mapped I/O exclusively");
            println!();
        }
    }

    // ── Pass 1b: ReadFile Analysis (v5 Track F) ─────────────
    println!("── ReadFile Analysis (Track F) ─────────────────────");

    let readfile_total_reads = *event_counts.get("VFS_READ").unwrap_or(&0);
    let _postcheck_count = *event_counts.get("VFS_READ_POSTCHECK").unwrap_or(&0);
    let loader_return_count = *event_counts.get("VFS_LOADER_RETURN").unwrap_or(&0);
    let loader_decrypt_count_val = *event_counts.get("VFS_LOADER_DECRYPT").unwrap_or(&0);

    println!("  VFS_READ events: {} total", readfile_total_reads);

    // Read size and entropy distribution
    let mut read_sizes: Vec<u64> = Vec::new();
    let mut read_entropies: Vec<f64> = Vec::new();
    let mut per_dir_reads: HashMap<String, (usize, Vec<u64>, Vec<f64>)> = HashMap::new();
    let mut return_addr_counts: HashMap<String, usize> = HashMap::new();

    for ev in &pass1.vfs_read_events {
        if let Some(bytes_read) = ev.get("bytesRead").and_then(|v| v.as_u64()) {
            read_sizes.push(bytes_read);
        }
        if let Some(ent) = ev.get("entropy").and_then(|v| v.as_f64())
            && ent >= 0.0
        {
            read_entropies.push(ent);
        }
        let dir_hash = ev
            .get("dirHash")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let entry = per_dir_reads
            .entry(dir_hash)
            .or_insert_with(|| (0, Vec::new(), Vec::new()));
        entry.0 += 1;
        if let Some(bs) = ev.get("bytesRead").and_then(|v| v.as_u64()) {
            entry.1.push(bs);
        }
        if let Some(ent) = ev.get("entropy").and_then(|v| v.as_f64())
            && ent >= 0.0
        {
            entry.2.push(ent);
        }

        // Return address
        if let Some(ret_addr) = ev.get("returnAddr").and_then(|v| v.as_str()) {
            let ret_mod = ev
                .get("returnModule")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ret_off = ev
                .get("returnModuleOffset")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let key = if !ret_mod.is_empty() {
                format!("{ret_addr} ({ret_mod}+{ret_off})")
            } else {
                ret_addr.to_string()
            };
            *return_addr_counts.entry(key).or_default() += 1;
        }
    }

    if !read_sizes.is_empty() {
        let avg_size = read_sizes.iter().sum::<u64>() as f64 / read_sizes.len() as f64;
        println!(
            "  Read size distribution: min={} max={} avg={}",
            format_bytes(*read_sizes.iter().min().unwrap()),
            format_bytes(*read_sizes.iter().max().unwrap()),
            format_bytes(avg_size as u64),
        );
    }
    let readfile_avg_entropy = if !read_entropies.is_empty() {
        let avg = read_entropies.iter().sum::<f64>() / read_entropies.len() as f64;
        println!(
            "  Entropy distribution: min={:.3} max={:.3} avg={:.3}",
            read_entropies
                .iter()
                .cloned()
                .reduce(f64::min)
                .unwrap_or(0.0),
            read_entropies
                .iter()
                .cloned()
                .reduce(f64::max)
                .unwrap_or(0.0),
            avg,
        );
        (avg * 1000.0).round() / 1000.0
    } else {
        0.0
    };

    // Per-directory breakdown
    if !per_dir_reads.is_empty() {
        println!("  Per-directory reads:");
        let mut dir_entries: Vec<_> = per_dir_reads.iter().collect();
        dir_entries.sort_by(|a, b| b.1.0.cmp(&a.1.0));
        for (dir, (count, sizes, entropies)) in &dir_entries {
            let avg_size = if sizes.is_empty() {
                0
            } else {
                (sizes.iter().sum::<u64>() as f64 / sizes.len() as f64) as u64
            };
            let avg_ent = if entropies.is_empty() {
                0.0
            } else {
                entropies.iter().sum::<f64>() / entropies.len() as f64
            };
            println!(
                "    {dir}: {count} reads (avg {}, entropy {avg_ent:.2})",
                format_bytes(avg_size)
            );
        }
    }
    println!();

    // Return address candidates
    let mut readfile_top_return_addrs: Vec<(String, usize)> =
        return_addr_counts.into_iter().collect();
    readfile_top_return_addrs.sort_by(|a, b| b.1.cmp(&a.1));

    if !readfile_top_return_addrs.is_empty() {
        println!("  Return address candidates:");
        for (i, (addr, count)) in readfile_top_return_addrs.iter().take(10).enumerate() {
            let marker = if i == 0 { "  ** TOP CANDIDATE **" } else { "" };
            println!("    {addr}: {count} calls{marker}");
        }
        println!();
    }

    // Loader detection
    let loader_detected;
    let loader_func_addr;
    if let Some(ref ev) = pass1.vfs_loader_detected {
        loader_detected = true;
        let func_addr = ev.get("funcAddr").and_then(|v| v.as_str()).unwrap_or("?");
        let func_rva = ev.get("funcRVA").and_then(|v| v.as_str()).unwrap_or("");
        let mod_name = ev.get("moduleName").and_then(|v| v.as_str()).unwrap_or("?");
        let ret_addr = ev.get("returnAddr").and_then(|v| v.as_str()).unwrap_or("?");
        let ret_count = ev
            .get("returnAddrCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let ga_addrs = ev
            .get("gaReturnAddrs")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let total_addrs = ev
            .get("totalReturnAddrs")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        loader_func_addr = Some(func_addr.to_string());

        let rva_display = if !func_rva.is_empty() {
            format!(" (GA+{})", func_rva)
        } else {
            String::new()
        };
        println!(
            "  VFS Loader: DETECTED at {}{} ({}) — return addr {} ({} GA calls, {}/{} addrs in GA)",
            func_addr, rva_display, mod_name, ret_addr, ret_count, ga_addrs, total_addrs
        );
    } else {
        loader_detected = false;
        loader_func_addr = None;
        println!("  VFS Loader: NOT YET DETECTED (v9: only GA return addresses considered)");
    }
    println!();

    // Post-read buffer checks
    let postcheck_entropy_drops = pass1
        .vfs_read_postcheck_events
        .iter()
        .filter(|ev| {
            ev.get("entropyDelta")
                .and_then(|v| v.as_f64())
                .is_some_and(|d| d > 1.0)
                || ev.get("detectedFormat").and_then(|v| v.as_str()).is_some()
        })
        .count();

    if !pass1.vfs_read_postcheck_events.is_empty() {
        println!(
            "  Post-read buffer checks: {}/{} showed entropy change",
            pass1.vfs_read_postcheck_events.len(),
            readfile_total_reads.min(50)
        );
        if postcheck_entropy_drops > 0 {
            let avg_delta: f64 = pass1
                .vfs_read_postcheck_events
                .iter()
                .filter_map(|ev| ev.get("entropyDelta").and_then(|v| v.as_f64()))
                .sum::<f64>()
                / pass1.vfs_read_postcheck_events.len() as f64;
            println!(
                "    {postcheck_entropy_drops} significant entropy drops (avg delta: {avg_delta:.3})"
            );
        }
        println!();
    } else {
        println!(
            "  Post-read buffer checks: 0 (no buffer changes detected in first {} reads)",
            readfile_total_reads.min(50)
        );
        println!();
    }

    // Loader decrypt successes
    if !pass1.vfs_loader_decrypt_events.is_empty() {
        println!(
            "  Loader decrypt successes: {}",
            pass1.vfs_loader_decrypt_events.len()
        );
        println!(
            "    {:>6}  {:>12}  {:>12}  {:>12}  {:>8}  dump_file",
            "call#", "format", "pre_entropy", "post_entropy", "size"
        );
        println!("    {}", "-".repeat(75));

        for ev in &pass1.vfs_loader_decrypt_events {
            let call_num = ev.get("callNum").and_then(|v| v.as_u64()).unwrap_or(0);
            let fmt = ev
                .get("detectedFormat")
                .and_then(|v| v.as_str())
                .unwrap_or("entropy_drop");
            let pre_ent = ev
                .get("preEntropy")
                .and_then(|v| v.as_f64())
                .unwrap_or(-1.0);
            let post_ent = ev
                .get("postEntropy")
                .and_then(|v| v.as_f64())
                .unwrap_or(-1.0);
            let buf_size = ev.get("bufferSize").and_then(|v| v.as_u64()).unwrap_or(0);
            let dump_file = ev
                .get("dumpFile")
                .and_then(|v| v.as_str())
                .unwrap_or("none");
            println!(
                "    {:>6}  {:>12}  {:>12.3}  {:>12.3}  {:>8}  {}",
                call_num,
                fmt,
                pre_ent,
                post_ent,
                format_bytes(buf_size),
                dump_file
            );
        }
        println!();
    } else if loader_return_count > 0 {
        println!(
            "  Loader calls: {} — but NO decrypt successes detected",
            loader_return_count
        );
        println!("    (No entropy drops or known plaintext signatures in loader output)");
        println!();
    }

    // Loader return event summary
    if !pass1.vfs_loader_return_events.is_empty() {
        let total_loader = pass1.vfs_loader_return_events.len();
        let decrypt_detected_count = pass1
            .vfs_loader_return_events
            .iter()
            .filter(|ev| {
                ev.get("decryptDetected")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .count();
        println!(
            "  Loader return events: {} total, {} with decrypt detected",
            total_loader, decrypt_detected_count
        );
        println!();
    }

    let readfile_per_dir: HashMap<String, usize> = per_dir_reads
        .iter()
        .map(|(k, v)| (k.clone(), v.0))
        .collect();

    // ── Pass 2: Signature analysis ──────────────────────────
    println!("── Pass 2: Decrypt Signature Analysis ─────────────");
    let signature = pass2_signature(&events_raw);
    if let Some(ref sig) = signature {
        println!("  Recon calls analyzed: {}", sig.total_recon_calls);
        println!(
            "  In-place decryption: {}",
            if sig.in_place_decryption { "YES" } else { "no" }
        );
        println!(
            "  Returns new buffer:  {}",
            if sig.returns_new_buffer { "YES" } else { "no" }
        );
        println!("  Call frequency:      {:.1} Hz", sig.call_frequency_hz);
        println!("  Avg call time:       {:.1} ms", sig.avg_elapsed_ms);
        println!();
        for role in &sig.arg_roles {
            println!(
                "  arg[{}]: {:20} (ptr={:.0}%, avgInt={:.0}, {} unique)",
                role.index, role.role, role.is_pointer_pct, role.avg_int_value, role.unique_values
            );
            if !role.sample_values.is_empty() {
                let samples: Vec<&str> = role
                    .sample_values
                    .iter()
                    .take(3)
                    .map(|s| s.as_str())
                    .collect();
                println!("          samples: {}", samples.join(", "));
            }
        }
    } else {
        println!("  No DECRYPT_CALL events found — decrypt function may not have been resolved");
    }
    println!();

    // ── Pass 3: Success analysis ────────────────────────────
    println!("── Pass 3: Decrypt Success Analysis ───────────────");
    let success = pass3_success(&events_raw);
    if let Some(ref s) = success {
        println!("  Successful decryptions: {}", s.total_successes);
        println!(
            "  Avg entropy: {:.3} → {:.3} (drop: {:.3})",
            s.avg_pre_entropy, s.avg_post_entropy, s.avg_entropy_drop
        );
        println!("  Format distribution:");
        let mut fmt_sorted: Vec<_> = s.format_distribution.iter().collect();
        fmt_sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (fmt, count) in &fmt_sorted {
            println!("    {fmt:20} {count:>6}");
        }
        println!(
            "  Size: min={} max={} avg={} median={}",
            format_bytes(s.size_distribution.min),
            format_bytes(s.size_distribution.max),
            format_bytes(s.size_distribution.avg as u64),
            format_bytes(s.size_distribution.median)
        );
        println!(
            "  Correlated with VFS open: {}/{}",
            s.correlated_with_vfs_open, s.total_successes
        );
        if !s.dir_hash_distribution.is_empty() {
            println!("  Per-directory distribution:");
            for (dir, count) in &s.dir_hash_distribution {
                println!("    {dir}: {count}");
            }
        }
    } else {
        println!("  No successful decryptions detected");
        println!("  (No entropy drops or known plaintext signatures found)");
    }
    println!();

    // ── Pass 4: HGP.dll analysis ────────────────────────────
    println!("── Pass 4: HGP.dll Analysis ─────────────────────");
    let hgp = pass4_hgp(&events_raw);
    if let Some(ref h) = hgp {
        println!("  Total HGP calls: {}", h.total_calls);
        for exp in &h.exports_called {
            println!("  {}: {} calls", exp.name, exp.call_count);
            if !exp.arg_patterns.is_empty() {
                println!(
                    "    arg patterns: {:?}",
                    &exp.arg_patterns[..exp.arg_patterns.len().min(3)]
                );
            }
            if !exp.return_patterns.is_empty() {
                println!(
                    "    ret patterns: {:?}",
                    &exp.return_patterns[..exp.return_patterns.len().min(3)]
                );
            }
        }
    } else {
        println!("  No HGP_CALL events found — HGP.dll may not have been loaded");
    }
    println!();

    // ── Pass 5: Export ──────────────────────────────────────
    println!("── Pass 5: Export Results ──────────────────────────");

    let dump_classifications = pass5_classify_dumps(&events_raw);
    let (vfs_dll_loaded, vfs_dll_exports) = extract_vfs_dll_info(&events_raw);

    if vfs_dll_loaded {
        println!(
            "  Unsafe.VFS.dll loaded with {} exports",
            vfs_dll_exports.len()
        );
        for exp_name in &vfs_dll_exports {
            println!("    - {exp_name}");
        }
    } else {
        println!("  Unsafe.VFS.dll: not detected");
    }

    let unityfs_count = dump_classifications
        .iter()
        .filter(|d| d.format == "UnityFS")
        .count();
    let json_count = dump_classifications
        .iter()
        .filter(|d| d.format == "JSON" || d.format == "JSON_ARRAY")
        .count();

    println!(
        "  Dump files: {} total ({} UnityFS, {} JSON)",
        dump_classifications.len(),
        unityfs_count,
        json_count
    );

    if unityfs_count > 0 {
        println!("  ** UnityFS bundles found — can be fed into existing extraction pipeline **");
    }

    // Build report
    let report = AnalysisReport {
        event_counts,
        total_events: events_raw.len(),
        decrypt_func_resolved: decrypt_resolved,
        decrypt_func_addr: decrypt_addr,
        decrypt_func_module: decrypt_module,
        signature,
        success,
        hgp,
        dump_classifications,
        vfs_dll_loaded,
        vfs_dll_exports,
        registration_calls: pass1.registration_calls,
        xlua_export_count,
        hgp_loaded,
        decrypt_source,
        disasm_discovery_count: pass1
            .disasm_result_event
            .as_ref()
            .and_then(|ev| ev.get("discoveryCount").and_then(|v| v.as_u64()))
            .unwrap_or(0) as usize,
        ga_cache_is_registration: pass1.ga_cache_is_registration,
        callsite_scan_modules,
        callsite_total_matches,
        callsite_read_matches,
        callsite_funcs_hooked,
        // v5 additions
        readfile_total_reads,
        readfile_avg_entropy,
        readfile_per_dir,
        readfile_top_return_addrs,
        loader_detected,
        loader_func_addr,
        loader_decrypt_count: loader_decrypt_count_val,
        postcheck_entropy_drops,
        // v7 additions
        readfile_diag_count: pass1.readfile_diag_events.len(),
        readfile_diag_any_tracked: pass1.readfile_diag_events.iter().any(|ev| {
            ev.get("isTrackedVfs")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        }),
    };

    let output_path = format!("{output_dir}/vfs_decrypted_analysis.json");
    match serde_json::to_string_pretty(&report) {
        Ok(json) => match fs::write(&output_path, &json) {
            Ok(()) => println!("  Report saved: {output_path}"),
            Err(e) => eprintln!("  ERROR writing report: {e}"),
        },
        Err(e) => eprintln!("  ERROR serializing report: {e}"),
    }

    println!();
    println!("Done.");
}
