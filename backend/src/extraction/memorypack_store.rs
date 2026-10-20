//! The runtime MemoryPack schema store and the flow that keeps it current.
//!
//! Generated schemas live in `<state dir>/memorypack-schema/<game version>-<crc32>.json.gz`,
//! next to `update-state.json`, where `<crc32>` identifies the client's `GameAssembly.dll`.
//! Decoding uses every stored schema, newest first, and the schema embedded in the binary
//! last (see [`SchemaSet`]).
//!
//! [`sync`] is `endfield schema-gen` (and the automatic step of `endfield update`) minus
//! the download: generate a schema from the client files, learn its `dir_roots` from the
//! extracted `JsonData/Json` tree, validate it against the schemas in use, install it, and
//! re-decode the JsonData groups from their `.mpk` sources.
use super::memorypack::{self, LabelledSchema, Schema, SchemaSet, StringPool, TreeDecodeResult};
use super::memorypack_schema::{self, GenerateError, SchemaMeta, SchemaReport};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Store folder name, next to the update state file.
pub const STORE_DIR: &str = "memorypack-schema";

/// Archive path of the client binary the schema is generated from.
pub const CLIENT_DLL: &str = "GameAssembly.dll";

/// Archive path of the IL2CPP metadata.
pub const CLIENT_METADATA: &str = "Endfield_Data/il2cpp_data/Metadata/global-metadata.dat";

/// Files listed by name in a report's new/lost lists (all are counted).
const LISTED_FILES: usize = 20;

/// The schema store of the update state at `state_path`.
pub fn store_dir(state_path: &Path) -> PathBuf {
    state_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join(STORE_DIR)
}

/// Store file name of the schema for `game_version` and a client whose `GameAssembly.dll`
/// has CRC-32 `client_crc32`.
pub fn store_file_name(game_version: &str, client_crc32: u32) -> String {
    let version: String = game_version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{version}-{client_crc32:08x}.json.gz")
}

/// Label of a store file: its name without `.json.gz`.
fn store_label(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_suffix(".json.gz").map(str::to_owned)
}

/// Every schema in `store_dir` (none when it does not exist), with a warning for each file
/// that does not parse; those are skipped.
pub fn load_store(store_dir: &Path) -> io::Result<(Vec<LabelledSchema>, Vec<String>)> {
    let entries = match fs::read_dir(store_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), Vec::new())),
        Err(e) => return Err(e),
    };
    let mut paths: Vec<PathBuf> = entries
        .map(|entry| entry.map(|e| e.path()))
        .collect::<io::Result<_>>()?;
    paths.sort();
    let (mut schemas, mut warnings) = (Vec::new(), Vec::new());
    for path in paths {
        let Some(label) = store_label(&path) else {
            continue;
        };
        match fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| Schema::from_gzip(&bytes))
        {
            Ok(schema) => schemas.push((label, Arc::new(schema))),
            Err(e) => warnings.push(format!("skipping schema {}: {e}", path.display())),
        }
    }
    Ok((schemas, warnings))
}

/// The schemas in `store_dir`, newest first, then the embedded schema.
pub fn load_set(store_dir: &Path) -> io::Result<(SchemaSet, Vec<String>)> {
    let (schemas, warnings) = load_store(store_dir)?;
    Ok((
        SchemaSet::new(schemas, Some(SchemaSet::embedded_entry()?)),
        warnings,
    ))
}

/// Write `bytes` to `store_dir/file_name` atomically (temporary file, sync, rename).
pub fn install(store_dir: &Path, file_name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    fs::create_dir_all(store_dir)?;
    let path = store_dir.join(file_name);
    let tmp = store_dir.join(format!(".{file_name}.tmp"));
    let mut file = File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, &path)?;
    Ok(path)
}

// ── Validation ──────────────────────────────────────────

