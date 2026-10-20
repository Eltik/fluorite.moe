//! Unified CLI for Arknights Endfield asset extraction.
//!
//! Subcommands:
//!   info          — show game config, version, patch info
//!   extract       — extract Unity assets (TextAssets, textures, MonoBehaviours, meshes)
//!   extract-file  — extract a single .chk by name
//!   decrypt       — extract BLC-catalogued VFS directories (tables, json, lua, audio, video)
//!   update        — incremental extraction of everything that changed upstream
//!   repair        — flip textures extracted before the orientation fix, in place
//!   health        — check the update run log for staleness and regressions
//!   schema-gen    — regenerate the MemoryPack schema from the client's IL2CPP files
//!   list          — list all VFS directories and files
//!   scan          — scan for asset names/paths
//!   survey        — survey all Unity objects, types, counts
use backend::core::endfield::config::resolve_game_config;
use backend::extraction::blc::{self, DirExtractResult};
use backend::extraction::gamedata::{self, GamedataStats};
use backend::extraction::health;
use backend::extraction::lua;
use backend::extraction::memorypack;
use backend::extraction::memorypack_store::{self, SyncInput, SyncOutcome};
use backend::extraction::mesh;
use backend::extraction::pipeline::{self, ResolvedPipeline, VFS_DIRS};
use backend::extraction::texture;
use backend::extraction::texture_layout::{self, FlipError};
use backend::extraction::texture_sort;
use backend::extraction::unity::SerializedFile;
use backend::extraction::unity_extract::{ChkContents, class_name, extract_chk_contents};
use backend::extraction::unityfs_header::process_chk_file;
use backend::extraction::update::{self, UpdateState};
use backend::extraction::utils::{format_bytes, hex_dump, remove_dir_all_retry, sanitize_filename};
use backend::extraction::vfs_decrypt;
use backend::extraction::zip_range::ZipCentralDirectory;
use chrono::Utc;
use clap::{Parser, Subcommand};
use futures::stream::{self, Stream, StreamExt};
use reqwest::Client;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

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
        /// Extract TextAssets (JSON, text, and binary files with their real extensions)
        #[arg(long)]
        json: bool,
        /// Extract Texture2D as PNG
        #[arg(long)]
        textures: bool,
        /// Extract MonoBehaviour + registry data
        #[arg(long)]
        gamedata: bool,
        /// Export Mesh as glTF binary (.glb)
        #[arg(long)]
        meshes: bool,
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
    /// Extract BLC-catalogued VFS directories (tables, JSON data, Lua, audio, video, ...)
    Decrypt {
        /// Specific directory hash or group name to extract (default: every non-bundle directory)
        #[arg(long)]
        dir: Option<String>,
        /// Skip the four voice-language groups (AudioChinese/English/Japanese/Korean)
        #[arg(long)]
        no_voice: bool,
        /// Output directory
        #[arg(long, default_value = "./output/decrypted")]
        output: String,
        /// Update state file; MemoryPack schemas are read from `memorypack-schema/` beside it
        #[arg(long, default_value = "./data/update-state.json")]
        state: String,
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
        /// Do not export meshes as .glb
        #[arg(long)]
        no_meshes: bool,
        /// Skip a BLC directory by hash or group name (repeatable or comma-separated), e.g.
        /// 55FC21C6 or Video
        #[arg(long, value_delimiter = ',')]
        skip_dir: Vec<String>,
        /// Skip the four voice-language groups (AudioChinese/English/Japanese/Korean, ~1.5 GB each)
        #[arg(long)]
        no_voice: bool,
        /// Emit `@progress` lines for scripts/auto-update.sh to render
        #[arg(long)]
        progress: bool,
        /// Do not regenerate the MemoryPack schema when the client binary changed
        #[arg(long)]
        no_schema: bool,
    },
    /// Generate, validate and install the MemoryPack schema for the current client, then
    /// re-decode JsonData; exit 2 if it would decode fewer files, 3 if the client is unreadable
    SchemaGen {
        /// Persistent update state file; schemas go to `memorypack-schema/` beside it
        #[arg(long, default_value = "./data/update-state.json")]
        state: String,
        /// Output directory (JsonData is found under `<output>/decrypted`)
        #[arg(long, default_value = "./output")]
        output: String,
        /// Local GameAssembly.dll instead of downloading it (needs --metadata, --game-version)
        #[arg(long, requires_all = ["metadata", "game_version"])]
        dll: Option<String>,
        /// Local global-metadata.dat instead of downloading it
        #[arg(long, requires = "dll")]
        metadata: Option<String>,
        /// Game version of the local client files (downloads use the live version)
        #[arg(long, requires = "dll")]
        game_version: Option<String>,
        /// JsonData `Json/` tree to learn and validate against (default: the JsonData group's)
        #[arg(long)]
        json_dir: Option<String>,
        /// Install even if the new schema decodes fewer files than the current one
        #[arg(long)]
        force: bool,
    },
    /// Flip PNGs extracted before the orientation fix in place (no download)
    Repair {
        /// Output directory
        #[arg(long, default_value = "./output")]
        output: String,
        /// Persistent update state file
        #[arg(long, default_value = "./data/update-state.json")]
        state: String,
    },
    /// Check `<output>/update-log.jsonl`; exit 0 if healthy, 1 on warnings, 2 on failures
    Health {
        /// Output directory
        #[arg(long, default_value = "./output")]
        output: String,
        /// Newest run older than this many hours fails the check
        #[arg(long, default_value_t = 26)]
        max_age_hours: i64,
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
    // A CDN connection can stall without closing; the read timeout turns that into an error
    // the download retry loop can recover from instead of hanging the run forever.
    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()?;

    match cli.command {
        Commands::Info => cmd_info(&client).await,
        Commands::Extract {
            json,
            textures,
            gamedata,
            meshes,
            all,
            dir,
            output,
            scan_all,
        } => {
            let everything = all || !(json || textures || gamedata || meshes);
            let modes = ExtractModes {
                json: json || everything,
                textures: textures || everything,
                gamedata: gamedata || everything,
                meshes: meshes || everything,
            };
            cmd_extract(&client, modes, dir, &output, scan_all).await
        }
        Commands::ExtractFile {
            chk_name,
            dir,
            output,
        } => cmd_extract_file(&client, &chk_name, dir, &output).await,
        Commands::Decrypt {
            dir,
            no_voice,
            output,
            state,
        } => cmd_decrypt(&client, dir, no_voice, &output, Path::new(&state)).await,
        Commands::Update {
            output,
            state,
            force,
            check,
            no_textures,
            no_meshes,
            skip_dir,
            no_voice,
            progress,
            no_schema,
        } => {
            cmd_update(
                &client,
                Path::new(&output),
                Path::new(&state),
                UpdateOptions {
                    force,
                    check,
                    no_textures,
                    no_meshes,
                    skip_dirs: skip_dir_hashes(&skip_dir, no_voice),
                    progress,
                    no_schema,
                },
            )
            .await
        }
        Commands::SchemaGen {
            state,
            output,
            dll,
            metadata,
            game_version,
            json_dir,
            force,
        } => {
            let local = dll.zip(metadata).map(|(dll, metadata)| LocalClient {
                dll: PathBuf::from(dll),
                metadata: PathBuf::from(metadata),
                game_version: game_version.unwrap_or_default(),
            });
            let options = SchemaGenOptions {
                state: PathBuf::from(state),
                output: PathBuf::from(output),
                local,
                json_dir: json_dir.map(PathBuf::from),
                force,
            };
            let code = cmd_schema_gen(&client, options).await?;
            std::process::exit(code);
        }
        Commands::Repair { output, state } => cmd_repair(Path::new(&output), Path::new(&state)),
        Commands::Health {
            output,
            max_age_hours,
        } => cmd_health(Path::new(&output), max_age_hours),
        Commands::List => cmd_list(&client).await,
        Commands::Scan { scan_all } => cmd_scan(&client, scan_all).await,
        Commands::Survey { scan_all } => cmd_survey(&client, scan_all).await,
    }
}

// ═══════════════════════════════════════════════════════════
// shared helpers
// ═══════════════════════════════════════════════════════════

/// `--skip-dir` values (hashes or group names) as uppercase hashes, plus the voice groups
/// with `--no-voice`.
fn skip_dir_hashes(skip_dir: &[String], no_voice: bool) -> Vec<String> {
    let mut hashes: Vec<String> = skip_dir
        .iter()
        .filter(|name| !name.trim().is_empty())
        .map(|name| pipeline::resolve_dir_name(name))
        .collect();
    if no_voice {
        hashes.extend(
            pipeline::VOICE_GROUPS
                .iter()
                .map(|(hash, _)| hash.to_string()),
        );
    }
    hashes.sort();
    hashes.dedup();
    hashes
}

