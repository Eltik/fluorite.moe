//! Rigged-model exporter for Arknights: Endfield.
//!
//! Subcommands:
//!   chunks   — list the bundle `.chk` files of the game archive
//!   inspect  — summarize the Unity objects and container paths of chunks
//!   scan     — download chunks once and keep what rig export needs in a small store
//!   export   — write one self-contained GLB per entity prefab matching a filter
//!   clips    — write one animation-only GLB per clip of every exported model
//!   charinfo — write each operator's character-screen data (menu state machine, weapon
//!              mounts, deco props, lights and camera)
use backend::extraction::pipeline::{self, VFS_DIRS};
use backend::extraction::rig::{self, ChunkFiles};
use backend::extraction::zip_range::ZipCentralDirectory;
use clap::{Parser, Subcommand};
use reqwest::Client;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "endfield-models",
    about = "Export rigged Endfield models as GLB"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List bundle .chk files with their sizes
    Chunks,
    /// Summarize objects and container paths of chunks
    Inspect {
        /// Chunk name (hash, with or without .chk); repeatable
        #[arg(long = "chk")]
        chks: Vec<String>,
        /// Read pre-extracted SerializedFiles from this directory instead of downloading
        #[arg(long)]
        source_dir: Option<PathBuf>,
        /// Only print container paths containing this substring
        #[arg(long, default_value = "")]
        filter: String,
        /// Keep downloaded .chk files here and reuse them on later runs
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    /// Print objects of a stored SerializedFile as JSON (debugging)
    Dump {
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        cab: String,
        /// Only this Unity class ID
        #[arg(long)]
        class: Option<i32>,
        /// Only this path ID
        #[arg(long)]
        path_id: Option<i64>,
        #[arg(long, default_value_t = 3)]
        limit: usize,
        /// Truncate arrays longer than this
        #[arg(long, default_value_t = 16)]
        max_array: usize,
    },
    /// Summarize AnimationClip / Animator data in a store (for planning clip export)
    AnimSurvey {
        #[arg(long)]
        store: PathBuf,
        /// Read clips from at most this many SerializedFiles
        #[arg(long, default_value_t = 400)]
        max_files: usize,
    },
    /// Check stored files against the index; remove damaged ones and re-queue their chunks
    Repair {
        #[arg(long)]
        store: PathBuf,
    },
    /// Delete stored files the store's relevance rules no longer keep
    Prune {
        #[arg(long)]
        store: PathBuf,
    },
    /// Download chunks and keep their SerializedFiles + mesh streams in a store
    Scan {
        /// Store directory (sf/, res/, index/)
        #[arg(long)]
        store: PathBuf,
        /// Chunk(s) to scan; default: every bundle chunk not yet scanned
        #[arg(long = "chk")]
        chks: Vec<String>,
        /// Concurrent downloads
        #[arg(long, default_value_t = 3)]
        jobs: usize,
        /// Reuse .chk files from this directory when present
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    /// Export entity prefabs matching a filter as GLB
    Export {
        /// Substring of the prefab container path (e.g. "entity/actor", "postmodels/characters")
        #[arg(long)]
        filter: String,
        /// Only entities whose id contains one of these (repeatable)
        #[arg(long = "id")]
        ids: Vec<String>,
        /// Chunk(s) to scan into the store first (downloaded once)
        #[arg(long = "chk")]
        chks: Vec<String>,
        /// Pre-extracted files instead of the store: a store-layout dir (sf/, res/) or a flat
        /// dir of SerializedFiles (`CAB-*` or `*.bin`) and `*.resS`
        #[arg(long)]
        source_dir: Option<PathBuf>,
        /// Output directory for <kind>/<id>.glb and index.json
        #[arg(long)]
        output: PathBuf,
        /// Store of scanned chunks (default: <output>/.store)
        #[arg(long)]
        store: Option<PathBuf>,
        /// Extraction output root holding textures/ (texture PNGs and .index)
        #[arg(long, default_value = "output")]
        data_root: PathBuf,
        /// Reuse .chk files from this directory when present
        #[arg(long)]
        cache_dir: Option<PathBuf>,
        /// Concurrent downloads when scanning
        #[arg(long, default_value_t = 3)]
        jobs: usize,
        /// Embed every referenced texture, not only base color and normal maps
        #[arg(long)]
        all_textures: bool,
        /// Embedded image encoding: auto (JPEG q90 when opaque, else PNG), png, webp (lossless)
        #[arg(long, default_value = "auto")]
        images: rig::material::ImageEncoding,
        /// Downscale textures larger than this many pixels
        #[arg(long)]
        max_texture_size: Option<u32>,
        /// Keep every LOD instead of only the best per part
        #[arg(long)]
        all_lods: bool,
        /// Don't scan chunks that the texture index associates with the filter
        #[arg(long)]
        no_auto_scan: bool,
        /// Embed AnimationClips whose file name contains a keyword. Comma-separated keywords are
        /// tried in order and the first that matches any clip wins, e.g.
        /// `uiteam_idle,dialog_single_idle,idle`. Clips are found by container path:
        /// `.../<code>/animations/**/*<keyword>*.fbx`, where <code> is the entity id's last
        /// segment (chr_0005_chen -> chen). Needs a build with `--features acl` (Endfield clips
        /// are ACL-compressed).
        #[arg(long = "anims")]
        anims: Vec<String>,
        /// At most this many clips per entity with --anims
        #[arg(long, default_value_t = 4)]
        max_anims: usize,
    },
    /// Write every clip of each exported model (operators, NPCs, enemies) as an
    /// animation-only GLB: <models>/clips/<kind>/<id>/<clip>.glb plus index.json, bound by
    /// node name to <models>/<kind>/<id>.glb. Needs `--features acl`.
    Clips {
        /// Store of scanned chunks
        #[arg(long)]
        store: PathBuf,
        /// Models directory holding index.json and <kind>/<id>.glb
        #[arg(long, default_value = "output/models")]
        models: PathBuf,
        /// Only these kinds (operator, npc, enemy); repeatable
        #[arg(long = "kind")]
        kinds: Vec<String>,
        /// Only entities whose id contains one of these (repeatable)
        #[arg(long = "id")]
        ids: Vec<String>,
        /// Worker threads (default: available cores)
        #[arg(long)]
        jobs: Option<usize>,
        /// Leave entities that already have a clips index.json
        #[arg(long)]
        skip_existing: bool,
        /// Don't add root-motion channels to the clip root
        #[arg(long)]
        no_root_motion: bool,
        /// Key reduction: largest rotation error in radians
        #[arg(long, default_value_t = rig::clips::TOLERANCE.rotation)]
        rotation_tolerance: f32,
        /// Key reduction: largest translation error in metres
        #[arg(long, default_value_t = rig::clips::TOLERANCE.position)]
        position_tolerance: f32,
    },
    /// Write the character-screen data of every exported operator: the UI Animator state
    /// machine (<models>/clips/<kind>/<id>/menu.json), default weapon mounting
    /// (<models>/charinfo/weapons/<id>.json), deco props (<models>/charinfo/deco/<id>/) and
    /// menu lights with the default camera (<models>/lights/<id>.json). Run after `clips`.
    /// Needs `--features acl` (weapon visibility is read from clip curves).
    Charinfo {
        /// Store of scanned chunks
        #[arg(long)]
        store: PathBuf,
        /// Models directory holding index.json, <kind>/<id>.glb and clips/
        #[arg(long, default_value = "output/models")]
        models: PathBuf,
        /// Extraction output root holding decrypted/Table, gamedata/ and textures/
        #[arg(long, default_value = "output")]
        data_root: PathBuf,
        /// Only operators whose id contains one of these (repeatable)
        #[arg(long = "id")]
        ids: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let client = Client::new();
    match cli.command {
        Commands::Chunks => cmd_chunks(&client).await,
        Commands::Inspect {
            chks,
            source_dir,
            filter,
            cache_dir,
        } => {
            let mut source = ChunkSource::new(&client, cache_dir).await?;
            let sets = source.load_all(&chks, source_dir.as_deref()).await?;
            for (name, files) in sets {
                println!("== {name}");
                print!("{}", rig::inspect::summarize(&files, &filter));
            }
            Ok(())
        }
        Commands::Repair { store } => {
            let report = rig::store::repair(&store)?;
            println!(
                "checked {} stored SerializedFiles; removed {} damaged; re-queued chunks: {:?}",
                report.checked,
                report.removed.len(),
                report.requeued
            );
            Ok(())
        }
        Commands::Prune { store } => {
            let freed = rig::store::prune(&store)?;
            println!("freed {:.2} GB", freed as f64 / 1e9);
            Ok(())
        }
        Commands::AnimSurvey { store, max_files } => {
            let records = rig::store::read_index(&store)?;
            let survey = rig::inspect::animation_survey(&store, &records, max_files);
            println!("{}", serde_json::to_string_pretty(&survey)?);
            Ok(())
        }
        Commands::Dump {
            store,
            cab,
            class,
            path_id,
            limit,
            max_array,
        } => {
            let mut source = rig::store::CabSource::new(Some(store), ChunkFiles::default());
            let i = source.get(&cab).ok_or("cab not in store")?;
            let file = &source.files.files[i];
            println!(
                "externals: {:?}",
                file.sf
                    .externals
                    .iter()
                    .map(|e| e.cab_name())
                    .collect::<Vec<_>>()
            );
            let mut n = 0;
            for obj in &file.sf.objects {
                if class.is_some_and(|c| c != obj.class_id)
                    || path_id.is_some_and(|p| p != obj.path_id)
                {
                    continue;
                }
                n += 1;
                if n > limit {
                    break;
                }
                let mut value = file
                    .sf
                    .read_object(obj)
                    .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
                truncate_arrays(&mut value, max_array);
                println!(
                    "--- class {} path_id {}\n{}",
                    obj.class_id,
                    obj.path_id,
                    serde_json::to_string_pretty(&value)?
                );
            }
            Ok(())
        }
        Commands::Scan {
            store,
            chks,
            jobs,
            cache_dir,
        } => {
            scan_chunks(
                &client,
                &store,
                &chks,
                chks.is_empty(),
                jobs,
                cache_dir.as_deref(),
            )
            .await
        }
        Commands::Clips {
            store,
            models,
            kinds,
            ids,
            jobs,
            skip_existing,
            no_root_motion,
            rotation_tolerance,
            position_tolerance,
        } => {
            if !rig::acl::available() {
                return Err("clips needs a build with `--features acl`".into());
            }
            cmd_clips(&ClipsArgs {
                store,
                models,
                kinds,
                ids,
                jobs: jobs.unwrap_or_else(|| {
                    std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get)
                }),
                skip_existing,
                root_motion: !no_root_motion,
                tolerance: rig::clips::Tolerance {
                    rotation: rotation_tolerance,
                    position: position_tolerance,
                    ..rig::clips::TOLERANCE
                },
            })
        }
        Commands::Charinfo {
            store,
            models,
            data_root,
            ids,
        } => {
            if !rig::acl::available() {
                return Err("charinfo needs a build with `--features acl`".into());
            }
            cmd_charinfo(&store, &models, &data_root, &ids)
        }
        Commands::Export {
            filter,
            ids,
            chks,
            source_dir,
            output,
            store,
            data_root,
            cache_dir,
            jobs,
            all_textures,
            images,
            max_texture_size,
            all_lods,
            no_auto_scan,
            anims,
            max_anims,
        } => {
            if !anims.is_empty() && !rig::acl::available() {
                return Err("--anims needs a build with `--features acl`".into());
            }
            let store = store.unwrap_or_else(|| output.join(".store"));
            let args = ExportArgs {
                filter,
                ids,
                anims,
                max_anims,
                output,
                data_root,
                opts: rig::ExportOptions {
                    material: rig::material::MaterialOptions {
                        library: None,
                        all_textures,
                        encoding: images,
                        max_texture_size,
                    },
                    all_lods,
                },
            };
            cmd_export(
                &client,
                args,
                &store,
                &chks,
                source_dir.as_deref(),
                cache_dir.as_deref(),
                jobs,
                no_auto_scan,
            )
            .await
        }
    }
}

/// Downloads chunks (once per run), optionally through an on-disk cache.
struct ChunkSource<'a> {
    client: &'a Client,
    cd: Option<std::sync::Arc<ZipCentralDirectory>>,
    cache_dir: Option<PathBuf>,
}