/// How a candidate schema compares with the schemas in use, over every MemoryPack file of a
/// `Json/` tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validation {
    /// MemoryPack files seen.
    pub files: usize,
    /// Decoded by the candidate alone.
    pub candidate_decoded: usize,
    /// The newest schema in use (the baseline the candidate must not fall below).
    pub baseline: String,
    /// Decoded by the baseline alone.
    pub baseline_decoded: usize,
    /// Decoded by the current set (every schema in use).
    pub current_decoded: usize,
    /// Decoded by the candidate or the current set: what the set holds after installing.
    pub combined_decoded: usize,
    /// Files the candidate decodes and the current set does not (relative paths).
    pub new_files: Vec<String>,
    /// Files the current set decodes and the candidate does not.
    pub lost_files: Vec<String>,
}

/// Whether to install a validated candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Install,
    /// Not installed; the reason names both counts.
    Refuse(String),
}

/// Install unless the candidate alone decodes fewer files than the newest schema in use
/// (overridden by `force`).
pub fn decide(validation: &Validation, force: bool) -> Decision {
    if validation.candidate_decoded < validation.baseline_decoded && !force {
        return Decision::Refuse(format!(
            "regression: the new schema decodes {} of {} files, fewer than the {} of {} \
             (pass --force to install anyway)",
            validation.candidate_decoded,
            validation.files,
            validation.baseline_decoded,
            validation.baseline
        ));
    }
    Decision::Install
}

/// Per-file results of [`validate`].
#[derive(Default)]
struct Tally {
    files: usize,
    candidate: usize,
    baseline: usize,
    current: usize,
    combined: usize,
    new_files: Vec<String>,
    lost_files: Vec<String>,
}

/// Decode every MemoryPack file under `json_dir` with `candidate` alone, the newest schema of
/// `current` alone, and all of `current`. Nothing is written.
pub fn validate(
    candidate: &Schema,
    current: &SchemaSet,
    json_dir: &Path,
) -> io::Result<Validation> {
    let schemas: Vec<(&str, &Schema)> = current.entries().collect();
    let baseline = schemas.first().map_or("", |(label, _)| *label).to_owned();
    let pool = StringPool::load(&json_dir.join("StringPool"))?;
    let files = memorypack::json_files(json_dir)?;
    let workers = std::thread::available_parallelism().map_or(4, usize::from);
    let chunk = files.len().div_ceil(workers).max(1);
    let (pool, schemas) = (&pool, &schemas);
    let mut tally = std::thread::scope(|scope| -> io::Result<Tally> {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|batch| {
                scope.spawn(move || -> io::Result<Tally> {
                    let mut t = Tally::default();
                    for (path, rel_dir) in batch {
                        let Some(data) = memorypack_schema::memorypack_source(path)? else {
                            continue;
                        };
                        t.files += 1;
                        let decodes = |schema: &Schema| {
                            memorypack::decode_bytes(schema, pool, &data, rel_dir).is_some()
                        };
                        let cand = decodes(candidate);
                        let base = schemas.first().is_some_and(|(_, s)| decodes(s));
                        let cur = base || schemas.iter().skip(1).any(|(_, s)| decodes(s));
                        t.candidate += usize::from(cand);
                        t.baseline += usize::from(base);
                        t.current += usize::from(cur);
                        t.combined += usize::from(cand || cur);
                        if cand != cur {
                            let rel = path
                                .strip_prefix(json_dir)
                                .unwrap_or(path)
                                .to_string_lossy()
                                .replace('\\', "/");
                            if cand {
                                t.new_files.push(rel);
                            } else {
                                t.lost_files.push(rel);
                            }
                        }
                    }
                    Ok(t)
                })
            })
            .collect();
        let mut total = Tally::default();
        for handle in handles {
            let part = handle
                .join()
                .map_err(|_| io::Error::other("schema validation worker panicked"))??;
            total.files += part.files;
            total.candidate += part.candidate;
            total.baseline += part.baseline;
            total.current += part.current;
            total.combined += part.combined;
            total.new_files.extend(part.new_files);
            total.lost_files.extend(part.lost_files);
        }
        Ok(total)
    })?;
    tally.new_files.sort();
    tally.lost_files.sort();
    Ok(Validation {
        files: tally.files,
        candidate_decoded: tally.candidate,
        baseline,
        baseline_decoded: tally.baseline,
        current_decoded: tally.current,
        combined_decoded: tally.combined,
        new_files: tally.new_files,
        lost_files: tally.lost_files,
    })
}

