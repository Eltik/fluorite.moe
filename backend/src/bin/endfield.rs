/// Unified CLI for Arknights Endfield asset extraction.
///
/// Subcommands:
///   info          — show game config, version, patch info
///   extract       — extract Unity assets (JSON, textures, gamedata) from AES dirs
///   extract-file  — extract a single .chk by name
///   decrypt       — decrypt ChaCha20-encrypted VFS directories (tables, configs, lua)
///   list          — list all VFS directories and files
///   scan          — scan for asset names/paths
///   survey        — survey all Unity objects, types, counts
use backend::extraction::blc;
use backend::extraction::pipeline::{self, VFS_DIRS};
use backend::extraction::texture;
use backend::extraction::texture_layout;
use backend::extraction::texture_sort;
use backend::extraction::unity_extract::{
    ChkContents, class_name, extract_chk_contents, get_script_type_name, parse_text_asset,
};
use backend::extraction::update;
use backend::extraction::utils::{format_bytes, hex_dump, sanitize_filename};
use backend::extraction::vfs_decrypt;
use backend::extraction::zip_range::ZipCentralDirectory;
use chrono::Utc;
use clap::{Parser, Subcommand};
use futures::stream::{self, StreamExt};
use reqwest::Client;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use unity_asset_binary::asset::SerializedFileParser;
use unity_asset_binary::object::ObjectHandle;

#[derive(Parser)]
#[command(name = "endfield", about = "Arknights Endfield asset extraction CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show game config, version, patch info
    Info,
    /// Extract assets from VFS
    Extract {
        /// Extract JSON/TextAssets
        #[arg(long)]
        json: bool,
        /// Extract Texture2D as PNG
        #[arg(long)]
        textures: bool,
        /// Extract MonoBehaviour + registry data
        #[arg(long)]
        gamedata: bool,
        /// Extract all types (default if no flags given)
        #[arg(long)]
        all: bool,
        /// Specific VFS directory hash (default: all known)
        #[arg(long)]
        dir: Option<String>,
        /// Output directory
        #[arg(long, default_value = "./output")]
        output: String,
        /// Process ALL .chk files (default: smallest per directory)
        #[arg(long)]
        scan_all: bool,
    },
    /// Extract a single .chk by name
    ExtractFile {
        /// The .chk filename (or substring) to extract
        chk_name: String,
        /// Which VFS directory hash
        #[arg(long)]
        dir: Option<String>,
        /// Output directory
        #[arg(long, default_value = "./output")]
        output: String,
    },
    /// Decrypt ChaCha20-encrypted VFS directories (game data tables, JSON configs, Lua)
    Decrypt {
        /// Specific directory hash to decrypt (default: all encrypted dirs)
        #[arg(long)]
        dir: Option<String>,
        /// Output directory
        #[arg(long, default_value = "./output/decrypted")]
        output: String,
    },
    /// Incrementally extract assets and decrypted data changed upstream
    Update {
        /// Output directory
        #[arg(long, default_value = "./output")]
        output: String,
        /// Persistent update state file
        #[arg(long, default_value = "./data/update-state.json")]
        state: String,
        /// Re-process all entries
        #[arg(long)]
        force: bool,
        /// Check for an update without downloading or changing state
        #[arg(long)]
        check: bool,
        /// Do not extract Texture2D PNGs
        #[arg(long)]
        no_textures: bool,
        /// Emit `@progress` lines for scripts/auto-update.sh to render
        #[arg(long)]
        progress: bool,
    },
    /// List all VFS directories and files in the CDN archive
    List,
    /// Scan for asset names/paths
    Scan {
        /// Process ALL .chk files
        #[arg(long)]
        scan_all: bool,
    },
    /// Survey all Unity objects, types, counts
    Survey {
        /// Process ALL .chk files
        #[arg(long)]
        scan_all: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let client = Client::new();

    match cli.command {
        Commands::Info => cmd_info(&client).await,
        Commands::Extract {
            json,
            textures,
            gamedata,
            all,
            dir,
            output,
            scan_all,
        } => {
            let (do_json, do_tex, do_gamedata) = if all || (!json && !textures && !gamedata) {
                (true, true, true)
            } else {
                (json, textures, gamedata)
            };
            cmd_extract(
                &client,
                do_json,
                do_tex,
                do_gamedata,
                dir,
                &output,
                scan_all,
            )
            .await
        }
        Commands::ExtractFile {
            chk_name,
            dir,
            output,
        } => cmd_extract_file(&client, &chk_name, dir, &output).await,
        Commands::Decrypt { dir, output } => cmd_decrypt(&client, dir, &output).await,
        Commands::Update {
            output,
            state,
            force,
            check,
            no_textures,
            progress,
        } => {
            cmd_update(
                &client,
                Path::new(&output),
                Path::new(&state),
                force,
                check,
                no_textures,
                progress,
            )
            .await
        }
        Commands::List => cmd_list(&client).await,
        Commands::Scan { scan_all } => cmd_scan(&client, scan_all).await,
        Commands::Survey { scan_all } => cmd_survey(&client, scan_all).await,
    }
}

// ═══════════════════════════════════════════════════════════
// info — show game config
// ═══════════════════════════════════════════════════════════