impl<'a> ChunkSource<'a> {
    async fn new(
        client: &'a Client,
        cache_dir: Option<PathBuf>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            client,
            cd: None,
            cache_dir,
        })
    }

    async fn cd(
        &mut self,
    ) -> Result<std::sync::Arc<ZipCentralDirectory>, Box<dyn std::error::Error>> {
        if self.cd.is_none() {
            let resolved = pipeline::resolve_pipeline(self.client, false).await?;
            self.cd = Some(resolved.cd);
        }
        Ok(self.cd.clone().unwrap())
    }

    /// Raw bytes of a chunk by bare hash name.
    async fn fetch(&mut self, hash: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let file_name = format!("{hash}.chk");
        if let Some(dir) = &self.cache_dir {
            let cached = dir.join(&file_name);
            if let Ok(data) = std::fs::read(&cached) {
                return Ok(data);
            }
        }
        let cd = self.cd().await?;
        let full = cd
            .filenames()
            .find(|f| f.ends_with(&format!("/{file_name}")))
            .map(str::to_string)
            .ok_or_else(|| format!("chunk {file_name} not found in archive"))?;
        let started = Instant::now();
        let data = pipeline::download_chk(self.client, &cd, &full).await?;
        eprintln!(
            "  downloaded {file_name} ({:.1} MB) in {:.1}s",
            data.len() as f64 / 1e6,
            started.elapsed().as_secs_f64()
        );
        if let Some(dir) = &self.cache_dir {
            std::fs::create_dir_all(dir)?;
            let tmp = dir.join(format!("{file_name}.tmp"));
            std::fs::write(&tmp, &data)?;
            std::fs::rename(&tmp, dir.join(&file_name))?;
        }
        Ok(data)
    }

    async fn load(&mut self, hash: &str) -> Result<ChunkFiles, Box<dyn std::error::Error>> {
        let data = self.fetch(hash).await?;
        Ok(rig::load_chunk(&data)?)
    }

    async fn load_all(
        &mut self,
        chks: &[String],
        source_dir: Option<&Path>,
    ) -> Result<Vec<(String, ChunkFiles)>, Box<dyn std::error::Error>> {
        let mut out = Vec::new();
        if let Some(dir) = source_dir {
            out.push((dir.display().to_string(), rig::load_source_dir(dir)?));
        }
        for chk in chks {
            let hash = chunk_hash(chk);
            out.push((hash.clone(), self.load(&hash).await?));
        }
        Ok(out)
    }
}