// ── Sync ────────────────────────────────────────────────

/// Inputs of [`sync`].
pub struct SyncInput<'a> {
    pub dll: &'a [u8],
    pub metadata: &'a [u8],
    /// Game version the client files belong to (`"1.6.4"`).
    pub game_version: &'a str,
    /// CRC-32 of `dll`.
    pub client_crc32: u32,
    pub store_dir: &'a Path,
    /// `JsonData/Json` tree to learn `dir_roots` from and validate against (only read).
    pub json_dir: Option<&'a Path>,
    /// JsonData group folders (parents of `Json/`) to re-decode after installing.
    pub groups: &'a [PathBuf],
    /// Install even when the candidate decodes fewer files.
    pub force: bool,
}

/// What [`sync`] did with the candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Written to this store file.
    Installed(PathBuf),
    /// The store file already held the same schema (only `generated_at` differs).
    Unchanged(PathBuf),
    /// Not installed; see [`decide`].
    Refused(String),
}

/// What [`sync`] found and did.
#[derive(Debug, Clone)]
pub struct SyncReport {
    /// Store label of the candidate (`<game version>-<crc32>`).
    pub label: String,
    pub schema: SchemaReport,
    pub validation: Validation,
    pub outcome: SyncOutcome,
    /// Totals of re-decoding `groups` with the new set (after an install or no-op).
    pub redecode: Option<TreeDecodeResult>,
    /// Game version of the newest schema in use afterwards.
    pub schema_version: Option<String>,
    /// Store files that did not parse, and similar.
    pub warnings: Vec<String>,
    pub elapsed: Duration,
}

impl SyncReport {
    /// Whether the store holds the candidate afterwards.
    pub fn installed(&self) -> bool {
        !matches!(self.outcome, SyncOutcome::Refused(_))
    }

    /// The report as printable lines.
    pub fn lines(&self) -> Vec<String> {
        let s = &self.schema;
        let v = &self.validation;
        let mut out = vec![format!(
            "schema {}: {} typedefs, {} wrappers ({} closed generic), {} roots, {} nodes; \
             compiled in {:.2?}",
            self.label,
            s.typedefs,
            s.wrappers,
            s.generic_wrappers,
            s.roots,
            s.nodes,
            s.compile_time
        )];
        match &s.learn {
            Some(learn) => out.push(format!(
                "learned dir_roots for {} directories from {} files ({} decoded, {} not) in {:.2?}",
                learn.dirs, learn.files, learn.decoded, learn.failed_count, learn.elapsed
            )),
            None => out.push("no JsonData tree: dir_roots left empty, nothing to validate".into()),
        }
        let unsupported: usize = s.unsupported.values().sum();
        if unsupported > 0 {
            out.push(format!(
                "{unsupported} unsupported nodes ({} reasons)",
                s.unsupported.len()
            ));
        }
        for note in &s.notes {
            out.push(format!("note: {note}"));
        }
        out.push(format!(
            "validation over {} files: new schema alone {}, newest in use ({}) {}, current set {}, \
             with the new schema {}",
            v.files,
            v.candidate_decoded,
            if v.baseline.is_empty() {
                "none"
            } else {
                &v.baseline
            },
            v.baseline_decoded,
            v.current_decoded,
            v.combined_decoded
        ));
        for (label, files) in [("new", &v.new_files), ("lost", &v.lost_files)] {
            if files.is_empty() {
                continue;
            }
            out.push(format!(
                "{} files {label} vs the current set{}",
                files.len(),
                if files.len() > LISTED_FILES {
                    format!(" (first {LISTED_FILES})")
                } else {
                    String::new()
                }
            ));
            out.extend(
                files
                    .iter()
                    .take(LISTED_FILES)
                    .map(|f| format!("  {label}: {f}")),
            );
        }
        out.push(match &self.outcome {
            SyncOutcome::Installed(path) => format!("installed {}", path.display()),
            SyncOutcome::Unchanged(path) => format!("unchanged: {} is current", path.display()),
            SyncOutcome::Refused(reason) => format!("not installed: {reason}"),
        });
        if let Some(r) = &self.redecode {
            let by: Vec<String> = r
                .by_schema
                .iter()
                .map(|(l, n)| format!("{l} {n}"))
                .collect();
            out.push(format!(
                "re-decoded JsonData: {} decoded ({}), {} not decodable, {} write errors",
                r.decoded,
                by.join(", "),
                r.failed,
                r.write_errors
            ));
        }
        out.extend(self.warnings.iter().map(|w| format!("warning: {w}")));
        out.push(format!("schema-gen took {:.2?}", self.elapsed));
        out
    }
}