async fn cmd_info(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Endfield Game Info ===\n");

    let resolved = pipeline::resolve_pipeline(client, true).await?;
    let config = &resolved.config;

    println!("Version:      {}", config.version);
    println!("Game version: {}", config.game_version);
    println!("Rand str:     {}", config.rand_str);
    println!("Packs:        {} files", config.packs.len());
    println!("Resources:    {} entries", config.resources.len());
    println!("Domain:       {}", config.domain);
    println!("Res version:  {}", config.res_version);

    for res in &config.resources {
        println!("  Resource: {} v{}", res.name, res.version);
    }

    if let Some(patch) = &resolved.patch_index {
        println!("\nPatch version: {}", patch.version);
        println!("Chunk files:   {}", patch.files.len());
        let total_bytes: u64 = patch.files.iter().map(|f| f.size).sum();
        for (i, file) in patch.files.iter().enumerate() {
            println!(
                "  [{}] {} ({}, md5: {})",
                i,
                file.name,
                format_bytes(file.size),
                file.md5
            );
        }
        println!("Total size: {}", format_bytes(total_bytes));
    }

    println!("\nZIP entries: {}", resolved.cd.len());
    println!("\n=== Done ===");
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// extract — JSON, textures, gamedata
// ═══════════════════════════════════════════════════════════

async fn cmd_extract(
    client: &Client,
    do_json: bool,
    do_tex: bool,
    do_gamedata: bool,
    dir_filter: Option<String>,
    output: &str,
    scan_all: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Endfield Asset Extraction ===\n");

    let modes: Vec<&str> = [
        do_json.then_some("JSON"),
        do_tex.then_some("textures"),
        do_gamedata.then_some("gamedata"),
    ]
    .into_iter()
    .flatten()
    .collect();
    println!("Modes: {}", modes.join(", "));

    let resolved = pipeline::resolve_pipeline(client, false).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    let dirs: Vec<(&str, &str)> = if let Some(ref d) = dir_filter {
        VFS_DIRS
            .iter()
            .filter(|(hash, _)| hash == &d.as_str())
            .copied()
            .collect()
    } else {
        VFS_DIRS.to_vec()
    };

    std::fs::create_dir_all(output)?;

    let gamedata_dir = format!("{}/gamedata", output);
    let mono_dir = format!("{}/gamedata/monobehaviour", output);
    let tex_dir = format!("{}/textures", output);
    if do_json || do_gamedata {
        std::fs::create_dir_all(&gamedata_dir)?;
        std::fs::create_dir_all(&mono_dir)?;
    }
    if do_tex {
        std::fs::create_dir_all(&tex_dir)?;
    }

    let mut stats = ExtractStats::default();

    for (dir, desc) in &dirs {
        println!("--- {} ({}) ---", dir, desc);
        let mut candidates = pipeline::list_chk_files(&resolved.cd, dir);
        if !scan_all {
            candidates.truncate(1);
        }
        println!("  Processing {} .chk file(s)\n", candidates.len());

        for (i, (name, size)) in candidates.iter().enumerate() {
            if candidates.len() > 1 {
                println!(
                    "  [{}/{}] {} ({})",
                    i + 1,
                    candidates.len(),
                    name,
                    format_bytes(*size)
                );
            } else {
                println!("  {} ({})", name, format_bytes(*size));
            }

            let data = match pipeline::download_chk(client, &resolved.cd, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("    Download error: {}", e);
                    continue;
                }
            };

            let options = ChkExtractOptions {
                chk_name: name,
                do_json,
                do_tex,
                do_gamedata,
                gamedata_dir: &gamedata_dir,
                mono_dir: &mono_dir,
                tex_dir: &tex_dir,
            };
            process_chk_contents(&data, &options, &mut stats);
        }
        println!();
    }

    if do_tex {
        match texture_layout::rebuild_index(Path::new(&tex_dir)) {
            Ok(count) => println!("  Texture index entries: {count}"),
            Err(error) => eprintln!("  FAIL rebuild texture index: {error}"),
        }
    }

    println!("=== Extraction Summary ===");
    if do_json {
        println!(
            "  TextAssets: {} (JSON: {}, Text: {}, Bin: {})",
            stats.total_text_assets, stats.total_json, stats.total_text, stats.total_bin
        );
    }
    if do_gamedata {
        println!("  MonoBehaviours: {}", stats.total_mono);
        if !stats.mono_type_counts.is_empty() {
            let mut types: Vec<_> = stats.mono_type_counts.iter().collect();
            types.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            for (type_name, count) in types.iter().take(20) {
                println!("    {}: {}", type_name, count);
            }
        }
    }
    if do_tex {
        println!(
            "  Textures extracted: {}, failed: {}",
            stats.total_tex_extracted, stats.total_tex_failed
        );
    }
    println!("  Output: {}", output);
    println!("\n=== Done ===");
    Ok(())
}

#[derive(Default)]
struct ExtractStats {
    total_text_assets: u64,
    total_json: u64,
    total_text: u64,
    total_bin: u64,
    total_mono: u64,
    total_tex_extracted: u64,
    total_tex_failed: u64,
    mono_type_counts: HashMap<String, u64>,
    seen_names: HashSet<String>,
    seen_mono: HashSet<String>,
}

struct ChkExtractOptions<'a> {
    chk_name: &'a str,
    do_json: bool,
    do_tex: bool,
    do_gamedata: bool,
    gamedata_dir: &'a str,
    mono_dir: &'a str,
    tex_dir: &'a str,
}

