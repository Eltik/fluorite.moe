//! VFS File Reconstructor — Reassembles decrypted VFS files from Frida dump chunks
//!
//! Reads vfs_decrypt_summary.json (handleReadMap) + vfs_decrypted/*.bin dump files,
//! concatenates sequential chunks per handle, and writes complete decrypted files.
//!
//! Usage: cargo run --bin vfs-reconstruct --release -- <summary.json> <dump_dir> [output_dir]

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage: {} <summary.json> <dump_dir> [output_dir]",
            args[0]
        );
        eprintln!("  summary.json: vfs_decrypt_summary.json from Frida capture");
        eprintln!("  dump_dir:     directory containing decrypt_NNNN.bin files");
        eprintln!("  output_dir:   where to write reconstructed files (default: ./vfs_reconstructed)");
        std::process::exit(1);
    }

    let summary_path = &args[1];
    let dump_dir = PathBuf::from(&args[2]);
    let output_dir = if args.len() > 3 {
        PathBuf::from(&args[3])
    } else {
        PathBuf::from("./vfs_reconstructed")
    };

    // ── Parse summary JSON ──
    let summary_raw = fs::read_to_string(summary_path).expect("Failed to read summary JSON");
    let summary: Value = serde_json::from_str(&summary_raw).expect("Failed to parse summary JSON");

    let version = summary["version"].as_u64().unwrap_or(0);
    println!("Summary version: {version}");
    println!(
        "Handle dump count: {}",
        summary["handleDumpCount"].as_u64().unwrap_or(0)
    );

    let handle_map = match summary.get("handleReadMap") {
        Some(Value::Object(m)) => m,
        _ => {
            eprintln!("No handleReadMap in summary (need v16+ capture)");
            std::process::exit(1);
        }
    };

    // ── Collect handles by file path ──
    // Multiple handles may refer to the same file (game re-reads).
    // Group them and pick the one with the most chunks.
    struct HandleInfo {
        handle: String,
        path: String,
        dir_hash: String,
        total_read: u64,
        chunks: Vec<ChunkInfo>,
    }
    struct ChunkInfo {
        offset: u64,
        size: u64,
        dump_file: String,
        format: Option<String>,
    }

    let mut handles: Vec<HandleInfo> = Vec::new();

    for (handle_id, info) in handle_map {
        let path = info["path"].as_str().unwrap_or("").to_string();
        let dir_hash = info["dirHash"].as_str().unwrap_or("").to_string();
        let total_read = info["totalRead"].as_u64().unwrap_or(0);

        let chunks: Vec<ChunkInfo> = match info.get("chunks") {
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|c| {
                    Some(ChunkInfo {
                        offset: c["offset"].as_u64()?,
                        size: c["size"].as_u64()?,
                        dump_file: c["dumpFile"].as_str()?.to_string(),
                        format: c["format"].as_str().map(|s| s.to_string()),
                    })
                })
                .collect(),
            _ => Vec::new(),
        };

        handles.push(HandleInfo {
            handle: handle_id.clone(),
            path,
            dir_hash,
            total_read,
            chunks,
        });
    }

    // Sort by dir_hash then path for display
    handles.sort_by(|a, b| (&a.dir_hash, &a.path).cmp(&(&b.dir_hash, &b.path)));

    // ── Summary table ──
    println!("\n── Handle Summary ──");
    println!(
        "{:>8} {:>8} {:>8} {:>12} {:>12}  File",
        "Handle", "Dir", "Chunks", "ChunkBytes", "TotalRead"
    );

    // Group by (dir_hash, filename) to find best handle per file
    let mut best_per_file: BTreeMap<String, usize> = BTreeMap::new(); // key → index in handles

    for (i, h) in handles.iter().enumerate() {
        let filename = h.path
            .rsplit(['\\', '/'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("?")
            .to_string();
        let chunk_bytes: u64 = h.chunks.iter().map(|c| c.size).sum();

        println!(
            "{:>8} {:>8} {:>8} {:>12} {:>12}  {}",
            h.handle, h.dir_hash, h.chunks.len(), chunk_bytes, h.total_read, filename
        );

        let key = format!("{}:{}", h.dir_hash, filename);
        if let Some(&prev_idx) = best_per_file.get(&key) {
            if h.chunks.len() > handles[prev_idx].chunks.len() {
                best_per_file.insert(key, i);
            }
        } else {
            best_per_file.insert(key, i);
        }
    }

    // ── Reconstruct files ──
    println!("\n── Reconstructing Files ──");

    let mut total_files = 0u32;
    let mut total_bytes = 0u64;
    let mut total_gaps = 0u32;

    for (key, &idx) in &best_per_file {
        let h = &handles[idx];
        if h.chunks.is_empty() {
            continue;
        }

        // Extract filename from Windows path (backslashes not parsed by Unix Path)
        let filename = h.path
            .rsplit(['\\', '/'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&format!("handle_{}", h.handle))
            .to_string();

        // Create output directory: output_dir/dir_hash/
        let dir_out = output_dir.join(&h.dir_hash);
        fs::create_dir_all(&dir_out).ok();

        let out_path = dir_out.join(&filename);

        // Sort chunks by offset
        let mut sorted_chunks: Vec<&ChunkInfo> = h.chunks.iter().collect();
        sorted_chunks.sort_by_key(|c| c.offset);

        // Check for gaps and coverage
        let mut expected_offset = 0u64;
        let mut gaps = Vec::new();
        for chunk in &sorted_chunks {
            if chunk.offset > expected_offset {
                gaps.push((expected_offset, chunk.offset));
            }
            expected_offset = chunk.offset + chunk.size;
        }

        let _chunk_bytes: u64 = sorted_chunks.iter().map(|c| c.size).sum();

        // Write reconstructed file
        let mut out_file = match fs::File::create(&out_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  SKIP {key}: cannot create output: {e}");
                continue;
            }
        };

        let mut written = 0u64;
        let mut missing_chunks = 0u32;

        for chunk in &sorted_chunks {
            // If there's a gap before this chunk, write zeros
            if chunk.offset > written {
                let gap = chunk.offset - written;
                let zeros = vec![0u8; gap as usize];
                out_file.write_all(&zeros).ok();
                written += gap;
            }

            // Read the dump file — strip directory prefix (summary has "vfs_decrypted/decrypt_NNNN.bin"
            // but dump_dir already points to the directory containing the files)
            let dump_basename = Path::new(&chunk.dump_file)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| chunk.dump_file.clone());
            let dump_path = dump_dir.join(&dump_basename);
            match fs::read(&dump_path) {
                Ok(data) => {
                    let write_size = std::cmp::min(data.len() as u64, chunk.size);
                    out_file.write_all(&data[..write_size as usize]).ok();
                    written += write_size;
                }
                Err(_) => {
                    // Missing dump file — write zeros
                    let zeros = vec![0u8; chunk.size as usize];
                    out_file.write_all(&zeros).ok();
                    written += chunk.size;
                    missing_chunks += 1;
                }
            }
        }

        // Detect formats found
        let formats: Vec<&str> = sorted_chunks
            .iter()
            .filter_map(|c| c.format.as_deref())
            .collect();
        let format_str = if formats.is_empty() {
            String::new()
        } else {
            let mut unique: Vec<&str> = formats.clone();
            unique.dedup();
            format!(" formats={}", unique.join(","))
        };

        let gap_count = gaps.len();
        println!(
            "  {}/{}: {} → {:.1} KB ({} chunks, {} gaps, {} missing){format_str}",
            h.dir_hash,
            filename,
            h.handle,
            written as f64 / 1024.0,
            sorted_chunks.len(),
            gap_count,
            missing_chunks,
        );

        if !gaps.is_empty() && gap_count <= 5 {
            for (start, end) in &gaps {
                println!(
                    "    gap: 0x{start:08x}..0x{end:08x} ({:.1} KB)",
                    (*end - *start) as f64 / 1024.0
                );
            }
        }

        total_files += 1;
        total_bytes += written;
        total_gaps += gap_count as u32;
    }

    println!("\n── Results ──");
    println!("  Reconstructed files: {total_files}");
    println!("  Total bytes written: {total_bytes} ({:.1} MB)", total_bytes as f64 / 1024.0 / 1024.0);
    println!("  Total gaps (zero-filled): {total_gaps}");
    println!("  Output directory: {}", output_dir.display());
}