/// Bare chunk hash from a name, file name or archive path.
fn chunk_hash(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    base.strip_suffix(".chk").unwrap_or(base).to_string()
}

async fn cmd_chunks(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = pipeline::resolve_pipeline(client, false).await?;
    let mut totals: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (dir, label) in VFS_DIRS {
        for (name, size) in pipeline::list_chk_files(&resolved.cd, dir) {
            if !seen.insert(name.clone()) {
                continue;
            }
            println!(
                "{label}\t{}\t{:.1} MB",
                chunk_hash(&name),
                size as f64 / 1e6
            );
            let t = totals.entry(label).or_default();
            t.0 += 1;
            t.1 += size;
        }
    }
    for (label, (count, size)) in totals {
        println!("# {label}: {count} chunks, {:.2} GB", size as f64 / 1e9);
    }
    Ok(())
}

/// Archive paths of every bundle chunk, deduplicated, smallest first.
fn bundle_chunks(cd: &ZipCentralDirectory) -> Vec<(String, u64)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (dir, _) in VFS_DIRS {
        for (name, size) in pipeline::list_chk_files(cd, dir) {
            if seen.insert(chunk_hash(&name)) {
                out.push((name, size));
            }
        }
    }
    out.sort_by_key(|(_, size)| *size);
    out
}

/// Scan chunks into the store: `chks`, or every bundle chunk when `all`. Scanned chunks are
/// skipped, so this is cheap to call again.
async fn scan_chunks(
    client: &Client,
    store: &Path,
    chks: &[String],
    all: bool,
    jobs: usize,
    cache_dir: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let hashes: BTreeSet<String> = chks.iter().map(|c| chunk_hash(c)).collect();
    if !all && hashes.iter().all(|h| rig::store::is_scanned(store, h)) {
        return Ok(());
    }
    use futures::stream::{self, StreamExt};
    let started = Instant::now();
    let resolved = pipeline::resolve_pipeline(client, false).await?;
    let cd = resolved.cd.clone();
    let all_chunks = bundle_chunks(&cd);
    let wanted: Vec<(String, u64)> = if all {
        all_chunks
    } else {
        all_chunks
            .into_iter()
            .filter(|(name, _)| hashes.contains(&chunk_hash(name)))
            .collect()
    };
    let todo: Vec<(String, u64)> = wanted
        .into_iter()
        .filter(|(name, _)| !rig::store::is_scanned(store, &chunk_hash(name)))
        .collect();
    let total_bytes: u64 = todo.iter().map(|(_, s)| s).sum();
    eprintln!(
        "scan: {} chunk(s), {:.2} GB to download, {} job(s)",
        todo.len(),
        total_bytes as f64 / 1e9,
        jobs
    );
    let count = todo.len();
    let mut downloads = stream::iter(todo.into_iter().map(|(name, size)| {
        let cd = cd.clone();
        let client = client.clone();
        let cache = cache_dir.map(Path::to_path_buf);
        let store = store.to_path_buf();
        async move {
            let hash = chunk_hash(&name);
            // Held from download to index write; another scan holding it owns the chunk.
            let lock = match rig::store::try_lock_chunk(&store, &hash) {
                Ok(Some(lock)) if !rig::store::is_scanned(&store, &hash) => lock,
                Ok(Some(_)) => return (hash, size, Ok(None)),
                Ok(None) => {
                    return (
                        hash,
                        size,
                        Err("being scanned by another process".to_string()),
                    );
                }
                Err(e) => return (hash, size, Err(format!("chunk lock: {e}"))),
            };
            if let Some(dir) = &cache
                && let Ok(data) = std::fs::read(dir.join(format!("{hash}.chk")))
            {
                return (hash, size, Ok(Some((data, lock))));
            }
            let mut last_error = String::new();
            for attempt in 1..=3 {
                let t0 = Instant::now();
                match pipeline::download_chk(&client, &cd, &name).await {
                    Ok(data) => {
                        eprintln!(
                            "  {hash}: {:.0} MB downloaded at {:.1} MB/s",
                            data.len() as f64 / 1e6,
                            data.len() as f64 / 1e6 / t0.elapsed().as_secs_f64().max(0.001)
                        );
                        return (hash, size, Ok(Some((data, lock))));
                    }
                    Err(e) => {
                        last_error = e.to_string();
                        eprintln!("  {hash}: download attempt {attempt} failed: {last_error}");
                    }
                }
            }
            (hash, size, Err(last_error))
        }
    }))
    .buffer_unordered(jobs.max(1));
    let mut done = 0usize;
    let mut failed = Vec::new();
    while let Some((hash, size, data)) = downloads.next().await {
        done += 1;
        let (data, lock) = match data {
            Ok(Some(fetched)) => fetched,
            // Scanned meanwhile by another process.
            Ok(None) => continue,
            Err(e) => {
                failed.push(format!("{hash}: {e}"));
                continue;
            }
        };
        let store = store.to_path_buf();
        let hash2 = hash.clone();
        let result = tokio::task::spawn_blocking(move || {
            let files = rig::load_chunk(&data)?;
            drop(data);
            let stats = rig::store::store_chunk(&store, &hash2, &files).map_err(|e| e.to_string());
            drop(lock);
            stats
        })
        .await?;
        match result {
            Ok(stats) => eprintln!(
                "  [{done}/{count}] {hash} ({:.0} MB): {} files, sf {:.1} MB, res {:.1} MB, {:.0}s elapsed",
                size as f64 / 1e6,
                stats.files,
                stats.sf_bytes as f64 / 1e6,
                stats.res_bytes as f64 / 1e6,
                started.elapsed().as_secs_f64()
            ),
            Err(e) => failed.push(format!("{hash}: {e}")),
        }
    }
    eprintln!(
        "scan finished in {:.0}s, {} failure(s)",
        started.elapsed().as_secs_f64(),
        failed.len()
    );
    for f in &failed {
        eprintln!("  FAILED {f}");
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} chunk(s) failed", failed.len()).into())
    }
}