/// Process one downloaded `.chk`. A valid container is successful even if individual objects fail.
fn process_chk_contents(
    data: &[u8],
    options: &ChkExtractOptions<'_>,
    stats: &mut ExtractStats,
) -> bool {
    let Some(chk) = extract_chk_contents(data) else {
        println!("    Not a valid VFS container, skipping");
        return false;
    };
    println!(
        "    {} SerializedFile(s), {} .resS stream(s)",
        chk.sf_data.len(),
        chk.resources.len()
    );
    if options.do_json {
        for sf_bytes in &chk.sf_data {
            let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                Ok(sf) => sf,
                Err(_) => continue,
            };
            for obj in sf.objects_of_type(49) {
                if let Ok(bytes) = sf.object_bytes(obj)
                    && let Some((name, content)) = parse_text_asset(bytes)
                {
                    if !stats.seen_names.insert(name.clone()) {
                        continue;
                    }
                    stats.total_text_assets += 1;
                    if let Some(kind) = save_text_asset(options.gamedata_dir, &name, &content) {
                        match kind {
                            "JSON" => stats.total_json += 1,
                            "TEXT" => stats.total_text += 1,
                            "BIN" => stats.total_bin += 1,
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    if options.do_gamedata {
        for sf_bytes in &chk.sf_data {
            let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                Ok(sf) => sf,
                Err(_) => continue,
            };
            if !sf.enable_type_tree {
                continue;
            }
            for obj in sf.objects_of_type(114) {
                let type_name = get_script_type_name(&sf, obj);
                let handle = ObjectHandle::new(&sf, obj);
                let obj_name = handle
                    .peek_name()
                    .ok()
                    .flatten()
                    .filter(|n| !n.is_empty())
                    .map_or_else(|| format!("obj_{}", obj.path_id), |n| sanitize_filename(&n));
                let unity_obj = match handle.read() {
                    Ok(o) => o,
                    Err(_) => continue,
                };
                let json = match serde_json::to_string_pretty(unity_obj.class.properties()) {
                    Ok(j) => j,
                    Err(_) => continue,
                };
                let safe_type = sanitize_filename(&type_name);
                if !stats
                    .seen_mono
                    .insert(format!("{}/{}", safe_type, obj_name))
                {
                    continue;
                }
                let type_dir = format!("{}/{}", options.mono_dir, safe_type);
                let _ = std::fs::create_dir_all(&type_dir);
                if std::fs::write(format!("{}/{}.json", type_dir, obj_name), json).is_ok() {
                    stats.total_mono += 1;
                    *stats.mono_type_counts.entry(safe_type).or_insert(0) += 1;
                }
            }
        }
    }
    if options.do_tex {
        let path = Path::new(options.tex_dir);
        let mut entries = Vec::new();
        for sf_bytes in &chk.sf_data {
            let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                Ok(sf) => sf,
                Err(_) => continue,
            };
            let result = texture::extract_textures(&sf, &chk.resources, path);
            stats.total_tex_extracted += result.extracted;
            stats.total_tex_failed += result.failed;
            entries.extend(result.entries);
        }
        if let Err(error) = texture_layout::write_chk_index(path, options.chk_name, &entries) {
            eprintln!(
                "    FAIL write texture index for {}: {}",
                options.chk_name, error
            );
        }
    }
    true
}

/// Save a TextAsset to disk. Returns the file type label.
fn save_text_asset(output_dir: &str, name: &str, content: &[u8]) -> Option<&'static str> {
    if content.is_empty() {
        return None;
    }
    let safe_name = sanitize_filename(name);
    if let Some(parent) = std::path::Path::new(&safe_name).parent()
        && parent != std::path::Path::new("")
    {
        let full_parent = format!("{}/{}", output_dir, parent.display());
        let _ = std::fs::create_dir_all(&full_parent);
    }
    if let Ok(text) = std::str::from_utf8(content) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
            let path = format!("{}/{}.json", output_dir, safe_name);
            if let Ok(pretty) = serde_json::to_string_pretty(&json)
                && std::fs::write(&path, &pretty).is_ok()
            {
                return Some("JSON");
            }
        }
        if !text.trim().is_empty() {
            let path = format!("{}/{}.txt", output_dir, safe_name);
            if std::fs::write(&path, content).is_ok() {
                return Some("TEXT");
            }
        }
    }
    let path = format!("{}/{}.bytes", output_dir, safe_name);
    if std::fs::write(&path, content).is_ok() {
        return Some("BIN");
    }
    None
}

// ═══════════════════════════════════════════════════════════
// update — incremental cron-friendly extraction
// ═══════════════════════════════════════════════════════════

/// Machine-readable progress line: `@progress <phase> <done> <total> <label>`.
/// A total of 0 means the phase has no known size yet.
fn emit_progress(enabled: bool, phase: &str, done: u64, total: u64, label: &str) {
    if enabled {
        println!("@progress {phase} {done} {total} {label}");
    }
}

/// Byte-level progress for a chunk download phase. While enabled, a background task emits
/// `@progress <phase> <MiB done> <MiB total> <n>/<total> chunks` once a second.
struct DownloadProgress {
    enabled: bool,
    phase: &'static str,
    total_bytes: u64,
    chunks_total: u64,
    downloaded: Arc<AtomicU64>,
    chunks_done: Arc<AtomicU64>,
    ticker: Option<tokio::task::JoinHandle<()>>,
}