/// Print the version and archive size most subcommands open with.
fn print_pipeline_summary(resolved: &ResolvedPipeline) {
    println!("Version: {}", resolved.config.version);
    println!("ZIP entries: {}\n", resolved.cd.len());
}

/// `.chk` files of a VFS directory to process: all of them, or only the smallest one.
fn chk_candidates(cd: &ZipCentralDirectory, dir: &str, scan_all: bool) -> Vec<(String, u64)> {
    let mut candidates = pipeline::list_chk_files(cd, dir);
    if !scan_all {
        candidates.truncate(1);
    }
    candidates
}

/// Download a `.chk`, reporting a failure instead of aborting the run.
async fn download_or_report(
    client: &Client,
    cd: &ZipCentralDirectory,
    name: &str,
) -> Option<Vec<u8>> {
    match pipeline::download_chk(client, cd, name).await {
        Ok(data) => Some(data),
        Err(e) => {
            println!("    Download error: {}", e);
            None
        }
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
        println!("  Resource: {} v{} {}", res.name, res.version, res.path);
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

#[derive(Clone, Copy)]
struct ExtractModes {
    json: bool,
    textures: bool,
    gamedata: bool,
    meshes: bool,
}

/// Per-kind output directories under the extraction output root.
struct OutputDirs {
    gamedata: PathBuf,
    textures: PathBuf,
    meshes: PathBuf,
}

impl OutputDirs {
    fn new(output: &Path) -> Self {
        Self {
            gamedata: output.join("gamedata"),
            textures: output.join("textures"),
            meshes: output.join("meshes"),
        }
    }
}

async fn cmd_extract(
    client: &Client,
    modes: ExtractModes,
    dir_filter: Option<String>,
    output: &str,
    scan_all: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Endfield Asset Extraction ===\n");

    let names: Vec<&str> = [
        modes.json.then_some("TextAssets"),
        modes.textures.then_some("textures"),
        modes.gamedata.then_some("gamedata"),
        modes.meshes.then_some("meshes"),
    ]
    .into_iter()
    .flatten()
    .collect();
    println!("Modes: {}", names.join(", "));

    let resolved = pipeline::resolve_pipeline(client, false).await?;
    print_pipeline_summary(&resolved);

    let dirs: Vec<(&str, &str)> = match &dir_filter {
        Some(d) => VFS_DIRS
            .iter()
            .filter(|(hash, _)| *hash == d.as_str())
            .copied()
            .collect(),
        None => VFS_DIRS.to_vec(),
    };

    let output = Path::new(output);
    fs::create_dir_all(output)?;
    let out_dirs = OutputDirs::new(output);

    let mut stats = ExtractStats::default();

    for (dir, desc) in &dirs {
        println!("--- {} ({}) ---", dir, desc);
        let candidates = chk_candidates(&resolved.cd, dir, scan_all);
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

            let Some(data) = download_or_report(client, &resolved.cd, name).await else {
                continue;
            };

            let options = ChkExtractOptions {
                chk_name: name,
                modes,
                skip_existing_textures: false,
                out_dirs: &out_dirs,
            };
            // Failures are already reported inside; `extract` has no state to keep consistent.
            let _ = process_chk_contents(&data, &options, &mut stats);
        }
        println!();
    }

    if modes.textures {
        match texture_layout::rebuild_index(&out_dirs.textures) {
            Ok(count) => println!("  Texture index entries: {count}"),
            Err(error) => eprintln!("  FAIL rebuild texture index: {error}"),
        }
    }

    println!("=== Extraction Summary ===");
    stats.print(modes);
    println!("  Output: {}", output.display());
    println!("\n=== Done ===");
    Ok(())
}

#[derive(Default)]
struct ExtractStats {
    gamedata: GamedataStats,
    total_tex_extracted: u64,
    total_tex_existing: u64,
    total_tex_failed: u64,
    total_meshes: u64,
    total_mesh_failed: u64,
}

impl ExtractStats {
    fn print(&self, modes: ExtractModes) {
        let g = &self.gamedata;
        if modes.json {
            println!(
                "  TextAssets: {} (JSON: {}, Text: {}, Binary: {})",
                g.text_assets, g.json, g.text, g.binary
            );
        }
        if modes.gamedata {
            println!("  MonoBehaviours: {}", g.mono);
            let mut types: Vec<_> = g.mono_type_counts.iter().collect();
            types.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            for (type_name, count) in types.iter().take(20) {
                println!("    {}: {}", type_name, count);
            }
        }
        if modes.textures {
            println!(
                "  Textures extracted: {}, kept: {}, failed: {}",
                self.total_tex_extracted, self.total_tex_existing, self.total_tex_failed
            );
        }
        if modes.meshes {
            println!(
                "  Meshes exported: {}, failed: {}",
                self.total_meshes, self.total_mesh_failed
            );
        }
    }
}

struct ChkExtractOptions<'a> {
    chk_name: &'a str,
    modes: ExtractModes,
    /// Keep PNGs already on disk (only safe when they match the current texture layout).
    skip_existing_textures: bool,
    out_dirs: &'a OutputDirs,
}

/// Why [`process_chk_contents`] rejected a chunk.
enum ChkFailure {
    NotContainer,
    NoSerializedFile,
    WriteErrors(u64),
}

impl std::fmt::Display for ChkFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChkFailure::NotContainer => write!(f, "not a valid VFS container"),
            ChkFailure::NoSerializedFile => write!(f, "no SerializedFile could be parsed"),
            ChkFailure::WriteErrors(count) => write!(f, "{count} file(s) could not be written"),
        }
    }
}

/// Process one downloaded `.chk`.
///
/// Objects that fail to decode are skipped, but any output that cannot be written fails the
/// whole chunk so it is retried instead of being recorded as extracted.
fn process_chk_contents(
    data: &[u8],
    options: &ChkExtractOptions<'_>,
    stats: &mut ExtractStats,
) -> Result<(), ChkFailure> {
    let Some(chk) = extract_chk_contents(data) else {
        println!("    Not a valid VFS container, skipping");
        return Err(ChkFailure::NotContainer);
    };
    println!(
        "    {} SerializedFile(s), {} .resS stream(s)",
        chk.sf_data.len(),
        chk.resources.len()
    );
    let modes = options.modes;
    let out_dirs = options.out_dirs;
    let mut tex_entries = Vec::new();
    let write_errors_before = stats.gamedata.write_errors;
    let mut write_errors = 0u64;
    let mut parsed = 0usize;
    for sf_bytes in &chk.sf_data {
        let Ok(sf) = SerializedFile::parse(sf_bytes.clone()) else {
            continue;
        };
        parsed += 1;
        let containers = texture_layout::container_map(&sf);
        // Scoped to one SerializedFile so every object's output path is deterministic.
        let mut seen = HashSet::new();
        if modes.json {
            gamedata::extract_text_assets(
                &sf,
                &containers,
                &out_dirs.gamedata,
                &mut seen,
                &mut stats.gamedata,
            );
        }
        if modes.gamedata {
            gamedata::extract_monobehaviours(
                &sf,
                &containers,
                &out_dirs.gamedata,
                &mut seen,
                &mut stats.gamedata,
            );
        }
        if modes.textures {
            let result = texture::extract_textures(
                &sf,
                &chk.resources,
                &out_dirs.textures,
                options.skip_existing_textures,
            );
            stats.total_tex_extracted += result.extracted;
            stats.total_tex_existing += result.existing;
            stats.total_tex_failed += result.failed;
            write_errors += result.write_errors;
            tex_entries.extend(result.entries);
        }
        if modes.meshes {
            let result = mesh::extract_meshes(
                &sf,
                &containers,
                &chk.resources,
                &out_dirs.meshes,
                &mut seen,
            );
            stats.total_meshes += result.exported;
            stats.total_mesh_failed += result.failed;
            write_errors += result.write_errors;
        }
    }
    if modes.textures
        && let Err(error) =
            texture_layout::write_chk_index(&out_dirs.textures, options.chk_name, &tex_entries)
    {
        eprintln!(
            "    FAIL write texture index for {}: {}",
            options.chk_name, error
        );
        write_errors += 1;
    }
    write_errors += stats.gamedata.write_errors - write_errors_before;
    if write_errors > 0 {
        println!("    {write_errors} file(s) could not be written");
        return Err(ChkFailure::WriteErrors(write_errors));
    }
    if parsed == 0 && !chk.sf_data.is_empty() {
        println!("    no SerializedFile could be parsed");
        return Err(ChkFailure::NoSerializedFile);
    }
    Ok(())
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
fn compressed_total<'a>(cd: &ZipCentralDirectory, names: impl Iterator<Item = &'a String>) -> u64 {
    names
        .filter_map(|name| cd.get(name))
        .map(|entry| entry.compressed_size)
        .sum()
}