fn truncate_arrays(value: &mut serde_json::Value, max: usize) {
    match value {
        serde_json::Value::Array(items) => {
            let len = items.len();
            items.truncate(max);
            for item in items.iter_mut() {
                truncate_arrays(item, max);
            }
            if len > max {
                items.push(serde_json::json!(format!("... {len} items")));
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                truncate_arrays(v, max);
            }
        }
        serde_json::Value::String(s) if s.len() > 200 => {
            *s = format!("{}... ({} chars)", &s[..80], s.len());
        }
        _ => {}
    }
}

struct ExportArgs {
    filter: String,
    ids: Vec<String>,
    anims: Vec<String>,
    max_anims: usize,
    output: PathBuf,
    data_root: PathBuf,
    opts: rig::ExportOptions,
}

/// Chunks whose texture index lists a dependency of a prefab matching `filter`.
fn chunks_from_texture_index(textures: &Path, filter: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(textures.join(".index")) else {
        return out;
    };
    let filter = filter.to_ascii_lowercase();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Some(hash) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let hit = text.lines().any(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| v.get("container")?.as_str().map(str::to_ascii_lowercase))
                .is_some_and(|c| c.ends_with(".prefab") && c.contains(&filter))
        });
        if hit {
            out.insert(hash);
        }
    }
    out
}

#[derive(serde::Serialize)]
struct IndexEntry {
    id: String,
    kind: String,
    container: String,
    glb: Option<String>,
    size: u64,
    chunks: Vec<String>,
    source_cab: Option<String>,
    counts: Option<rig::ExportStats>,
    warnings: Vec<String>,
    error: Option<String>,
}