impl DownloadProgress {
    fn start(enabled: bool, phase: &'static str, total_bytes: u64, chunks_total: u64) -> Self {
        let downloaded = Arc::new(AtomicU64::new(0));
        let chunks_done = Arc::new(AtomicU64::new(0));
        let ticker = enabled.then(|| {
            let downloaded = Arc::clone(&downloaded);
            let chunks_done = Arc::clone(&chunks_done);
            tokio::spawn(async move {
                loop {
                    emit_download_progress(
                        phase,
                        downloaded.load(Ordering::Relaxed),
                        total_bytes,
                        chunks_done.load(Ordering::Relaxed),
                        chunks_total,
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            })
        });
        Self {
            enabled,
            phase,
            total_bytes,
            chunks_total,
            downloaded,
            chunks_done,
            ticker,
        }
    }

    fn chunk_done(&self) {
        self.chunks_done.fetch_add(1, Ordering::Relaxed);
    }

    fn finish(&mut self) {
        if let Some(ticker) = self.ticker.take() {
            ticker.abort();
        }
        if self.enabled {
            emit_download_progress(
                self.phase,
                self.total_bytes,
                self.total_bytes,
                self.chunks_done.load(Ordering::Relaxed),
                self.chunks_total,
            );
        }
    }
}

fn emit_download_progress(phase: &str, done: u64, total: u64, chunks: u64, chunks_total: u64) {
    const MIB: u64 = 1024 * 1024;
    emit_progress(
        true,
        phase,
        done.min(total) / MIB,
        total / MIB,
        &format!("{chunks}/{chunks_total} chunks"),
    );
}

/// Sum of compressed (downloaded) sizes for the listed archive entries.
fn compressed_total(cd: &ZipCentralDirectory, chks: &[(String, String)]) -> u64 {
    chks.iter()
        .filter_map(|(name, _)| cd.get(name))
        .map(|entry| entry.compressed_size)
        .sum()
}

#[allow(clippy::too_many_arguments)]
async fn cmd_update(
    client: &Client,
    output: &Path,
    state_path: &Path,
    force: bool,
    check: bool,
    no_textures: bool,
    progress: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    if let Some(parent) = state_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut lock_name = state_path.as_os_str().to_os_string();
    lock_name.push(".lock");
    let lock_path = std::path::PathBuf::from(lock_name);
    let lock = File::create(lock_path)?;
    if let Err(error) = lock.try_lock() {
        match error {
            std::fs::TryLockError::WouldBlock => {
                println!("another update is running");
                return Ok(());
            }
            std::fs::TryLockError::Error(error) => return Err(error.into()),
        }
    }

    let mut state = update::load_state(state_path)?;
    emit_progress(progress, "resolve", 0, 0, "checking launcher");
    let config = backend::core::endfield::config::resolve_game_config(
        client,
        &pipeline::default_resolve_params(),
    )
    .await?;
    let fingerprint = update::fingerprint(&config);
    let current = state.fingerprint == fingerprint;
    if current && !force {
        println!(
            "up to date ({} / res {})",
            config.version, config.res_version
        );
        return Ok(());
    }
    if check {
        println!(
            "update available ({} / res {})",
            config.version, config.res_version
        );
        std::process::exit(10);
    }

    emit_progress(progress, "resolve", 0, 0, "reading archive index");
    let resolved = pipeline::resolve_pipeline_from_config(client, config, true).await?;
    let mut chk_dirty = 0u64;
    let mut chk_ok = 0u64;
    let mut chk_failed = 0u64;
    let mut sort_failed = 0u64;
    let mut sort_moved = 0u64;
    let mut sort_chks = 0u64;
    let mut blc_dirty = 0u64;
    let mut blc_ok = 0u64;
    let mut blc_failed = 0u64;
    let mut stats = ExtractStats::default();
    let gamedata_dir = output.join("gamedata");
    let mono_dir = gamedata_dir.join("monobehaviour");
    let tex_dir = output.join("textures");
    std::fs::create_dir_all(&gamedata_dir)?;
    std::fs::create_dir_all(&mono_dir)?;
    if !no_textures {
        std::fs::create_dir_all(&tex_dir)?;
    }
    let gamedata = gamedata_dir.to_string_lossy().to_string();
    let mono = mono_dir.to_string_lossy().to_string();
    let textures = tex_dir.to_string_lossy().to_string();

    let mut existing_chks = HashSet::new();
    let mut dirty = Vec::new();
    let mut sort_list = Vec::new();
    for (dir, _) in VFS_DIRS {
        for (name, _) in pipeline::list_chk_files(&resolved.cd, dir) {
            let entry = resolved.cd.get(&name).expect("listed ZIP entry missing");
            let base = update::zip_digest(entry.crc32, entry.uncompressed_size);
            let mut digest = base.clone();
            if no_textures {
                // Tagged so a later run with textures sees these chks as dirty.
                digest.push_str("+notex");
            } else {
                digest.push_str(&format!("+tex{}", texture_layout::LAYOUT_VERSION));
            }
            existing_chks.insert(name.clone());
            if !no_textures && !force && state.chk.get(&name) == Some(&base) {
                sort_list.push((name, digest));
            } else if update::is_dirty(&state.chk, &name, &digest, force) {
                chk_dirty += 1;
                dirty.push((name, digest));
            }
        }
    }
    state.chk.retain(|name, _| existing_chks.contains(name));
    dirty.sort_by(|a, b| a.0.cmp(&b.0));
    sort_list.sort_by(|a, b| a.0.cmp(&b.0));
    // Downloads run as spawned tasks (at most 4 in flight) so they keep progressing while
    // the current chk is being decoded; memory stays bounded to the in-flight window.
    let mut chk_progress = DownloadProgress::start(
        progress,
        "chk",
        compressed_total(&resolved.cd, &dirty),
        chk_dirty,
    );
    let chk_downloaded = Arc::clone(&chk_progress.downloaded);
    let mut downloads = stream::iter(dirty.into_iter().map(|(name, digest)| {
        let client = client.clone();
        let cd = Arc::clone(&resolved.cd);
        let downloaded = Arc::clone(&chk_downloaded);
        tokio::spawn(async move {
            let result = pipeline::download_chk_counting(&client, &cd, &name, &downloaded)
                .await
                .map_err(|e| e.to_string());
            (name, digest, result)
        })
    }))
    .buffered(4);
    while let Some(joined) = downloads.next().await {
        let (name, digest, result) = joined?;
        chk_progress.chunk_done();
        let data = match result {
            Ok(data) => data,
            Err(e) => {
                println!("  {}: download error: {}", name, e);
                chk_failed += 1;
                continue;
            }
        };
        let ok = tokio::task::block_in_place(|| {
            process_chk_contents(
                &data,
                &ChkExtractOptions {
                    chk_name: &name,
                    do_json: true,
                    do_tex: !no_textures,
                    do_gamedata: true,
                    gamedata_dir: &gamedata,
                    mono_dir: &mono,
                    tex_dir: &textures,
                },
                &mut stats,
            )
        });
        if ok {
            state.chk.insert(name, digest);
            // Checkpoint per chk so an interrupted run resumes instead of starting over.
            update::save_state(state_path, &state)?;
            chk_ok += 1;
        } else {
            println!("  {}: not a valid VFS container", name);
            chk_failed += 1;
        }
    }
    chk_progress.finish();
    update::save_state(state_path, &state)?;

    if !sort_list.is_empty() {
        let sort_total = sort_list.len() as u64;
        sort_chks = sort_total;
        let mut sort_candidates = Vec::new();
        let mut sorted_chks = Vec::new();
        let mut sort_progress = DownloadProgress::start(
            progress,
            "sort",
            compressed_total(&resolved.cd, &sort_list),
            sort_total,
        );
        let sort_downloaded = Arc::clone(&sort_progress.downloaded);
        let mut sort_downloads = stream::iter(sort_list.into_iter().map(|(name, digest)| {
            let client = client.clone();
            let cd = Arc::clone(&resolved.cd);
            let downloaded = Arc::clone(&sort_downloaded);
            tokio::spawn(async move {
                let result = pipeline::download_chk_counting(&client, &cd, &name, &downloaded)
                    .await
                    .map_err(|e| e.to_string());
                (name, digest, result)
            })
        }))
        .buffered(4);
        while let Some(joined) = sort_downloads.next().await {
            let (name, digest, result) = joined?;
            sort_progress.chunk_done();
            let data = match result {
                Ok(data) => data,
                Err(error) => {
                    println!("  {name}: download error: {error}");
                    sort_failed += 1;
                    continue;
                }
            };
            let candidates = tokio::task::block_in_place(|| {
                let chk = extract_chk_contents(&data)?;
                Some(texture_sort::collect_candidates(&name, &chk))
            });
            let Some(candidates) = candidates else {
                println!("  {name}: not a valid VFS container");
                sort_failed += 1;
                continue;
            };
            sort_candidates.extend(candidates);
            sorted_chks.push((name, digest));
        }
        sort_progress.finish();
        if sort_failed == 0 {
            let report = texture_sort::sort_flat_textures(&tex_dir, &sort_candidates)?;
            sort_moved = report.moved as u64;
            for (name, _) in &sorted_chks {
                let entries = report
                    .moved_by_chk
                    .get(name)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                texture_layout::write_chk_index(&tex_dir, name, entries)?;
            }
            for (name, digest) in sorted_chks {
                state.chk.insert(name, digest);
            }
            update::save_state(state_path, &state)?;
            println!(
                "sort: moved {}, already sorted {}, ambiguous {} (see textures/_ambiguous/candidates.jsonl), missing {}, unmatched {}",
                report.moved,
                report.already_sorted,
                report.ambiguous,
                report.missing,
                report.unmatched
            );
            if report.missing > 0 {
                println!(
                    "{} textures have no file (lost to flat-layout name collisions, or never decodable); run update --force to re-extract",
                    report.missing
                );
            }
        }
    }

    if !no_textures {
        if let Err(error) = texture_layout::remove_chk_indexes_except(&tex_dir, &existing_chks) {
            eprintln!("  FAIL remove stale texture indexes: {error}");
        }
        if chk_failed == 0 && sort_failed == 0 {
            match texture_layout::remove_legacy_flat_textures(&tex_dir) {
                Ok(removed) if removed > 0 => println!("removed {removed} legacy flat textures"),
                Ok(_) => {}
                Err(error) => eprintln!("  FAIL remove legacy flat textures: {error}"),
            }
        }
        if let Err(error) = texture_layout::rebuild_index(&tex_dir) {
            eprintln!("  FAIL rebuild texture index: {error}");
        }
    }

    let staging_parent = output.join("decrypted/.staging");
    let decrypted = output.join("decrypted");
    std::fs::create_dir_all(&staging_parent)?;
    let blc_total = blc::ENCRYPTED_DIRS.len() as u64;
    for (i, dir) in blc::ENCRYPTED_DIRS.iter().enumerate() {
        emit_progress(progress, "blc", i as u64, blc_total, dir);
        let blc_name = format!("Endfield_Data/StreamingAssets/VFS/{0}/{0}.blc", dir);
        let Some(entry) = resolved.cd.get(&blc_name) else {
            state.blc_dirs.remove(*dir);
            continue;
        };
        let mut parts = vec![update::zip_digest(entry.crc32, entry.uncompressed_size)];
        if let Some(index) = &resolved.patch_index {
            let mut patch: Vec<String> = index
                .files
                .iter()
                .filter(|f| f.name.starts_with(&format!("{dir}/")))
                .map(|f| format!("{}:{}", f.name, f.md5))
                .collect();
            patch.sort();
            parts.extend(patch);
        }
        let digest = parts.join(";");
        if !update::is_dirty(&state.blc_dirs, dir, &digest, force) {
            continue;
        }
        blc_dirty += 1;
        let staged = staging_parent.join(dir);
        if staged.exists() {
            std::fs::remove_dir_all(&staged)?;
        }
        match blc::extract_directory(client, &resolved, dir, &staging_parent).await {
            Ok(result) => {
                std::fs::create_dir_all(&decrypted)?;
                let old = decrypted.join(format!(".old-{dir}"));
                if old.exists() {
                    std::fs::remove_dir_all(&old)?;
                }
                let final_dir = decrypted.join(dir);
                if final_dir.exists() {
                    std::fs::rename(&final_dir, &old)?;
                }
                std::fs::rename(&staged, &final_dir)?;
                if old.exists() {
                    std::fs::remove_dir_all(old)?;
                }
                if result.errors.is_empty() {
                    state.blc_dirs.insert((*dir).to_string(), digest);
                    blc_ok += 1;
                } else {
                    blc_failed += 1;
                }
            }
            Err(_) => {
                blc_failed += 1;
            }
        }
    }
    emit_progress(progress, "blc", blc_total, blc_total, "");
    update::save_state(state_path, &state)?;
    let failures = chk_failed + sort_failed + blc_failed;
    if failures == 0 {
        state.fingerprint = fingerprint;
        state.version = resolved.config.version.clone();
        state.res_version = resolved.config.res_version.clone();
        state.updated_at = Utc::now().to_rfc3339();
        update::save_state(state_path, &state)?;
    }
    std::fs::create_dir_all(output)?;
    let line = serde_json::json!({"at": Utc::now().to_rfc3339(), "version": resolved.config.version, "res_version": resolved.config.res_version,
        "chk_dirty": chk_dirty, "chk_ok": chk_ok, "chk_failed": chk_failed, "sort_chks": sort_chks, "sort_moved": sort_moved, "sort_failed": sort_failed, "blc_dirty": blc_dirty, "blc_ok": blc_ok, "blc_failed": blc_failed,
        "duration_secs": started.elapsed().as_secs_f64()});
    use std::io::Write;
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(output.join("update-log.jsonl"))?;
    writeln!(log, "{}", line)?;
    println!(
        "update: chk {chk_ok}/{chk_dirty} ok, blc {blc_ok}/{blc_dirty} ok, failures {failures}"
    );
    if failures > 0 {
        return Err("update had failures".into());
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// extract-file — single .chk extraction
// ═══════════════════════════════════════════════════════════

async fn cmd_extract_file(
    client: &Client,
    chk_name: &str,
    dir_filter: Option<String>,
    output: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Extract Single File ===\n");

    let resolved = pipeline::resolve_pipeline(client, false).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    // Find the .chk file
    let matching: Vec<String> = resolved
        .cd
        .filenames()
        .filter(|f| f.ends_with(".chk") && f.contains(chk_name))
        .filter(|f| dir_filter.as_ref().is_none_or(|d| f.contains(d.as_str())))
        .map(|f| f.to_string())
        .collect();

    if matching.is_empty() {
        println!("No .chk files matching '{}'", chk_name);
        return Ok(());
    }

    println!("Found {} matching file(s):", matching.len());
    for name in &matching {
        let size = resolved.cd.get(name).map_or(0, |e| e.uncompressed_size);
        println!("  {} ({})", name, format_bytes(size));
    }

    let target = &matching[0];
    println!("\nExtracting: {}", target);

    let data = pipeline::download_chk(client, &resolved.cd, target).await?;

    if !vfs_decrypt::is_valid_header(&data) {
        println!("Not a valid VFS container");
        println!("First 64 bytes:");
        hex_dump(&data, 64);
        return Ok(());
    }

    let assets = backend::extraction::unityfs_header::process_chk_file(&data)?;
    println!("{} assets from VFS pipeline\n", assets.len());

    std::fs::create_dir_all(output)?;

    for (i, asset) in assets.iter().enumerate() {
        if asset.is_unityfs {
            println!(
                "[{}] UnityFS \"{}\" ({}, {} entries)",
                i,
                asset.vfs_name,
                format_bytes(asset.data.len() as u64),
                asset.bundle_entries.len()
            );
            for entry in &asset.bundle_entries {
                println!(
                    "  {} ({} at offset {})",
                    entry.path,
                    format_bytes(entry.size as u64),
                    entry.offset
                );
            }
        } else {
            println!(
                "[{}] \"{}\" ({})",
                i,
                asset.vfs_name,
                format_bytes(asset.data.len() as u64)
            );
        }

        let save_path = format!("{}/{}", output, sanitize_filename(&asset.vfs_name));
        std::fs::write(&save_path, &asset.data)?;
        println!("  → saved to {}", save_path);
    }

    println!("\n=== Done ===");
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// decrypt — ChaCha20-encrypted VFS directories
// ═══════════════════════════════════════════════════════════

async fn cmd_decrypt(
    client: &Client,
    dir_filter: Option<String>,
    output: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ChaCha20 VFS Decryption ===\n");

    let resolved = pipeline::resolve_pipeline(client, true).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    let output_path = std::path::Path::new(output);
    std::fs::create_dir_all(output_path)?;

    let dirs: Vec<&str> = if let Some(ref d) = dir_filter {
        blc::ENCRYPTED_DIRS
            .iter()
            .filter(|h| h == &&d.as_str())
            .copied()
            .collect()
    } else {
        blc::ENCRYPTED_DIRS.to_vec()
    };

    if dirs.is_empty() {
        println!("No matching directories found.");
        if let Some(d) = &dir_filter {
            println!("Available: {:?}", blc::ENCRYPTED_DIRS);
            println!("You specified: {}", d);
        }
        return Ok(());
    }

    let mut total_extracted = 0usize;
    let mut total_sparkbuffer = 0usize;

    for dir_hash in &dirs {
        println!("{}", "=".repeat(60));
        println!("=== {} ===", dir_hash);

        match blc::extract_directory(client, &resolved, dir_hash, output_path).await {
            Ok(result) => {
                println!(
                    "  {} ({}): {}/{} files extracted, {} SparkBuffer→JSON",
                    result.group_name,
                    result.dir_hash,
                    result.extracted,
                    result.total_files,
                    result.sparkbuffer_parsed,
                );
                if !result.errors.is_empty() {
                    println!("  {} errors:", result.errors.len());
                    for e in result.errors.iter().take(5) {
                        println!("    {}", e);
                    }
                    if result.errors.len() > 5 {
                        println!("    ... and {} more", result.errors.len() - 5);
                    }
                }
                total_extracted += result.extracted;
                total_sparkbuffer += result.sparkbuffer_parsed;
            }
            Err(e) => {
                println!("  Error: {}", e);
            }
        }
    }

    println!("\n=== Decryption Summary ===");
    println!("  Directories: {}", dirs.len());
    println!("  Files extracted: {}", total_extracted);
    println!("  SparkBuffer→JSON: {}", total_sparkbuffer);
    println!("  Output: {}", output);
    println!("\n=== Done ===");
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// list — VFS directory listing
// ═══════════════════════════════════════════════════════════

async fn cmd_list(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== VFS Directory Listing ===\n");

    let resolved = pipeline::resolve_pipeline(client, true).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    let dirs = pipeline::list_vfs_directories(&resolved.cd);
    println!("VFS directories ({}):\n", dirs.len());

    for dir in &dirs {
        let files = pipeline::list_vfs_dir_files(&resolved.cd, dir);
        let total_size: u64 = files.iter().map(|(_, s)| *s).sum();
        println!(
            "  {}/  ({} files, {})",
            dir,
            files.len(),
            format_bytes(total_size)
        );
        for (name, size) in &files {
            let short = name.rsplit('/').next().unwrap_or(name);
            println!("    {} ({})", short, format_bytes(*size));
        }
    }

    if let Some(patch) = &resolved.patch_index {
        println!("\nPatch files ({}):", patch.files.len());
        for pf in &patch.files {
            println!("  {} ({})", pf.name, format_bytes(pf.size));
        }
    }

    println!("\n=== Done ===");
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// scan — asset name/path discovery
// ═══════════════════════════════════════════════════════════

const SEARCH_KEYWORDS: &[&str] = &[
    "operator",
    "character",
    "model",
    "mesh",
    "skeleton",
    "texture",
    "material",
    "avatar",
    "npc",
    "char_",
    "prefab",
    "anim",
    "skin",
    "weapon",
    "body",
];

async fn cmd_scan(client: &Client, scan_all: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Asset Name Scanner ===\n");

    let resolved = pipeline::resolve_pipeline(client, true).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    if let Some(patch) = &resolved.patch_index {
        println!("Patch version: {}", patch.version);
        println!("Chunk files: {}\n", patch.files.len());
    }

    let output_dir = "./output";
    std::fs::create_dir_all(output_dir)?;

    #[allow(dead_code)]
    struct ChkManifest {
        dir: String,
        chk_name: String,
        vfs_names: Vec<String>,
        bundle_paths: Vec<String>,
        embedded_strings: Vec<String>,
    }

    let mut manifests: Vec<ChkManifest> = Vec::new();

    for (dir, desc) in VFS_DIRS {
        println!("--- {} ({}) ---", dir, desc);
        let mut candidates = pipeline::list_chk_files(&resolved.cd, dir);
        if !scan_all {
            candidates.truncate(1);
        }

        for (i, (name, size)) in candidates.iter().enumerate() {
            println!(
                "  [{}/{}] {} ({})",
                i + 1,
                candidates.len(),
                name,
                format_bytes(*size)
            );
            let data = match pipeline::download_chk(client, &resolved.cd, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("    Download error: {}", e);
                    continue;
                }
            };

            if !vfs_decrypt::is_valid_header(&data) {
                println!("    Not a valid VFS container, skipping");
                continue;
            }

            let assets = match backend::extraction::unityfs_header::process_chk_file(&data) {
                Ok(a) => a,
                Err(e) => {
                    println!("    Pipeline error: {}", e);
                    continue;
                }
            };

            let mut vfs_names = Vec::new();
            let mut bundle_paths = Vec::new();
            let mut embedded_strings = Vec::new();
            let mut seen_strings = HashSet::new();

            for asset in &assets {
                if !vfs_names.contains(&asset.vfs_name) {
                    vfs_names.push(asset.vfs_name.clone());
                }
                for entry in &asset.bundle_entries {
                    if !bundle_paths.contains(&entry.path) {
                        bundle_paths.push(entry.path.clone());
                    }
                }
                // Extract readable strings from raw data
                let mut current = Vec::new();
                for &byte in &asset.data {
                    if (0x20..0x7F).contains(&byte) {
                        current.push(byte);
                    } else {
                        if current.len() >= 8
                            && let Ok(s) = std::str::from_utf8(&current)
                            && seen_strings.insert(s.to_string())
                        {
                            embedded_strings.push(s.to_string());
                        }
                        current.clear();
                    }
                }
                if current.len() >= 8
                    && let Ok(s) = std::str::from_utf8(&current)
                    && seen_strings.insert(s.to_string())
                {
                    embedded_strings.push(s.to_string());
                }
            }

            println!(
                "    {} assets, {} VFS names, {} bundle paths, {} embedded strings",
                assets.len(),
                vfs_names.len(),
                bundle_paths.len(),
                embedded_strings.len()
            );

            manifests.push(ChkManifest {
                dir: dir.to_string(),
                chk_name: name.clone(),
                vfs_names,
                bundle_paths,
                embedded_strings,
            });
        }
        println!();
    }

    // Keyword search
    println!("=== Keyword Search Results ===\n");
    for keyword in SEARCH_KEYWORDS {
        let kw_lower = keyword.to_lowercase();
        let mut matches: Vec<(&str, &str, &str)> = Vec::new();
        for manifest in &manifests {
            for name in &manifest.vfs_names {
                if name.to_lowercase().contains(&kw_lower) {
                    matches.push((&manifest.dir, "vfs", name));
                }
            }
            for path in &manifest.bundle_paths {
                if path.to_lowercase().contains(&kw_lower) {
                    matches.push((&manifest.dir, "bundle", path));
                }
            }
            for s in &manifest.embedded_strings {
                if s.to_lowercase().contains(&kw_lower) {
                    matches.push((&manifest.dir, "embedded", s));
                }
            }
        }
        if matches.is_empty() {
            println!("  \"{}\": no matches", keyword);
        } else {
            println!("  \"{}\" ({} matches):", keyword, matches.len());
            for (dir, source, value) in matches.iter().take(30) {
                println!("    [{}:{}] {}", dir, source, value);
            }
            if matches.len() > 30 {
                println!("    ... and {} more", matches.len() - 30);
            }
        }
    }

    println!("\n=== Done ===");
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// survey — object census
// ═══════════════════════════════════════════════════════════

async fn cmd_survey(client: &Client, scan_all: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Asset Survey ===\n");

    let resolved = pipeline::resolve_pipeline(client, false).await?;
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());

    let mut all_chks: Vec<ChkContents> = Vec::new();

    for (dir, desc) in VFS_DIRS {
        println!("--- {} ({}) ---", dir, desc);
        let mut candidates = pipeline::list_chk_files(&resolved.cd, dir);
        if !scan_all {
            candidates.truncate(1);
        }

        for (name, size) in &candidates {
            println!("  {} ({})", name, format_bytes(*size));
            let data = match pipeline::download_chk(client, &resolved.cd, name).await {
                Ok(d) => d,
                Err(e) => {
                    println!("    Download error: {}", e);
                    continue;
                }
            };
            if let Some(chk) = extract_chk_contents(&data) {
                println!(
                    "    {} SerializedFile(s), {} .resS stream(s)",
                    chk.sf_data.len(),
                    chk.resources.len()
                );
                all_chks.push(chk);
            }
        }
        println!();
    }

    // Pass 1: Class ID Census
    println!("╔══════════════════════════════════════════╗");
    println!("║  Class ID Census                         ║");
    println!("╚══════════════════════════════════════════╝\n");

    let mut class_counts: BTreeMap<i32, u64> = BTreeMap::new();
    let mut class_total_bytes: BTreeMap<i32, u64> = BTreeMap::new();
    let mut total_objects = 0u64;

    for chk in &all_chks {
        for sf_bytes in &chk.sf_data {
            let sf = match SerializedFileParser::from_bytes(sf_bytes.clone()) {
                Ok(sf) => sf,
                Err(_) => continue,
            };
            for obj in &sf.objects {
                *class_counts.entry(obj.type_id).or_insert(0) += 1;
                *class_total_bytes.entry(obj.type_id).or_insert(0) += obj.byte_size as u64;
                total_objects += 1;
            }
        }
    }

    println!("  Total objects: {}\n", total_objects);
    println!(
        "  {:>8} | {:>6} | {:>10} | Name",
        "ClassID", "Count", "Total Size"
    );
    println!("  {}", "-".repeat(55));

    let mut sorted_classes: Vec<_> = class_counts.iter().collect();
    sorted_classes.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

    for (class_id, count) in &sorted_classes {
        let total = class_total_bytes.get(class_id).copied().unwrap_or(0);
        println!(
            "  {:>8} | {:>6} | {:>10} | {}",
            class_id,
            count,
            format_bytes(total),
            class_name(**class_id)
        );
    }

    let total_sfs: usize = all_chks.iter().map(|c| c.sf_data.len()).sum();
    let total_res: usize = all_chks.iter().map(|c| c.resources.len()).sum();
    let total_res_bytes: u64 = all_chks
        .iter()
        .flat_map(|c| c.resources.values())
        .map(|v| v.len() as u64)
        .sum();

    println!("\n  SerializedFiles: {}", total_sfs);
    println!("  Total objects:   {}", total_objects);
    println!("  Unique classes:  {}", class_counts.len());
    println!("  Resource files:  {}", total_res);
    println!("  Resource data:   {}", format_bytes(total_res_bytes));

    println!("\n=== Done ===");
    Ok(())
}