/// Whether the gzip schema at `path` equals `json` apart from `generated_at`.
fn same_schema(path: &Path, json: &[u8]) -> bool {
    let strip = |bytes: &[u8]| -> Option<serde_json::Value> {
        let mut value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        value.as_object_mut()?.remove("generated_at");
        Some(value)
    };
    let Ok(gz) = fs::read(path) else {
        return false;
    };
    let mut existing = Vec::new();
    if flate2::read::GzDecoder::new(&gz[..])
        .read_to_end(&mut existing)
        .is_err()
    {
        return false;
    }
    matches!((strip(&existing), strip(json)), (Some(a), Some(b)) if a == b)
}

/// Generate, validate and (unless it regresses) install a schema, then re-decode the
/// JsonData groups with the resulting set.
pub fn sync(input: &SyncInput) -> Result<SyncReport, GenerateError> {
    let started = Instant::now();
    let (current, mut warnings) = load_set(input.store_dir)?;
    let label = store_label(Path::new(&store_file_name(
        input.game_version,
        input.client_crc32,
    )))
    .unwrap_or_default();
    let meta = SchemaMeta {
        game_version: input.game_version.to_owned(),
        generated_at: Some(chrono::Utc::now().to_rfc3339()),
        client_crc32: Some(format!("{:08x}", input.client_crc32)),
    };
    let (json, schema_report) =
        memorypack_schema::generate(input.dll, input.metadata, &meta, input.json_dir)?;
    let candidate = Schema::from_json(&json).map_err(io::Error::other)?;
    let validation = match input.json_dir {
        Some(dir) => validate(&candidate, &current, dir)?,
        None => Validation {
            baseline: current.newest().map_or("", |(l, _)| l).to_owned(),
            ..Validation::default()
        },
    };
    let mut report = SyncReport {
        label: label.clone(),
        schema: schema_report,
        validation,
        outcome: SyncOutcome::Refused(String::new()),
        redecode: None,
        schema_version: None,
        warnings: Vec::new(),
        elapsed: Duration::ZERO,
    };
    if let Decision::Refuse(reason) = decide(&report.validation, input.force) {
        report.outcome = SyncOutcome::Refused(reason);
        report.schema_version = current
            .newest()
            .and_then(|(_, s)| s.game_version())
            .map(str::to_owned);
        report.warnings = warnings;
        report.elapsed = started.elapsed();
        return Ok(report);
    }

    let file_name = store_file_name(input.game_version, input.client_crc32);
    let path = input.store_dir.join(&file_name);
    report.outcome = if same_schema(&path, &json) {
        SyncOutcome::Unchanged(path)
    } else {
        let gz = memorypack_schema::gzip(&json)?;
        SyncOutcome::Installed(install(input.store_dir, &file_name, &gz)?)
    };
    drop(candidate);

    // Reload rather than patch the old set, so decoding uses exactly what later runs will.
    let (set, more) = load_set(input.store_dir)?;
    warnings.extend(more);
    warnings.dedup();
    let mut totals = TreeDecodeResult::default();
    for group in input.groups {
        totals.add(&memorypack::decode_tree_set(&set, group)?);
    }
    report.redecode = (!input.groups.is_empty()).then_some(totals);
    report.schema_version = set
        .newest()
        .and_then(|(_, s)| s.game_version())
        .map(str::to_owned);
    report.warnings = warnings;
    report.elapsed = started.elapsed();
    Ok(report)
}