/// One finished chunk download: its name, the caller's data, and the bytes or an error.
type ChunkDownload<T> = (String, T, Result<Vec<u8>, String>);

/// Download chunks as spawned tasks (at most 4 in flight), yielded in input order.
///
/// Spawning keeps downloads progressing while the caller decodes the current chunk; memory
/// stays bounded to the in-flight window.
fn download_chunks<T: Send + 'static>(
    client: &Client,
    cd: &Arc<ZipCentralDirectory>,
    progress: &DownloadProgress,
    chunks: Vec<(String, T)>,
) -> impl Stream<Item = Result<ChunkDownload<T>, tokio::task::JoinError>> + use<T> {
    let client = client.clone();
    let cd = Arc::clone(cd);
    let downloaded = Arc::clone(&progress.downloaded);
    stream::iter(chunks.into_iter().map(move |(name, extra)| {
        let client = client.clone();
        let cd = Arc::clone(&cd);
        let downloaded = Arc::clone(&downloaded);
        tokio::spawn(async move {
            let result = pipeline::download_chk_counting(&client, &cd, &name, &downloaded)
                .await
                .map_err(|e| e.to_string());
            (name, extra, result)
        })
    }))
    .buffered(4)
}

struct UpdateOptions {
    force: bool,
    check: bool,
    no_textures: bool,
    no_meshes: bool,
    /// BLC directory hashes to leave untouched (not extracted, state unchanged).
    skip_dirs: Vec<String>,
    progress: bool,
    /// Skip regenerating the MemoryPack schema.
    no_schema: bool,
}

/// Inputs shared by every phase of one `update` run.
struct UpdateContext<'a> {
    client: &'a Client,
    resolved: &'a ResolvedPipeline,
    state_path: &'a Path,
    force: bool,
    progress: bool,
    /// MemoryPack schemas JsonData is decoded with.
    schemas: &'a memorypack::SchemaSet,
}