#[allow(clippy::too_many_arguments)]
async fn cmd_export(
    client: &Client,
    mut args: ExportArgs,
    store: &Path,
    chks: &[String],
    source_dir: Option<&Path>,
    cache_dir: Option<&Path>,
    jobs: usize,
    no_auto_scan: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let textures = args.data_root.join("textures");
    let mut source = match source_dir {
        Some(dir) if dir.join("sf").is_dir() => {
            rig::store::CabSource::new(Some(dir.to_path_buf()), ChunkFiles::default())
        }
        Some(dir) => {
            let mut s = rig::store::CabSource::new(None, rig::load_source_dir(dir)?);
            s.load_all_memory();
            s
        }
        None => {
            let mut wanted: Vec<String> = chks.to_vec();
            if !no_auto_scan {
                wanted.extend(chunks_from_texture_index(&textures, &args.filter));
            }
            if !wanted.is_empty() {
                scan_chunks(client, store, &wanted, false, jobs, cache_dir).await?;
            }
            rig::store::CabSource::new(Some(store.to_path_buf()), ChunkFiles::default())
        }
    };
    let records: Vec<rig::store::CabRecord> = match source_dir {
        Some(dir) if dir.join("sf").is_dir() => rig::store::read_index(dir)?,
        Some(_) => source
            .files
            .files
            .iter()
            .map(|f| rig::store::CabRecord {
                cab: f.cab.clone(),
                chunk: "source-dir".into(),
                sf_size: f.sf.data.len(),
                resources: Vec::new(),
                resource_sizes: BTreeMap::new(),
                classes: f.sf.objects.iter().fold(BTreeMap::new(), |mut m, o| {
                    *m.entry(o.class_id).or_default() += 1;
                    m
                }),
                containers: rig::containers(&f.sf)
                    .into_iter()
                    .map(|c| (c.path, c.file_id, c.path_id))
                    .collect(),
                externals: f
                    .sf
                    .externals
                    .iter()
                    .map(|e| e.cab_name().to_string())
                    .collect(),
            })
            .collect(),
        None => rig::store::read_index(store)?,
    };
    let scanned: BTreeSet<&str> = records.iter().map(|r| r.chunk.as_str()).collect();
    eprintln!(
        "store: {} chunk(s), {} SerializedFile(s)",
        scanned.len(),
        records.len()
    );
    let mut candidates = rig::entity::candidates(&records, &args.filter);
    candidates.extend(rig::entity::avatar_candidates(&records, &args.filter));
    // Pruned bundles (effects, cutscenes…) are indexed but not stored: not exportable.
    let stored_root = match source_dir {
        Some(dir) if dir.join("sf").is_dir() => Some(dir.to_path_buf()),
        Some(_) => None,
        None => Some(store.to_path_buf()),
    };
    if let Some(root) = &stored_root {
        for c in &mut candidates {
            c.sources
                .retain(|(cab, _)| root.join("sf").join(cab).is_file());
        }
        candidates.retain(|c| !c.sources.is_empty());
    }
    if !args.ids.is_empty() {
        candidates.retain(|c| args.ids.iter().any(|id| c.id.contains(id.as_str())));
    }
    eprintln!("{} prefab(s) match {:?}", candidates.len(), args.filter);
    let clip_index = animation_containers(&records, stored_root.as_deref());
    // Part materials of renderer-less (Avatar-only) imports, found by name.
    let material_assets = rig::material::MaterialAssets::from_records(&records, &|cab: &str| {
        stored_root
            .as_ref()
            .is_none_or(|r| r.join("sf").join(cab).is_file())
    });
    let library = std::sync::Arc::new(rig::material::TextureLibrary::load(&textures));
    eprintln!(
        "texture library: {} named textures under {}",
        library.by_name.len(),
        textures.display()
    );
    args.opts.material.library = Some(library);
    std::fs::create_dir_all(&args.output)?;
    let mut index = Vec::new();
    let mut total_bytes = 0u64;
    // Output paths taken in this run, so two prefabs never overwrite one GLB.
    let mut claimed: HashMap<String, String> = HashMap::new();
    for (n, cand) in candidates.iter().enumerate() {
        let t0 = Instant::now();
        let mut last_error = None;
        let mut result = None;
        if cand.avatar {
            let clips = select_clips(&clip_index, &cand.id, &args.anims, args.max_anims);
            match export_avatar_candidate(&mut source, cand, &args, clips, &material_assets) {
                Ok(r) => result = Some(r),
                Err(e) => last_error = Some(e),
            }
        }
        for (cab, path_id) in cand.sources.iter().filter(|_| !cand.avatar) {
            let Some(file) = source.get(cab) else {
                last_error = Some(format!("{cab} not loadable"));
                continue;
            };
            let extras = serde_json::json!({
                "source": {
                    "container": cand.container,
                    "cab": cab,
                    "pathId": path_id.to_string(),
                    "chunks": cand.chunks,
                },
                "entity": {"id": cand.id, "kind": cand.kind},
            });
            let mut exporter = rig::Exporter::new(&mut source, args.opts.clone());
            exporter.clip_sources =
                select_clips(&clip_index, &cand.id, &args.anims, args.max_anims);
            match export_isolated(
                exporter,
                rig::ObjRef {
                    file,
                    path_id: *path_id,
                },
                extras,
            ) {
                Ok(r) => {
                    result = Some((cab.clone(), r));
                    break;
                }
                // Keep the most informative failure: a missing Transform only means this
                // container entry is not the prefab's root GameObject.
                Err(e) => {
                    if last_error.is_none() || !e.contains("no Transform") {
                        last_error = Some(e);
                    }
                }
            }
        }
        let mut entry = IndexEntry {
            id: cand.id.clone(),
            kind: cand.kind.to_string(),
            container: cand.container.clone(),
            glb: None,
            size: 0,
            chunks: cand.chunks.clone(),
            source_cab: None,
            counts: None,
            warnings: Vec::new(),
            error: None,
        };
        match result {
            Some((cab, r)) => {
                let rel =
                    rig::entity::claim_output(&mut claimed, cand.kind, &cand.id, &cand.container);
                let path = args.output.join(&rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let tmp = path.with_extension("glb.tmp");
                std::fs::write(&tmp, &r.glb)?;
                std::fs::rename(&tmp, &path)?;
                total_bytes += r.glb.len() as u64;
                eprintln!(
                    "[{}/{}] {} -> {} ({:.1} MB, {} meshes, {} materials, {} textures, {} joints, {} warnings, {:.1}s)",
                    n + 1,
                    candidates.len(),
                    cand.id,
                    rel,
                    r.glb.len() as f64 / 1e6,
                    r.stats.meshes,
                    r.stats.materials,
                    r.stats.textures,
                    r.stats.joints,
                    r.warnings.len(),
                    t0.elapsed().as_secs_f64()
                );
                entry.glb = Some(rel);
                entry.size = r.glb.len() as u64;
                entry.source_cab = Some(cab);
                entry.counts = Some(r.stats);
                entry.warnings = r.warnings;
            }
            None => {
                eprintln!(
                    "[{}/{}] {} FAILED: {}",
                    n + 1,
                    candidates.len(),
                    cand.id,
                    last_error.clone().unwrap_or_default()
                );
                entry.error = last_error;
            }
        }
        index.push(entry);
        if (n + 1) % 8 == 0 {
            source.clear();
        }
    }
    // Per-character menu lights of exported operators: <output>/lights/<id>.json.
    let stored = |cab: &str| {
        stored_root
            .as_ref()
            .is_none_or(|r| r.join("sf").join(cab).is_file())
    };
    let lights_dir = args.output.join("lights");
    for e in index
        .iter()
        .filter(|e| e.glb.is_some() && e.kind == "operator")
    {
        if let Some(lights) = rig::lights::character_lights(&mut source, &records, &e.id, &stored) {
            std::fs::create_dir_all(&lights_dir)?;
            let path = lights_dir.join(format!("{}.json", e.id));
            rig::store::write_atomic(&path, &serde_json::to_vec_pretty(&lights)?)?;
        }
    }
    let exported = index.iter().filter(|e| e.glb.is_some()).count();
    // Keep entries of earlier runs (other filters) that this run did not touch, under a lock
    // so concurrent exports into one directory don't lose each other's entries.
    let index_path = args.output.join("index.json");
    let _index_lock = rig::store::FileLock::acquire(
        &args.output.join(".index.lock"),
        std::time::Duration::from_secs(600),
    )?;
    let old = std::fs::read(&index_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let fresh: Vec<serde_json::Value> = index
        .iter()
        .map(|e| serde_json::to_value(e).unwrap_or_default())
        .collect();
    let entries = rig::entity::merge_index(old.as_ref(), fresh);
    let summary = serde_json::json!({
        "generator": "endfield-models export",
        "filter": args.filter,
        "lastRun": {
            "exported": exported,
            "failed": index.len() - exported,
            "totalBytes": total_bytes,
            "seconds": started.elapsed().as_secs_f64(),
            "scannedChunks": scanned.len(),
        },
        "entries": entries,
    });
    rig::store::write_atomic(&index_path, &serde_json::to_vec_pretty(&summary)?)?;
    eprintln!(
        "exported {exported}/{} prefab(s), {:.1} MB, in {:.0}s",
        candidates.len(),
        total_bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// `(container path lowercased, cab, path_id)` of every stored animation container.
fn animation_containers(
    records: &[rig::store::CabRecord],
    stored_root: Option<&Path>,
) -> Vec<(String, String, i64)> {
    let mut out = Vec::new();
    for rec in records {
        if !rec.classes.contains_key(&74) {
            continue;
        }
        if let Some(root) = stored_root
            && !root.join("sf").join(&rec.cab).is_file()
        {
            continue;
        }
        for (path, file_id, path_id) in &rec.containers {
            let lower = path.to_ascii_lowercase();
            if *file_id == 0
                && lower.contains("/animations/")
                && (lower.ends_with(".fbx") || lower.ends_with(".anim"))
            {
                out.push((lower, rec.cab.clone(), *path_id));
            }
        }
    }
    out.sort();
    out
}

/// Clips for an entity, found by container path. Operators keep clips in
/// `…/<code>/animations/**`, monsters in `…/monster/animations/<code>/**`, where `<code>` is
/// the id's third `_` segment (`chr_0005_chen` -> `chen`, `eny_0054_hsmino_chrdg` -> `hsmino`).
/// Keywords are tried in order and the first one with any match wins, so `uiteam_idle,idle`
/// prefers the UI idles and only falls back to every `idle` clip. Additive clips are skipped:
/// they only make sense layered over a base pose. At most `max` container paths are taken.
fn select_clips(
    index: &[(String, String, i64)],
    id: &str,
    keywords: &[String],
    max: usize,
) -> Vec<(String, i64)> {
    let owned = owned_clips(index, id);
    for keyword in keywords
        .iter()
        .flat_map(|k| k.split(','))
        .map(|k| k.trim().to_ascii_lowercase())
    {
        if keyword.is_empty() {
            continue;
        }
        let mut paths: Vec<&str> = Vec::new();
        let mut out = Vec::new();
        for (path, cab, path_id) in owned.iter().copied() {
            let file = path.rsplit('/').next().unwrap_or(path);
            // Additive clips only make sense layered over a base pose.
            if !file.contains(&keyword) || file.contains("additive") {
                continue;
            }
            if !paths.contains(&path.as_str()) {
                if paths.len() >= max {
                    continue;
                }
                paths.push(path);
            }
            out.push((cab.clone(), *path_id));
        }
        if !out.is_empty() {
            return out;
        }
    }
    Vec::new()
}

/// Every animation container entry belonging to an entity (see [`select_clips`]).
fn owned_clips<'a>(index: &'a [(String, String, i64)], id: &str) -> Vec<&'a (String, String, i64)> {
    let code = clip_code(id);
    let in_folder = |prefix: bool| -> Vec<&(String, String, i64)> {
        index
            .iter()
            .filter(|(path, _, _)| {
                let Some(at) = path.find("/animations/") else {
                    return false;
                };
                let before = path[..at].rsplit('/').next().unwrap_or("");
                let after = path[at + "/animations/".len()..]
                    .split('/')
                    .next()
                    .unwrap_or("");
                clip_folder_matches(before, &code, prefix)
                    || clip_folder_matches(after, &code, prefix)
            })
            .collect()
    };
    // Exact folders first, so `chen` never picks up `chenpast/`; prefixes only as a fallback.
    let mut owned = in_folder(false);
    if owned.is_empty() {
        // A prefix match is only trusted when it names exactly one folder.
        let folder_of = |path: &str| path.find("/animations/").map(|at| path[..at].to_string());
        let candidates = in_folder(true);
        let folders: BTreeSet<Option<String>> =
            candidates.iter().map(|(p, _, _)| folder_of(p)).collect();
        if folders.len() == 1 {
            owned = candidates;
        }
    }
    owned
}

/// The model code an entity's animation folders are named after: the id's third `_` segment
/// (`chr_0005_chen` -> `chen`, `eny_0054_hsmino_chrdg` -> `hsmino`), else its last segment.
fn clip_code(id: &str) -> String {
    let parts: Vec<&str> = id.split('_').collect();
    parts
        .get(2)
        .or(parts.last())
        .unwrap_or(&id)
        .to_ascii_lowercase()
}

/// Whether an animation folder belongs to `code`: an exact match, the code without its variant
/// digits (`hsmino1` -> `hsmino`), or with `prefix` a longer name (`purrche` -> `purrchena`).
fn clip_folder_matches(folder: &str, code: &str, prefix: bool) -> bool {
    let base = code.trim_end_matches(|c: char| c.is_ascii_digit());
    !folder.is_empty() && (folder == code || folder == base || (prefix && folder.starts_with(code)))
}

struct ClipsArgs {
    store: PathBuf,
    models: PathBuf,
    kinds: Vec<String>,
    ids: Vec<String>,
    jobs: usize,
    skip_existing: bool,
    root_motion: bool,
    tolerance: rig::clips::Tolerance,
}

/// One entity's line in `clips/summary.json`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClipsSummary {
    kind: String,
    id: String,
    index: String,
    candidates: usize,
    /// Why no human description was found (humanoid clips are then skipped).
    avatar_error: Option<String>,
    seconds: f64,
    error: Option<String>,
    #[serde(flatten)]
    report: rig::clips::EntityClips,
}

/// `endfield-models clips`: per-clip GLBs for every exported (non-weapon) model, on `jobs`
/// threads. Each worker keeps its own bundle cache, cleared between entities.
fn cmd_clips(args: &ClipsArgs) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let started = Instant::now();
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(args.models.join("index.json"))?)?;
    let mut entities: Vec<(String, String, String)> = index["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let glb = e.get("glb")?.as_str()?.to_string();
            let kind = e.get("kind")?.as_str()?.to_string();
            let id = e.get("id")?.as_str()?.to_string();
            Some((kind, id, glb))
        })
        .filter(|(kind, _, _)| kind != "weapon")
        .filter(|(kind, _, _)| args.kinds.is_empty() || args.kinds.contains(kind))
        .filter(|(_, id, _)| {
            args.ids.is_empty() || args.ids.iter().any(|f| id.contains(f.as_str()))
        })
        .collect();
    entities.sort();
    entities.dedup_by(|a, b| a.2 == b.2);
    let records = rig::store::read_index(&args.store)?;
    let clip_index = animation_containers(&records, Some(&args.store));
    let avatars = rig::humanoid::avatar_containers(&records, &args.store);
    drop(records);
    eprintln!(
        "clips: {} entities, {} stored animation container entries, {} job(s)",
        entities.len(),
        clip_index.len(),
        args.jobs
    );
    let clips_root = args.models.join("clips");
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<ClipsSummary>> = Mutex::new(Vec::new());
    let written: Mutex<rig::clips::WrittenFiles> = Mutex::new(HashMap::new());
    std::thread::scope(|scope| {
        for _ in 0..args.jobs.max(1) {
            scope.spawn(|| {
                let mut src = rig::store::CabSource::new(
                    Some(args.store.clone()),
                    rig::ChunkFiles::default(),
                );
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((kind, id, glb)) = entities.get(i) else {
                        break;
                    };
                    let t0 = Instant::now();
                    let stem = glb.strip_suffix(".glb").unwrap_or(glb);
                    let out_dir = clips_root.join(stem);
                    if args.skip_existing && out_dir.join("index.json").is_file() {
                        continue;
                    }
                    let sources: Vec<(String, String, i64)> =
                        owned_clips(&clip_index, id).into_iter().cloned().collect();
                    let mut avatar_error = None;
                    let outcome = rig::clips::read_glb_json(&args.models.join(glb))
                        .and_then(|doc| {
                            let mut model = rig::clips::ModelRig::from_document(&doc)?;
                            let human = match rig::humanoid::find_avatar(&mut src, &avatars, &doc) {
                                Ok(avatar) => {
                                    model.choose_root(&rig::humanoid::skeleton_ids(&avatar));
                                    model.set_motion_bone(rig::humanoid::root_motion_bone(&avatar));
                                    rig::humanoid::Humanoid::from_avatar(&avatar)
                                        .map_err(|e| avatar_error = Some(e))
                                        .ok()
                                }
                                Err(e) => {
                                    avatar_error = Some(e);
                                    None
                                }
                            };
                            Ok((model, human))
                        })
                        .and_then(|(model, human)| {
                            let code = clip_code(id);
                            let job = rig::clips::EntityJob {
                                id,
                                kind,
                                code: &code,
                                rig: &model,
                                sources: &sources,
                                out_dir: &out_dir,
                                root_motion: args.root_motion,
                                tolerance: args.tolerance,
                                human: human.as_ref(),
                                written: Some(&written),
                            };
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                rig::clips::export_entity(&mut src, &job)
                            }))
                            .unwrap_or_else(|_| Err("clip export panicked".into()))
                        });
                    src.clear();
                    let seconds = t0.elapsed().as_secs_f64();
                    let (report, error) = match outcome {
                        Ok(r) => (r, None),
                        Err(e) => (rig::clips::EntityClips::default(), Some(e)),
                    };
                    eprintln!(
                        "[{}/{}] {kind}/{id}: {} clips ({} humanoid, {} linked), {:.1} MB, skipped {} (additive {}, other skeleton {}, non-ACL {}, no avatar {}, duplicate {}, failed {}){} in {seconds:.1}s",
                        i + 1,
                        entities.len(),
                        report.clips,
                        report.humanoid,
                        report.linked,
                        report.bytes as f64 / 1e6,
                        report.skipped.len(),
                        report.additive,
                        report.other_skeleton,
                        report.non_acl,
                        report.no_avatar,
                        report.duplicates,
                        report.failed,
                        error.as_ref().map(|e| format!(" ERROR {e}")).unwrap_or_default(),
                    );
                    results
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(ClipsSummary {
                            kind: kind.clone(),
                            id: id.clone(),
                            index: format!("clips/{stem}/index.json"),
                            candidates: sources.len(),
                            avatar_error,
                            seconds,
                            error,
                            report,
                        });
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner());
    results.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    // Merge with earlier runs (other kinds / ids) by index path.
    std::fs::create_dir_all(&clips_root)?;
    let summary_path = clips_root.join("summary.json");
    let mut entries: BTreeMap<String, serde_json::Value> = std::fs::read(&summary_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("entities").and_then(|e| e.as_array()).cloned())
        .into_iter()
        .flatten()
        .filter_map(|e| Some((e.get("index")?.as_str()?.to_string(), e)))
        .collect();
    for r in &results {
        entries.insert(r.index.clone(), serde_json::to_value(r)?);
    }
    let total = |key: &str| -> u64 {
        entries
            .values()
            .map(|e| e.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0))
            .sum()
    };
    let summary = serde_json::json!({
        "generator": "endfield-models clips",
        "totals": {
            "entities": entries.len(),
            "clips": total("clips"),
            "bytes": total("bytes"),
            "nonAcl": total("nonAcl"),
            "additive": total("additive"),
            "otherSkeleton": total("otherSkeleton"),
            "duplicates": total("duplicates"),
            "failed": total("failed"),
            "linked": total("linked"),
            "humanoid": total("humanoid"),
            "noAvatar": total("noAvatar"),
        },
        "entities": entries.into_values().collect::<Vec<_>>(),
    });
    rig::store::write_atomic(&summary_path, &serde_json::to_vec_pretty(&summary)?)?;
    let clips: usize = results.iter().map(|r| r.report.clips).sum();
    let bytes: u64 = results.iter().map(|r| r.report.bytes).sum();
    eprintln!(
        "wrote {clips} clips for {} entities, {:.1} MB, in {:.0}s",
        results.len(),
        bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Character-screen data of every exported operator (see `Commands::Charinfo`).
fn cmd_charinfo(
    store: &Path,
    models: &Path,
    data_root: &Path,
    ids: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    use rig::charinfo;
    let started = Instant::now();
    let records = rig::store::read_index(store)?;
    let stored = |cab: &str| store.join("sf").join(cab).is_file();
    let containers = charinfo::uimodel_containers(&records, &stored);
    let tables = charinfo::Tables::load(data_root);
    let library = std::sync::Arc::new(rig::material::TextureLibrary::load(
        &data_root.join("textures"),
    ));
    let opts = rig::ExportOptions {
        material: rig::material::MaterialOptions {
            library: Some(library),
            all_textures: true,
            encoding: rig::material::ImageEncoding::Auto,
            max_texture_size: None,
        },
        all_lods: false,
    };
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(models.join("index.json"))?)?;
    let operators: Vec<(String, String)> = index
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|e| e.get("kind").and_then(serde_json::Value::as_str) == Some("operator"))
        .filter_map(|e| {
            let id = e.get("id")?.as_str()?;
            let glb = e.get("glb")?.as_str()?;
            Some((id.to_string(), glb.strip_suffix(".glb")?.to_string()))
        })
        .filter(|(id, _)| ids.is_empty() || ids.iter().any(|f| id.contains(f.as_str())))
        .collect();
    eprintln!(
        "{} operator(s), {} UI-model container(s)",
        operators.len(),
        containers.len()
    );
    let ctx = charinfo::Context {
        records: &records,
        containers: &containers,
        tables: &tables,
        opts: &opts,
        models,
    };
    let mut source = rig::store::CabSource::new(Some(store.to_path_buf()), ChunkFiles::default());
    let write_json = |path: PathBuf, value: &serde_json::Value| -> std::io::Result<()> {
        rig::store::write_atomic(&path, &serde_json::to_vec_pretty(value)?)
    };
    let mut written = 0usize;
    for (n, (id, stem)) in operators.iter().enumerate() {
        let clip_index = std::fs::read(models.join("clips").join(stem).join("index.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<serde_json::Value>>(&b).ok())
            .unwrap_or_default();
        let clips = charinfo::ClipIndex::from_entries(&clip_index);
        let nodes: HashSet<String> = rig::clips::read_glb_json(&models.join(format!("{stem}.glb")))
            .ok()
            .and_then(|doc| {
                Some(
                    doc.get("nodes")?
                        .as_array()?
                        .iter()
                        .filter_map(|n| n.get("name")?.as_str().map(str::to_string))
                        .collect(),
                )
            })
            .unwrap_or_default();
        let data = charinfo::character(&mut source, &ctx, id, &clips, &nodes);
        let mut parts = Vec::new();
        if let Some(menu) = &data.menu {
            write_json(models.join("clips").join(stem).join("menu.json"), menu)?;
            parts.push(format!(
                "menu {} states",
                menu.get("states")
                    .and_then(serde_json::Value::as_object)
                    .map_or(0, serde_json::Map::len)
            ));
        }
        if let Some(weapon) = &data.weapon {
            write_json(
                models.join("charinfo/weapons").join(format!("{id}.json")),
                weapon,
            )?;
            parts.push(format!(
                "weapon {}",
                weapon
                    .get("weapon")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?")
            ));
        }
        if let Some((deco, glbs)) = &data.deco {
            let dir = models.join("charinfo/deco").join(id);
            for (name, glb) in glbs {
                rig::store::write_atomic(&dir.join(name), glb)?;
            }
            write_json(dir.join("index.json"), deco)?;
            parts.push(format!("deco {} prop GLB(s)", glbs.len()));
        }
        if let Some(lights) = rig::lights::character_lights(&mut source, &records, id, &stored) {
            write_json(models.join("lights").join(format!("{id}.json")), &lights)?;
            if lights.get("camera").is_some_and(|c| !c.is_null()) {
                parts.push("camera".into());
            }
        }
        if !parts.is_empty() {
            written += 1;
        }
        eprintln!(
            "[{}/{}] {id}: {}{}",
            n + 1,
            operators.len(),
            if parts.is_empty() {
                "nothing".into()
            } else {
                parts.join(", ")
            },
            if data.warnings.is_empty() {
                String::new()
            } else {
                format!(" ({})", data.warnings.join("; "))
            }
        );
        if (n + 1) % 4 == 0 {
            source.clear();
        }
    }
    eprintln!(
        "charinfo: {written}/{} operator(s) in {:.0}s",
        operators.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Export an Avatar-only model import: its bundle's Avatar plus every Mesh of the container.
fn export_avatar_candidate(
    source: &mut rig::store::CabSource,
    cand: &rig::entity::Candidate,
    args: &ExportArgs,
    clips: Vec<(String, i64)>,
    materials: &rig::material::MaterialAssets,
) -> Result<(String, rig::EntityExport), String> {
    let cab = cand
        .sources
        .first()
        .map(|(c, _)| c.clone())
        .ok_or("no objects")?;
    let file = source
        .get(&cab)
        .ok_or_else(|| format!("{cab} not loadable"))?;
    let class_of = |path_id: i64| {
        source.files.files[file]
            .sf
            .objects
            .iter()
            .find(|o| o.path_id == path_id)
            .map(|o| o.class_id)
    };
    let refs = |class: i32| -> Vec<rig::ObjRef> {
        cand.sources
            .iter()
            .filter(|(_, p)| class_of(*p) == Some(class))
            .map(|(_, p)| rig::ObjRef { file, path_id: *p })
            .collect()
    };
    let avatar = *refs(90).first().ok_or("import has no Avatar")?;
    let meshes = refs(43);
    let extras = serde_json::json!({
        "source": {"container": cand.container, "cab": cab, "chunks": cand.chunks, "avatarOnly": true},
        "entity": {"id": cand.id, "kind": cand.kind},
    });
    let mut exporter = rig::Exporter::new(source, args.opts.clone());
    exporter.clip_sources = clips;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let near = rig::material::entity_dir(&cand.container);
        exporter.export_avatar(avatar, &meshes, &cand.id, extras, materials, &near)
    }))
    .unwrap_or_else(|_| Err("exporter panicked".into()))
    .map(|r| (cab, r))
}

/// Run one entity export, turning a panic into an error so one bad entity can't end the run.
fn export_isolated(
    exporter: rig::Exporter<'_>,
    root: rig::ObjRef,
    extras: serde_json::Value,
) -> Result<rig::EntityExport, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        exporter.export(root, extras)
    }))
    .unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_default();
        Err(format!("exporter panicked: {message}"))
    })
}

#[cfg(test)]
mod tests {
    use super::{clip_code, clip_folder_matches, select_clips};

    #[test]
    fn clip_codes_and_folders() {
        assert_eq!(clip_code("chr_0005_chen"), "chen");
        assert_eq!(clip_code("eny_0054_hsmino_chrdg"), "hsmino");
        assert!(clip_folder_matches("hsmino", "hsmino1", false));
        assert!(clip_folder_matches("purrchena", "purrche", true));
        assert!(!clip_folder_matches("purrchena", "purrche", false));
        assert!(!clip_folder_matches("hsminox", "hsmino1", true));
    }

    #[test]
    fn keywords_are_tried_in_order() {
        let index = vec![
            (
                "assets/a/actor/girl/chen/animations/ui/a_actor_chen_idle_avoid.fbx".to_string(),
                "C1".to_string(),
                1,
            ),
            (
                "assets/a/actor/girl/chen/animations/ui/a_actor_chen_uiteam_idle_01.fbx"
                    .to_string(),
                "C2".to_string(),
                2,
            ),
            (
                "assets/a/monster/animations/hsmino/a_monster_hsmino_idle.fbx".to_string(),
                "C3".to_string(),
                3,
            ),
            (
                "assets/a/npc/girl/chenpast/animations/a_chenpast_uiteam_idle_01.fbx".to_string(),
                "C4".to_string(),
                4,
            ),
        ];
        let kw = vec!["uiteam_idle,idle".to_string()];
        assert_eq!(
            select_clips(&index, "chr_0005_chen", &kw, 3),
            vec![("C2".to_string(), 2)]
        );
        assert_eq!(
            select_clips(&index, "eny_0054_hsmino1", &kw, 3),
            vec![("C3".to_string(), 3)]
        );
        assert!(select_clips(&index, "eny_0001_other", &kw, 3).is_empty());
    }
}