/// Lock the update state so concurrent `update`/`repair` runs bail out instead of racing.
/// Returns `None` when another run holds the lock.
fn lock_state(state_path: &Path) -> Result<Option<File>, Box<dyn std::error::Error>> {
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut lock_name = state_path.as_os_str().to_os_string();
    lock_name.push(".lock");
    let lock = File::create(PathBuf::from(lock_name))?;
    match lock.try_lock() {
        Ok(()) => Ok(Some(lock)),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

async fn cmd_update(
    client: &Client,
    output: &Path,
    state_path: &Path,
    options: UpdateOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let UpdateOptions {
        force,
        check,
        no_textures,
        no_meshes,
        skip_dirs,
        progress,
        no_schema,
    } = options;
    let started = Instant::now();
    let Some(_lock) = lock_state(state_path)? else {
        println!("another update is running");
        return Ok(());
    };

    let mut state = update::load_state(state_path)?;
    emit_progress(progress, "resolve", 0, 0, "checking launcher");
    let config = resolve_game_config(client, &pipeline::default_resolve_params()).await?;
    let fingerprint = update::fingerprint(&config);
    let up_to_date = state.fingerprint == fingerprint;
    // `--check` reports whether upstream changed; `--force` only affects a real run.
    if up_to_date && (check || !force) {
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
    let schemas = load_schema_set(state_path)?;
    let ctx = UpdateContext {
        client,
        resolved: &resolved,
        state_path,
        force,
        progress,
        schemas: &schemas,
    };
    let out_dirs = OutputDirs::new(output);
    fs::create_dir_all(&out_dirs.gamedata)?;
    let modes = ExtractModes {
        json: true,
        textures: !no_textures,
        gamedata: true,
        meshes: !no_meshes,
    };

    let plan = plan_chunks(&resolved.cd, &state, force, modes);
    state.chk.retain(|name, _| plan.existing.contains(name));
    let chk = extract_dirty_chunks(&ctx, &mut state, &out_dirs, modes, plan.dirty).await?;
    let sort = sort_legacy_chunks(&ctx, &mut state, &out_dirs.textures, plan.sort).await?;
    if !no_textures {
        clean_texture_outputs(
            &out_dirs.textures,
            &plan.existing,
            &state,
            chk.failed == 0 && sort.failed == 0,
        );
    }
    if chk.failed == 0 {
        clean_legacy_gamedata(&out_dirs.gamedata, &state);
    }
    let blc = extract_blc_directories(&ctx, &mut state, output, &skip_dirs).await?;
    let schema = if no_schema {
        SchemaRun::default()
    } else {
        regenerate_schema(&ctx, &mut state, output).await
    };
    let schema_version = schema.version.clone().or_else(|| {
        schemas
            .newest()
            .and_then(|(_, s)| s.game_version())
            .map(str::to_owned)
    });

    let failures = chk.failed + sort.failed + blc.failed;
    if !skip_dirs.is_empty() {
        // Left unrecorded so a later run without --skip-dir still fetches those directories.
        println!("skipped BLC directories: {}", skip_dirs.join(", "));
    } else if failures == 0 {
        state.fingerprint = fingerprint;
        state.version = resolved.config.version.clone();
        state.res_version = resolved.config.res_version.clone();
        state.updated_at = Utc::now().to_rfc3339();
        update::save_state(state_path, &state)?;
    }
    fs::create_dir_all(output)?;
    let mut line = serde_json::json!({"at": Utc::now().to_rfc3339(), "version": resolved.config.version, "res_version": resolved.config.res_version,
        "chk_dirty": chk.dirty, "chk_ok": chk.ok, "chk_failed": chk.failed, "sort_chks": sort.chks, "sort_moved": sort.moved, "sort_failed": sort.failed, "blc_dirty": blc.dirty, "blc_ok": blc.ok, "blc_failed": blc.failed,
        "tex_extracted": chk.tex_extracted, "tex_failed": chk.tex_failed, "mesh_exported": chk.mesh_exported, "mesh_failed": chk.mesh_failed,
        "lua_decrypted": blc.lua_decrypted, "memorypack_decoded": blc.memorypack_decoded, "memorypack_failed": blc.memorypack_failed,
        "schema_version": schema_version, "schema_regenerated": schema.regenerated,
        "duration_secs": started.elapsed().as_secs_f64()});
    if !blc.memorypack_by_schema.is_empty() {
        line["memorypack_schemas"] = serde_json::json!(blc.memorypack_by_schema);
    }
    if let Some((decoded, failed)) = schema.decoded {
        line["schema_decoded"] = decoded.into();
        line["schema_failed"] = failed.into();
    }
    if let Some(error) = &schema.error {
        line["schema_error"] = error.as_str().into();
    }
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(output.join("update-log.jsonl"))?;
    writeln!(log, "{}", line)?;
    println!(
        "update: chk {}/{} ok, blc {}/{} ok, failures {failures}",
        chk.ok, chk.dirty, blc.ok, blc.dirty
    );
    if failures > 0 {
        return Err("update had failures".into());
    }
    Ok(())
}

/// How each upstream `.chk` is handled by this update run.
struct ChunkPlan {
    /// Every `.chk` currently in the archive.
    existing: HashSet<String>,
    /// Chunks to extract: name, new digest, and whether PNGs already on disk may be kept.
    dirty: Vec<(String, (String, bool))>,
    /// Unchanged chunks whose PNGs still sit in the legacy flat layout, with their new digest.
    sort: Vec<(String, String)>,
}

fn plan_chunks(
    cd: &ZipCentralDirectory,
    state: &UpdateState,
    force: bool,
    modes: ExtractModes,
) -> ChunkPlan {
    let mut existing = HashSet::new();
    let mut dirty = Vec::new();
    let mut sort = Vec::new();
    for (dir, _) in VFS_DIRS {
        for (name, _) in pipeline::list_chk_files(cd, dir) {
            let entry = cd.get(&name).expect("listed ZIP entry missing");
            let base = update::zip_digest(entry.crc32, entry.uncompressed_size);
            let digest = update::chk_digest(&base, modes.textures, modes.meshes);
            existing.insert(name.clone());
            if modes.textures && !force && state.chk.get(&name) == Some(&base) {
                sort.push((name, digest));
            } else if update::is_dirty(&state.chk, &name, &digest, force) {
                // PNGs from an unchanged chunk in the current layout can be kept as-is.
                let keep_textures = !force
                    && state.chk.get(&name).is_some_and(|old| {
                        old.strip_prefix(&base)
                            .is_some_and(|tags| tags.starts_with('+'))
                            && update::textures_current(Some(old))
                    });
                dirty.push((name, (digest, keep_textures)));
            }
        }
    }
    dirty.sort_by(|a, b| a.0.cmp(&b.0));
    sort.sort_by(|a, b| a.0.cmp(&b.0));
    ChunkPlan {
        existing,
        dirty,
        sort,
    }
}

struct ChunkCounts {
    dirty: u64,
    ok: u64,
    failed: u64,
    tex_extracted: u64,
    tex_failed: u64,
    mesh_exported: u64,
    mesh_failed: u64,
}

/// Download and extract every dirty chunk, checkpointing the state after each one.
async fn extract_dirty_chunks(
    ctx: &UpdateContext<'_>,
    state: &mut UpdateState,
    out_dirs: &OutputDirs,
    modes: ExtractModes,
    dirty: Vec<(String, (String, bool))>,
) -> Result<ChunkCounts, Box<dyn std::error::Error>> {
    let cd = &ctx.resolved.cd;
    let mut counts = ChunkCounts {
        dirty: dirty.len() as u64,
        ok: 0,
        failed: 0,
        tex_extracted: 0,
        tex_failed: 0,
        mesh_exported: 0,
        mesh_failed: 0,
    };
    let mut stats = ExtractStats::default();
    let mut progress = DownloadProgress::start(
        ctx.progress,
        "chk",
        compressed_total(cd, dirty.iter().map(|(name, _)| name)),
        counts.dirty,
    );
    let mut downloads = download_chunks(ctx.client, cd, &progress, dirty);
    while let Some(joined) = downloads.next().await {
        let (name, (digest, keep_textures), result) = joined?;
        progress.chunk_done();
        let data = match result {
            Ok(data) => data,
            Err(e) => {
                println!("  {}: download error: {}", name, e);
                counts.failed += 1;
                continue;
            }
        };
        // Decoding rewrites `+tex1` PNGs in the new orientation; if this run is interrupted,
        // `repair` must not flip the mix of old and new files, so mark them untrusted first.
        if !keep_textures
            && let Some(old) = state.chk.get(&name)
            && update::mark_textures_flipped(old).is_some()
        {
            let stale = update::mark_textures_stale(old);
            state.chk.insert(name.clone(), stale);
            update::save_state(ctx.state_path, state)?;
        }
        let result = tokio::task::block_in_place(|| {
            process_chk_contents(
                &data,
                &ChkExtractOptions {
                    chk_name: &name,
                    modes,
                    skip_existing_textures: keep_textures,
                    out_dirs,
                },
                &mut stats,
            )
        });
        match result {
            Ok(()) => {
                state.chk.insert(name, digest);
                // Checkpoint per chk so an interrupted run resumes instead of starting over.
                update::save_state(ctx.state_path, state)?;
                counts.ok += 1;
            }
            Err(failure) => {
                println!("  {name}: {failure}");
                counts.failed += 1;
            }
        }
    }
    progress.finish();
    update::save_state(ctx.state_path, state)?;
    if counts.dirty > 0 {
        stats.print(modes);
    }
    counts.tex_extracted = stats.total_tex_extracted;
    counts.tex_failed = stats.total_tex_failed;
    counts.mesh_exported = stats.total_meshes;
    counts.mesh_failed = stats.total_mesh_failed;
    Ok(counts)
}

#[derive(Default)]
struct SortCounts {
    chks: u64,
    moved: u64,
    failed: u64,
}

/// Move PNGs of unchanged chunks out of the legacy flat layout into the container layout.
async fn sort_legacy_chunks(
    ctx: &UpdateContext<'_>,
    state: &mut UpdateState,
    tex_dir: &Path,
    sort_list: Vec<(String, String)>,
) -> Result<SortCounts, Box<dyn std::error::Error>> {
    let mut counts = SortCounts::default();
    if sort_list.is_empty() {
        return Ok(counts);
    }
    let cd = &ctx.resolved.cd;
    counts.chks = sort_list.len() as u64;
    let mut sort_candidates = Vec::new();
    let mut sorted_chks = Vec::new();
    let mut progress = DownloadProgress::start(
        ctx.progress,
        "sort",
        compressed_total(cd, sort_list.iter().map(|(name, _)| name)),
        counts.chks,
    );
    let mut downloads = download_chunks(ctx.client, cd, &progress, sort_list);
    while let Some(joined) = downloads.next().await {
        let (name, digest, result) = joined?;
        progress.chunk_done();
        let data = match result {
            Ok(data) => data,
            Err(error) => {
                println!("  {name}: download error: {error}");
                counts.failed += 1;
                continue;
            }
        };
        let candidates = tokio::task::block_in_place(|| {
            let chk = extract_chk_contents(&data)?;
            Some(texture_sort::collect_candidates(&name, &chk))
        });
        let Some(candidates) = candidates else {
            println!("  {name}: not a valid VFS container");
            counts.failed += 1;
            continue;
        };
        sort_candidates.extend(candidates);
        sorted_chks.push((name, digest));
    }
    progress.finish();
    if counts.failed > 0 {
        return Ok(counts);
    }
    let report = texture_sort::sort_flat_textures(tex_dir, &sort_candidates)?;
    counts.moved = report.moved as u64;
    for (name, _) in &sorted_chks {
        let entries = report
            .moved_by_chk
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        texture_layout::write_chk_index(tex_dir, name, entries)?;
    }
    // Sorted PNGs come from the old decoder (upside down) and the chunk's gamedata and
    // meshes are not extracted yet: record `+tex1` so `repair` flips them and the next
    // update re-extracts the rest.
    for (name, digest) in sorted_chks {
        state
            .chk
            .insert(name, update::sorted_legacy_digest(&digest));
    }
    update::save_state(ctx.state_path, state)?;
    println!(
        "sort: moved {}, already sorted {}, ambiguous {} (see textures/_ambiguous/candidates.jsonl), missing {}, unmatched {}",
        report.moved, report.already_sorted, report.ambiguous, report.missing, report.unmatched
    );
    if report.missing > 0 {
        println!(
            "{} textures have no file (lost to flat-layout name collisions, or never decodable); run update --force to re-extract",
            report.missing
        );
    }
    Ok(counts)
}

/// Prune stale texture indexes and rebuild the combined one; once every chunk extracted
/// cleanly (`remove_legacy`), also remove the legacy flat layout's leftovers.
fn clean_texture_outputs(
    tex_dir: &Path,
    existing_chks: &HashSet<String>,
    state: &UpdateState,
    remove_legacy: bool,
) {
    if let Err(error) = texture_layout::remove_chk_indexes_except(tex_dir, existing_chks) {
        eprintln!("  FAIL remove stale texture indexes: {error}");
    }
    if remove_legacy {
        match texture_layout::remove_legacy_flat_textures(tex_dir) {
            Ok(removed) if removed > 0 => println!("removed {removed} legacy flat textures"),
            Ok(_) => {}
            Err(error) => eprintln!("  FAIL remove legacy flat textures: {error}"),
        }
        // Every chunk now has its full texture set in the container layout, which
        // supersedes the textures the legacy sort could not place.
        let all_current = state
            .chk
            .values()
            .all(|digest| update::textures_current(Some(digest)));
        let ambiguous = tex_dir.join("_ambiguous");
        if all_current && ambiguous.is_dir() {
            match remove_dir_all_retry(&ambiguous) {
                Ok(()) => println!("removed textures/_ambiguous"),
                Err(error) => eprintln!("  FAIL remove textures/_ambiguous: {error}"),
            }
        }
    }
    if let Err(error) = texture_layout::rebuild_index(tex_dir) {
        eprintln!("  FAIL rebuild texture index: {error}");
    }
}

/// Remove the legacy flat gamedata once every chunk was re-extracted into the container layout.
fn clean_legacy_gamedata(gamedata_dir: &Path, state: &UpdateState) {
    let all_current = state
        .chk
        .values()
        .all(|digest| update::extract_current(digest));
    if all_current {
        match gamedata::remove_legacy_gamedata(gamedata_dir) {
            Ok(removed) if removed > 0 => println!("removed {removed} legacy gamedata entries"),
            Ok(_) => {}
            Err(error) => eprintln!("  FAIL remove legacy gamedata: {error}"),
        }
    }
}

#[derive(Default)]
struct BlcCounts {
    dirty: u64,
    ok: u64,
    failed: u64,
    lua_decrypted: u64,
    memorypack_decoded: u64,
    memorypack_failed: u64,
    memorypack_by_schema: BTreeMap<String, u64>,
}

impl BlcCounts {
    /// Sum a directory's decode counts, whether or not the directory is installed.
    fn add_decodes(&mut self, result: &DirExtractResult) {
        self.lua_decrypted += result.lua_decrypted as u64;
        self.memorypack_decoded += result.memorypack_decoded as u64;
        self.memorypack_failed += result.memorypack_failed as u64;
        for (label, n) in &result.memorypack_by_schema {
            *self.memorypack_by_schema.entry(label.clone()).or_default() += n;
        }
    }
}

/// Extract every changed BLC directory not in `skip_dirs` into `<output>/decrypted/<group>`.
async fn extract_blc_directories(
    ctx: &UpdateContext<'_>,
    state: &mut UpdateState,
    output: &Path,
    skip_dirs: &[String],
) -> Result<BlcCounts, Box<dyn std::error::Error>> {
    let resolved = ctx.resolved;
    let mut counts = BlcCounts::default();
    let staging_parent = output.join("decrypted/.staging");
    let decrypted = output.join("decrypted");
    fs::create_dir_all(&staging_parent)?;
    let upstream_dirs = pipeline::blc_directories(&resolved.cd, resolved.patch_index.as_ref());
    state.blc_dirs.retain(|dir, _| upstream_dirs.contains(dir));
    let blc_dirs: Vec<&String> = upstream_dirs
        .iter()
        .filter(|dir| !skip_dirs.iter().any(|skip| skip.eq_ignore_ascii_case(dir)))
        .collect();
    let blc_total = blc_dirs.len() as u64;
    for (i, dir) in blc_dirs.iter().enumerate() {
        let dir = dir.as_str();
        emit_progress(ctx.progress, "blc", i as u64, blc_total, dir);
        let archive_catalog = resolved
            .cd
            .get(&pipeline::blc_path(dir))
            .map(|entry| update::zip_digest(entry.crc32, entry.uncompressed_size));
        // Hot-updated and patch-only directories are read through their CDN catalog, which
        // is fetched up front: it names chunks the patch index does not list. When the CDN
        // has none, the archive catalog is used and the digest says so, so a later run that
        // finds a CDN catalog re-extracts.
        let (catalog, cdn_digest) = if blc::uses_cdn_catalog(resolved, dir) {
            let fetched = match blc::fetch_cdn_catalog(ctx.client, resolved, dir).await {
                Ok(Some(bytes)) => Ok((Some(update::catalog_digest(&bytes)), bytes)),
                Ok(None) => blc::fetch_archive_catalog(ctx.client, resolved, dir)
                    .await
                    .and_then(|archive| {
                        let bytes = archive.ok_or("no catalog on the CDN or in the archive")?;
                        println!("  {dir}: no CDN catalog; using the archive catalog");
                        Ok((Some(update::CDN_CATALOG_MISSING.to_string()), bytes))
                    }),
                Err(error) => Err(error),
            };
            match fetched {
                Ok((digest, bytes)) => (Some(bytes), digest),
                Err(error) => {
                    println!("  {dir}: catalog: {error}");
                    counts.dirty += 1;
                    counts.failed += 1;
                    continue;
                }
            }
        } else {
            (None, None)
        };
        let digest = update::blc_digest(
            archive_catalog,
            cdn_digest,
            resolved.patch_index.as_ref(),
            dir,
        );
        if !update::is_dirty(&state.blc_dirs, dir, &digest, ctx.force) {
            continue;
        }
        counts.dirty += 1;
        // Each directory stages under its own hash so a crash never leaves a half-written group.
        let staging = staging_parent.join(dir);
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        let extracted = match &catalog {
            Some(catalog) => {
                blc::extract_directory_from(
                    ctx.client,
                    resolved,
                    dir,
                    catalog,
                    &staging,
                    ctx.schemas,
                )
                .await
            }
            None => {
                blc::extract_directory_with(ctx.client, resolved, dir, &staging, ctx.schemas).await
            }
        };
        match extracted {
            Ok(result) if !result.errors.is_empty() => {
                counts.add_decodes(&result);
                // Keep the previous complete copy; the directory stays dirty and is retried.
                println!(
                    "  {} ({dir}): {} errors, first: {}",
                    result.group_name,
                    result.errors.len(),
                    result.errors[0]
                );
                fs::remove_dir_all(&staging)?;
                counts.failed += 1;
            }
            Ok(result) => {
                counts.add_decodes(&result);
                install_blc_group(&decrypted, dir, &result, &staging)?;
                state.blc_dirs.insert(dir.to_string(), digest);
                update::save_state(ctx.state_path, state)?;
                counts.ok += 1;
            }
            Err(error) => {
                println!("  {dir}: {error}");
                // Drop a partial group and any scratch chunk (up to 1.5 GB) right away.
                if staging.exists() {
                    fs::remove_dir_all(&staging)?;
                }
                counts.failed += 1;
            }
        }
    }
    emit_progress(ctx.progress, "blc", blc_total, blc_total, "");
    update::save_state(ctx.state_path, state)?;
    Ok(counts)
}

/// Swap a fully staged BLC directory into `decrypted/<group>`, replacing the previous copy.
fn install_blc_group(
    decrypted: &Path,
    dir: &str,
    result: &DirExtractResult,
    staging: &Path,
) -> std::io::Result<()> {
    let mut group = result
        .output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.to_string());
    // Another directory already owns this group folder: suffix ours with the hash.
    let owner = fs::read_to_string(decrypted.join(&group).join("blc_metadata.txt"));
    if owner.is_ok_and(|meta| {
        !meta
            .to_ascii_uppercase()
            .contains(&format!("({})", dir.to_ascii_uppercase()))
    }) {
        group = format!("{group}-{dir}");
    }
    fs::create_dir_all(decrypted)?;
    let old = decrypted.join(format!(".old-{dir}"));
    if old.exists() {
        fs::remove_dir_all(&old)?;
    }
    let final_dir = decrypted.join(&group);
    if final_dir.exists() {
        fs::rename(&final_dir, &old)?;
    }
    fs::rename(&result.output, &final_dir)?;
    fs::remove_dir_all(staging)?;
    if old.exists() {
        fs::remove_dir_all(old)?;
    }
    // Earlier versions wrote a flat `decrypted/<hash>/` directory.
    let legacy = decrypted.join(dir);
    if legacy != final_dir && legacy.is_dir() {
        fs::remove_dir_all(legacy)?;
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// schema-gen — regenerate the MemoryPack schema
// ═══════════════════════════════════════════════════════════

/// Exit code of `schema-gen` when the new schema would decode fewer files than the current.
const EXIT_SCHEMA_REGRESSION: i32 = 2;
/// Exit code of `schema-gen` when the client files have a layout the IL2CPP reader does not
/// know (metadata version, type stride, registration).
const EXIT_SCHEMA_STRUCTURAL: i32 = 3;

/// The stored MemoryPack schemas beside `state_path`, newest first, then the embedded one.
fn load_schema_set(state_path: &Path) -> std::io::Result<memorypack::SchemaSet> {
    let store = memorypack_store::store_dir(state_path);
    let (set, warnings) = memorypack_store::load_set(&store)?;
    for warning in warnings {
        eprintln!("MemoryPack: {warning}");
    }
    Ok(set)
}

/// Client files given on the command line instead of downloaded.
struct LocalClient {
    dll: PathBuf,
    metadata: PathBuf,
    game_version: String,
}

struct SchemaGenOptions {
    state: PathBuf,
    output: PathBuf,
    local: Option<LocalClient>,
    json_dir: Option<PathBuf>,
    force: bool,
}

/// The client's IL2CPP files.
struct ClientFiles {
    dll: Vec<u8>,
    metadata: Vec<u8>,
    dll_crc32: u32,
}

/// Archive name of a client file: `path` itself, else the shortest name with its file name.
fn client_entry_name(cd: &ZipCentralDirectory, path: &str) -> Option<String> {
    if cd.get(path).is_some() {
        return Some(path.to_owned());
    }
    let file = path.rsplit('/').next()?;
    cd.filenames()
        .filter(|name| name.rsplit('/').next() == Some(file))
        .min_by_key(|name| name.len())
        .map(str::to_owned)
}

/// `UpdateState::memorypack_client` digest of the archive's `GameAssembly.dll`, with its name.
fn client_digest(cd: &ZipCentralDirectory) -> Result<(String, String), String> {
    let name = client_entry_name(cd, memorypack_store::CLIENT_DLL)
        .ok_or_else(|| format!("no {} in the archive", memorypack_store::CLIENT_DLL))?;
    let entry = cd.get(&name).ok_or("archive entry vanished")?;
    Ok((
        update::zip_digest(entry.crc32, entry.uncompressed_size),
        name,
    ))
}

/// Download `GameAssembly.dll` and `global-metadata.dat`, checking the binary's CRC-32.
async fn download_client(client: &Client, cd: &ZipCentralDirectory) -> Result<ClientFiles, String> {
    let (_, dll_name) = client_digest(cd)?;
    let meta_name = client_entry_name(cd, memorypack_store::CLIENT_METADATA)
        .ok_or_else(|| format!("no {} in the archive", memorypack_store::CLIENT_METADATA))?;
    let entry = cd.get(&dll_name).ok_or("archive entry vanished")?;
    println!(
        "downloading {dll_name} ({}) and {meta_name}",
        format_bytes(entry.uncompressed_size)
    );
    let (dll, metadata) = futures::join!(
        pipeline::download_chk(client, cd, &dll_name),
        pipeline::download_chk(client, cd, &meta_name)
    );
    let dll = dll.map_err(|e| format!("{dll_name}: {e}"))?;
    let metadata = metadata.map_err(|e| format!("{meta_name}: {e}"))?;
    let dll_crc32 = backend::extraction::memorypack_schema::crc32(&dll);
    if dll_crc32 != entry.crc32 || dll.len() as u64 != entry.uncompressed_size {
        return Err(format!(
            "{dll_name} arrived corrupted (crc {dll_crc32:08x}, {} bytes; expected {:08x}, {})",
            dll.len(),
            entry.crc32,
            entry.uncompressed_size
        ));
    }
    Ok(ClientFiles {
        dll,
        metadata,
        dll_crc32,
    })
}

/// JsonData groups under `<output>/decrypted`, and the `Json/` tree to learn from: `json_dir`
/// when given, else the first group's.
fn json_data_inputs(
    output: &Path,
    json_dir: Option<PathBuf>,
) -> std::io::Result<(Vec<PathBuf>, Option<PathBuf>)> {
    let groups = blc_group_dirs(&output.join("decrypted"), memorypack::JSON_DATA_GROUP)?;
    let json_dir = json_dir.or_else(|| groups.iter().map(|g| g.join("Json")).find(|j| j.is_dir()));
    Ok((groups, json_dir))
}

/// Run [`memorypack_store::sync`] (CPU and disk bound) without stalling the async runtime.
fn run_schema_sync(
    files: &ClientFiles,
    game_version: &str,
    state_path: &Path,
    json_dir: Option<&Path>,
    groups: &[PathBuf],
    force: bool,
) -> Result<memorypack_store::SyncReport, backend::extraction::memorypack_schema::GenerateError> {
    let store = memorypack_store::store_dir(state_path);
    let input = SyncInput {
        dll: &files.dll,
        metadata: &files.metadata,
        game_version,
        client_crc32: files.dll_crc32,
        store_dir: &store,
        json_dir,
        groups,
        force,
    };
    tokio::task::block_in_place(|| memorypack_store::sync(&input))
}

/// `schema-gen`: returns the process exit code.
async fn cmd_schema_gen(
    client: &Client,
    options: SchemaGenOptions,
) -> Result<i32, Box<dyn std::error::Error>> {
    let Some(_lock) = lock_state(&options.state)? else {
        return Err("another update is running".into());
    };
    let (files, game_version, digest) = match &options.local {
        Some(local) => {
            let dll = fs::read(&local.dll)?;
            let metadata = fs::read(&local.metadata)?;
            let dll_crc32 = backend::extraction::memorypack_schema::crc32(&dll);
            let files = ClientFiles {
                dll,
                metadata,
                dll_crc32,
            };
            // Local files may not be the live client, so the update state is left alone.
            (files, local.game_version.clone(), None)
        }
        None => {
            let resolved = pipeline::resolve_pipeline(client, false).await?;
            let (digest, _) = client_digest(&resolved.cd)?;
            let files = download_client(client, &resolved.cd).await?;
            (files, resolved.config.version.clone(), Some(digest))
        }
    };
    let (groups, json_dir) = json_data_inputs(&options.output, options.json_dir.clone())?;
    match &json_dir {
        Some(dir) => println!("learning and validating against {}", dir.display()),
        None => println!(
            "no JsonData tree under {}; generating without dir_roots",
            options.output.join("decrypted").display()
        ),
    }
    let report = match run_schema_sync(
        &files,
        &game_version,
        &options.state,
        json_dir.as_deref(),
        &groups,
        options.force,
    ) {
        Ok(report) => report,
        Err(error) if error.is_structural() => {
            eprintln!("schema-gen: {error}");
            eprintln!("the IL2CPP reader needs updating for this client");
            return Ok(EXIT_SCHEMA_STRUCTURAL);
        }
        Err(error) => return Err(error.into()),
    };
    drop(files);
    for line in report.lines() {
        println!("{line}");
    }
    if !report.installed() {
        return Ok(EXIT_SCHEMA_REGRESSION);
    }
    if let Some(digest) = digest {
        let mut state = update::load_state(&options.state)?;
        state.memorypack_client = digest;
        update::save_state(&options.state, &state)?;
    }
    if report.redecode.as_ref().is_some_and(|r| r.write_errors > 0) {
        return Err("could not write some JsonData files; rerun to retry".into());
    }
    Ok(0)
}

/// What the schema step of `update` did.
#[derive(Default)]
struct SchemaRun {
    /// A schema for the current client was generated, validated and is installed.
    regenerated: bool,
    /// JsonData totals after re-decoding with the new set: `(decoded, failed)`.
    decoded: Option<(u64, u64)>,
    /// Why the schema was not regenerated, when it should have been.
    error: Option<String>,
    /// Game version of the newest schema in use afterwards.
    version: Option<String>,
}

/// Regenerate the MemoryPack schema when the archive's `GameAssembly.dll` differs from the
/// one the newest schema came from. Never fails the update: a problem is printed and
/// returned in [`SchemaRun::error`], and the schemas in use stay as they are.
async fn regenerate_schema(
    ctx: &UpdateContext<'_>,
    state: &mut UpdateState,
    output: &Path,
) -> SchemaRun {
    let mut run = SchemaRun::default();
    let fail = |run: &mut SchemaRun, error: String| {
        println!("MemoryPack schema not regenerated: {error}; keeping the current schemas");
        run.error = Some(error);
    };
    let cd = &ctx.resolved.cd;
    let digest = match client_digest(cd) {
        Ok((digest, _)) => digest,
        Err(error) => {
            fail(&mut run, error);
            return run;
        }
    };
    if !ctx.force && (state.memorypack_client == digest || state.memorypack_attempted == digest) {
        return run;
    }
    println!("MemoryPack: client binary changed ({digest}); regenerating the schema");
    emit_progress(ctx.progress, "schema", 0, 0, "downloading client");
    let files = match download_client(ctx.client, cd).await {
        Ok(files) => files,
        Err(error) => {
            fail(&mut run, format!("download: {error}"));
            return run;
        }
    };
    let (groups, json_dir) = match json_data_inputs(output, None) {
        Ok(inputs) => inputs,
        Err(error) => {
            fail(&mut run, format!("finding JsonData: {error}"));
            return run;
        }
    };
    emit_progress(ctx.progress, "schema", 0, 0, "generating");
    let version = &ctx.resolved.config.version;
    // A refused or broken client is not worth another ~400 MB download on every run.
    let give_up = |run: &mut SchemaRun, state: &mut UpdateState, error: String| {
        fail(run, error);
        state.memorypack_attempted = digest.clone();
        if let Err(error) = update::save_state(ctx.state_path, state) {
            eprintln!("  saving state: {error}");
        }
    };
    match run_schema_sync(
        &files,
        version,
        ctx.state_path,
        json_dir.as_deref(),
        &groups,
        false,
    ) {
        Ok(report) => {
            for line in report.lines() {
                println!("  {line}");
            }
            if let Some(r) = &report.redecode {
                run.decoded = Some((r.decoded, r.failed));
            }
            match &report.outcome {
                SyncOutcome::Refused(reason) => give_up(&mut run, state, reason.clone()),
                SyncOutcome::Installed(_) | SyncOutcome::Unchanged(_) => {
                    run.regenerated = true;
                    run.version = report.schema_version.clone();
                    state.memorypack_client = digest.clone();
                    state.memorypack_attempted.clear();
                    if let Err(error) = update::save_state(ctx.state_path, state) {
                        run.error = Some(format!("saving state: {error}"));
                    }
                }
            }
        }
        Err(error) if error.is_structural() => {
            give_up(&mut run, state, format!("structural: {error}"))
        }
        Err(error) => give_up(&mut run, state, error.to_string()),
    }
    emit_progress(ctx.progress, "schema", 1, 1, "");
    run
}

// ═══════════════════════════════════════════════════════════
// health — check the update run log
// ═══════════════════════════════════════════════════════════

fn cmd_health(output: &Path, max_age_hours: i64) -> Result<(), Box<dyn std::error::Error>> {
    // A missing log reads as empty, which the evaluation reports as a failure.
    let text = match fs::read_to_string(output.join("update-log.jsonl")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let entries = health::parse_log(&text);
    let thresholds = health::Thresholds {
        max_age: chrono::Duration::hours(max_age_hours),
    };
    let findings = health::evaluate(&entries, Utc::now(), thresholds);
    for finding in &findings {
        println!("{}: {}", finding.level.label(), finding.message);
    }
    std::process::exit(health::worst(&findings).exit_code());
}

// ═══════════════════════════════════════════════════════════
// repair — fix textures extracted before the orientation fix
// ═══════════════════════════════════════════════════════════

/// Decrypt Lua scripts left encrypted by older runs and decode JsonData MemoryPack files
/// (re-decoding from their `.mpk` sources), then flip every PNG recorded for chunks
/// still marked `+tex1` and mark them current.
///
/// A per-chunk journal (`.index/<chk>.flipped`) records finished files so an interrupted run
/// resumes without flipping anything twice.
fn cmd_repair(output: &Path, state_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let Some(_lock) = lock_state(state_path)? else {
        println!("another update is running");
        return Ok(());
    };
    repair_lua(&output.join("decrypted"))?;
    repair_json_data(&output.join("decrypted"), &load_schema_set(state_path)?)?;
    let mut state = update::load_state(state_path)?;
    let tex_dir = output.join("textures");
    let stale: Vec<String> = state
        .chk
        .iter()
        .filter(|(_, digest)| update::mark_textures_flipped(digest).is_some())
        .map(|(name, _)| name.clone())
        .collect();
    if stale.is_empty() {
        println!("repair: no textures need flipping");
        return Ok(());
    }
    let workers = std::thread::available_parallelism().map_or(4, usize::from);
    let (mut flipped_total, mut missing_total, mut failed_total) = (0u64, 0u64, 0u64);
    for (i, name) in stale.iter().enumerate() {
        let index = texture_layout::chk_index_path(&tex_dir, name);
        let journal_path = index.with_extension("flipped");
        let done: HashSet<String> = fs::read_to_string(&journal_path)
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default();
        let pending = pending_index_paths(&index, &done)?;
        let index_missing = pending.is_none();
        let paths = pending.unwrap_or_default();
        let journal = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&journal_path)?;
        let (counts, aborted) = flip_textures(&tex_dir, &paths, journal, workers);
        let FlipCounts {
            flipped,
            missing,
            corrupt,
            failed,
        } = counts;
        println!(
            "[{}/{}] {}: flipped {flipped}, missing {missing}, removed unreadable {corrupt}, failed {failed}",
            i + 1,
            stale.len(),
            name.rsplit('/').next().unwrap_or(name)
        );
        flipped_total += flipped;
        missing_total += missing + corrupt;
        failed_total += failed;
        if aborted {
            return Err(format!("could not write {}", journal_path.display()).into());
        }
        if failed == 0 {
            let Some(old) = state.chk.get(name) else {
                continue;
            };
            // Without an index there is no list of this chunk's files to flip, so the next
            // update re-decodes all of them instead.
            let digest = if index_missing {
                update::mark_textures_stale(old)
            } else {
                update::mark_textures_flipped(old).unwrap_or_else(|| old.clone())
            };
            state.chk.insert(name.clone(), digest);
            update::save_state(state_path, &state)?;
            fs::remove_file(&journal_path)?;
        }
    }
    println!(
        "repair: flipped {flipped_total}, missing {missing_total} (extracted by the next update), failed {failed_total}"
    );
    if failed_total > 0 {
        return Err("repair had failures; rerun to retry".into());
    }
    Ok(())
}

/// Decrypt Lua scripts that an older extractor left encrypted, in every Lua group under
/// `decrypted`.
fn repair_lua(decrypted: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut totals = lua::TreeDecryptResult::default();
    for group in blc_group_dirs(decrypted, lua::LUA_GROUP)? {
        let result = lua::decrypt_tree(&group)?;
        totals.decrypted += result.decrypted;
        totals.write_errors += result.write_errors;
    }
    if totals.decrypted > 0 || totals.write_errors > 0 {
        println!(
            "repair: decrypted {} Lua scripts, failed {}",
            totals.decrypted, totals.write_errors
        );
    }
    if totals.write_errors > 0 {
        return Err("repair could not write some Lua scripts; rerun to retry".into());
    }
    Ok(())
}

/// Decode MemoryPack files in every JsonData group under `decrypted` that an older extractor
/// left binary, or re-decode them from their `.mpk` sources after a schema update.
fn repair_json_data(
    decrypted: &Path,
    schemas: &memorypack::SchemaSet,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut totals = memorypack::TreeDecodeResult::default();
    for group in blc_group_dirs(decrypted, memorypack::JSON_DATA_GROUP)? {
        let result = match memorypack::decode_tree_set(schemas, &group) {
            Ok(result) => result,
            // An unreadable string pool means the files stay as stored, as in extraction.
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                eprintln!(
                    "repair: JsonData decode skipped in {}: {error}",
                    group.display()
                );
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        totals.add(&result);
    }
    if totals != memorypack::TreeDecodeResult::default() {
        println!(
            "repair: decoded {} JsonData files ({} not decodable), failed {}",
            totals.decoded, totals.failed, totals.write_errors
        );
    }
    if totals.write_errors > 0 {
        return Err("repair could not write some JsonData files; rerun to retry".into());
    }
    Ok(())
}

/// Folders under `decrypted` whose `blc_metadata.txt` names BLC group `name`.
fn blc_group_dirs(decrypted: &Path, name: &str) -> std::io::Result<Vec<PathBuf>> {
    let Ok(groups) = fs::read_dir(decrypted) else {
        return Ok(Vec::new());
    };
    let mut dirs = Vec::new();
    for group in groups {
        let group = group?.path();
        let matches = fs::read_to_string(group.join("blc_metadata.txt"))
            .is_ok_and(|metadata| blc::metadata_group(&metadata) == Some(name));
        if matches {
            dirs.push(group);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Texture paths listed in a chunk index and not yet journaled as done, or `None` when the
/// index does not exist.
///
/// Deduplicated: two workers flipping one file would undo each other.
fn pending_index_paths(
    index: &Path,
    done: &HashSet<String>,
) -> std::io::Result<Option<Vec<String>>> {
    match fs::read_to_string(index) {
        Ok(text) => Ok(Some(
            text.lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .filter_map(|entry| entry.get("path")?.as_str().map(str::to_string))
                .filter(|path| !done.contains(path))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Per-chunk results of flipping PNGs in place.
struct FlipCounts {
    flipped: u64,
    /// Listed but never extracted; the next update decodes them fresh.
    missing: u64,
    /// Unreadable files that were removed so the next update decodes them again.
    corrupt: u64,
    failed: u64,
}

enum FlipOutcome {
    Flipped,
    Missing,
    RemovedCorrupt,
    Failed,
    /// The flip could not be journaled; the original is untouched and the run must stop.
    JournalFailed,
}

/// Flip `paths` (relative to `tex_dir`) on `workers` threads, journaling each file before its
/// flipped copy replaces it. Also returns whether a journal write failed.
fn flip_textures(
    tex_dir: &Path,
    paths: &[String],
    journal: File,
    workers: usize,
) -> (FlipCounts, bool) {
    let journal = Mutex::new(journal);
    let next = AtomicU64::new(0);
    let (flipped, missing, corrupt, failed) = (
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    );
    let abort = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while !abort.load(Ordering::Relaxed)
                    && let Some(relative) = paths.get(next.fetch_add(1, Ordering::Relaxed) as usize)
                {
                    let counter = match flip_one(tex_dir, relative, &journal) {
                        FlipOutcome::Flipped => &flipped,
                        FlipOutcome::Missing => &missing,
                        FlipOutcome::RemovedCorrupt => &corrupt,
                        FlipOutcome::Failed => &failed,
                        FlipOutcome::JournalFailed => {
                            abort.store(true, Ordering::Relaxed);
                            &failed
                        }
                    };
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    let counts = FlipCounts {
        flipped: flipped.into_inner(),
        missing: missing.into_inner(),
        corrupt: corrupt.into_inner(),
        failed: failed.into_inner(),
    };
    (counts, abort.into_inner())
}

fn flip_one(tex_dir: &Path, relative: &str, journal: &Mutex<File>) -> FlipOutcome {
    let path = tex_dir.join(relative);
    if !path.is_file() {
        return FlipOutcome::Missing;
    }
    let tmp = match texture_layout::write_flipped_png(&path) {
        Ok(tmp) => tmp,
        Err(FlipError::Corrupt(error)) => {
            // Likely truncated by a crash. Removing it lets the next update decode the
            // texture again.
            eprintln!("  removing unreadable {}: {error}", path.display());
            return match fs::remove_file(&path) {
                Ok(()) => FlipOutcome::RemovedCorrupt,
                Err(_) => FlipOutcome::Failed,
            };
        }
        Err(error) => {
            eprintln!("  FAIL flip {}: {error}", path.display());
            return FlipOutcome::Failed;
        }
    };
    // Record the flip before it lands: if the journal cannot be written the original stays
    // untouched and the run stops.
    let recorded = journal
        .lock()
        .unwrap()
        .write_all(format!("{relative}\n").as_bytes())
        .is_ok();
    if !recorded {
        let _ = fs::remove_file(&tmp);
        return FlipOutcome::JournalFailed;
    }
    match fs::rename(&tmp, &path) {
        Ok(()) => FlipOutcome::Flipped,
        Err(error) => {
            eprintln!("  FAIL replace {}: {error}", path.display());
            FlipOutcome::Failed
        }
    }
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
    print_pipeline_summary(&resolved);

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

    let assets = process_chk_file(&data)?;
    println!("{} assets from VFS pipeline\n", assets.len());

    fs::create_dir_all(output)?;

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
        fs::write(&save_path, &asset.data)?;
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
    no_voice: bool,
    output: &str,
    state_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ChaCha20 VFS Decryption ===\n");
    let schemas = load_schema_set(state_path)?;

    let resolved = pipeline::resolve_pipeline(client, true).await?;
    print_pipeline_summary(&resolved);

    let output_path = Path::new(output);
    fs::create_dir_all(output_path)?;

    let available = pipeline::blc_directories(&resolved.cd, resolved.patch_index.as_ref());
    let dir_filter = dir_filter.map(|d| pipeline::resolve_dir_name(&d));
    let skipped = skip_dir_hashes(&[], no_voice);
    let dirs: Vec<&str> = available
        .iter()
        .map(String::as_str)
        .filter(|h| dir_filter.as_deref().is_none_or(|d| d == *h))
        .filter(|h| !skipped.iter().any(|s| s == h))
        .collect();

    if dirs.is_empty() {
        println!("No matching directories found.");
        if let Some(d) = &dir_filter {
            println!("Available: {:?}", available);
            println!("You specified: {}", d);
        }
        return Ok(());
    }

    let mut total_extracted = 0usize;
    let mut total_sparkbuffer = 0usize;
    let mut total_lua = 0usize;
    let mut total_memorypack = 0usize;
    let mut total_memorypack_failed = 0usize;

    for dir_hash in &dirs {
        println!("{}", "=".repeat(60));
        println!("=== {} ===", dir_hash);

        match blc::extract_directory_with(client, &resolved, dir_hash, output_path, &schemas).await
        {
            Ok(result) => {
                println!(
                    "  {} ({}): {}/{} files extracted, {} SparkBuffer→JSON, {} Lua decrypted, {} MemoryPack→JSON ({} not decodable)",
                    result.group_name,
                    result.dir_hash,
                    result.extracted,
                    result.total_files,
                    result.sparkbuffer_parsed,
                    result.lua_decrypted,
                    result.memorypack_decoded,
                    result.memorypack_failed,
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
                total_lua += result.lua_decrypted;
                total_memorypack += result.memorypack_decoded;
                total_memorypack_failed += result.memorypack_failed;
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
    println!("  Lua decrypted: {}", total_lua);
    println!(
        "  MemoryPack→JSON: {} ({} not decodable)",
        total_memorypack, total_memorypack_failed
    );
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
    print_pipeline_summary(&resolved);

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

/// Names and strings found in one scanned `.chk`.
struct ChkManifest {
    dir: String,
    vfs_names: Vec<String>,
    bundle_paths: Vec<String>,
    embedded_strings: Vec<String>,
}

/// Append each printable ASCII run of at least 8 bytes in `data` not already in `seen`.
fn collect_embedded_strings(data: &[u8], seen: &mut HashSet<String>, out: &mut Vec<String>) {
    for run in data.split(|byte| !(0x20..0x7F).contains(byte)) {
        if run.len() >= 8
            && let Ok(s) = std::str::from_utf8(run)
            && seen.insert(s.to_string())
        {
            out.push(s.to_string());
        }
    }
}

async fn cmd_scan(client: &Client, scan_all: bool) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Asset Name Scanner ===\n");

    let resolved = pipeline::resolve_pipeline(client, true).await?;
    print_pipeline_summary(&resolved);

    if let Some(patch) = &resolved.patch_index {
        println!("Patch version: {}", patch.version);
        println!("Chunk files: {}\n", patch.files.len());
    }

    let mut manifests: Vec<ChkManifest> = Vec::new();

    for (dir, desc) in VFS_DIRS {
        println!("--- {} ({}) ---", dir, desc);
        let candidates = chk_candidates(&resolved.cd, dir, scan_all);

        for (i, (name, size)) in candidates.iter().enumerate() {
            println!(
                "  [{}/{}] {} ({})",
                i + 1,
                candidates.len(),
                name,
                format_bytes(*size)
            );
            let Some(data) = download_or_report(client, &resolved.cd, name).await else {
                continue;
            };

            if !vfs_decrypt::is_valid_header(&data) {
                println!("    Not a valid VFS container, skipping");
                continue;
            }

            let assets = match process_chk_file(&data) {
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
                collect_embedded_strings(&asset.data, &mut seen_strings, &mut embedded_strings);
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
                vfs_names,
                bundle_paths,
                embedded_strings,
            });
        }
        println!();
    }

    println!("=== Keyword Search Results ===\n");
    for keyword in SEARCH_KEYWORDS {
        let kw_lower = keyword.to_lowercase();
        let mut matches: Vec<(&str, &str, &str)> = Vec::new();
        for manifest in &manifests {
            let sources = [
                ("vfs", &manifest.vfs_names),
                ("bundle", &manifest.bundle_paths),
                ("embedded", &manifest.embedded_strings),
            ];
            for (source, values) in sources {
                for value in values {
                    if value.to_lowercase().contains(&kw_lower) {
                        matches.push((&manifest.dir, source, value));
                    }
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
    print_pipeline_summary(&resolved);

    let mut all_chks: Vec<ChkContents> = Vec::new();

    for (dir, desc) in VFS_DIRS {
        println!("--- {} ({}) ---", dir, desc);
        let candidates = chk_candidates(&resolved.cd, dir, scan_all);

        for (name, size) in &candidates {
            println!("  {} ({})", name, format_bytes(*size));
            let Some(data) = download_or_report(client, &resolved.cd, name).await else {
                continue;
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

    println!("╔══════════════════════════════════════════╗");
    println!("║  Class ID Census                         ║");
    println!("╚══════════════════════════════════════════╝\n");

    let mut class_counts: BTreeMap<i32, u64> = BTreeMap::new();
    let mut class_total_bytes: BTreeMap<i32, u64> = BTreeMap::new();
    let mut total_objects = 0u64;

    for chk in &all_chks {
        for sf_bytes in &chk.sf_data {
            let Ok(sf) = SerializedFile::parse(sf_bytes.clone()) else {
                continue;
            };
            for obj in &sf.objects {
                *class_counts.entry(obj.class_id).or_insert(0) += 1;
                *class_total_bytes.entry(obj.class_id).or_insert(0) += obj.byte_size as u64;
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
