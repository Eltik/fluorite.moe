//! Health evaluation of the `update-log.jsonl` run log written by `endfield update`.
//!
//! Each check turns the parsed log into an `OK` / `WARN` / `FAIL` finding. Lines written by
//! older versions lack the newer counters, so every check only looks at entries that carry
//! the keys it needs.
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

/// Where to look when MemoryPack decoding degrades after a game update.
pub const SCHEMA_README: &str = "backend/scripts/memorypack-schema/README.md";

/// Failure rate above which MemoryPack decoding counts as degraded.
const MEMORYPACK_FAIL_RATE: f64 = 0.01;

/// Number of consecutive failing runs that escalates a warning to a failure.
const FAILING_STREAK: usize = 3;

/// One run's summary line. Every field is optional so old and partial lines still parse.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LogEntry {
    pub at: Option<String>,
    pub version: Option<String>,
    pub chk_failed: Option<u64>,
    pub sort_failed: Option<u64>,
    pub blc_failed: Option<u64>,
    pub tex_failed: Option<u64>,
    pub mesh_failed: Option<u64>,
    pub memorypack_decoded: Option<u64>,
    pub memorypack_failed: Option<u64>,
    pub schema_version: Option<String>,
}

impl LogEntry {
    fn failures(&self) -> u64 {
        [self.chk_failed, self.sort_failed, self.blc_failed]
            .into_iter()
            .flatten()
            .sum()
    }

    fn at(&self) -> Option<DateTime<Utc>> {
        let at = DateTime::parse_from_rfc3339(self.at.as_deref()?).ok()?;
        Some(at.with_timezone(&Utc))
    }
}

/// Parse a JSONL log, oldest first, skipping blank and garbled lines.
pub fn parse_log(text: &str) -> Vec<LogEntry> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Ok => "OK",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        }
    }

    /// Process exit code for a run whose worst finding is `self`.
    pub fn exit_code(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub message: String,
}

impl Finding {
    fn new(level: Level, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Oldest acceptable age of the newest run.
    pub max_age: Duration,
}

/// Evaluate every check against `entries` (oldest first) as of `now`.
pub fn evaluate(entries: &[LogEntry], now: DateTime<Utc>, thresholds: Thresholds) -> Vec<Finding> {
    let Some(newest) = entries.last() else {
        return vec![Finding::new(Level::Fail, "update log is missing or empty")];
    };
    let mut findings = vec![check_age(newest, now, thresholds.max_age)];
    findings.push(check_failures(entries));
    findings.extend(check_memorypack(entries));
    findings.push(check_regression(entries, "texture", |e| e.tex_failed));
    findings.push(check_regression(entries, "mesh", |e| e.mesh_failed));
    findings
}

/// Worst level among `findings`, `Ok` when there are none.
pub fn worst(findings: &[Finding]) -> Level {
    findings.iter().map(|f| f.level).max().unwrap_or(Level::Ok)
}

fn check_age(newest: &LogEntry, now: DateTime<Utc>, max_age: Duration) -> Finding {
    let Some(at) = newest.at() else {
        return Finding::new(Level::Fail, "newest log entry has no valid timestamp");
    };
    let age = now - at;
    let hours = age.num_minutes() as f64 / 60.0;
    let version = newest.version.as_deref().unwrap_or("unknown version");
    if age > max_age {
        Finding::new(
            Level::Fail,
            format!("last update ran {hours:.1} h ago — is the scheduler running?"),
        )
    } else {
        Finding::new(
            Level::Ok,
            format!("last update ran {hours:.1} h ago ({version})"),
        )
    }
}

fn check_failures(entries: &[LogEntry]) -> Finding {
    let newest = entries.last().map_or(0, LogEntry::failures);
    if newest == 0 {
        return Finding::new(Level::Ok, "last run had no failures");
    }
    let streak = entries.len() >= FAILING_STREAK
        && entries[entries.len() - FAILING_STREAK..]
            .iter()
            .all(|e| e.failures() > 0);
    if streak {
        Finding::new(
            Level::Fail,
            format!(
                "the last {FAILING_STREAK} runs all had failures ({newest} in the newest); retries are not recovering"
            ),
        )
    } else {
        Finding::new(
            Level::Warn,
            format!("last run had {newest} failures (retried next run)"),
        )
    }
}

fn major_minor(version: &str) -> &str {
    match version.match_indices('.').nth(1) {
        Some((i, _)) => &version[..i],
        None => version,
    }
}

fn check_memorypack(entries: &[LogEntry]) -> Vec<Finding> {
    let mut findings = Vec::new();
    // A run that left JsonData untouched records 0/0, which says nothing about the decoder.
    let counted = entries.iter().rev().find_map(|e| {
        let decoded = e.memorypack_decoded?;
        let failed = e.memorypack_failed?;
        (decoded + failed > 0).then_some((e, decoded, failed))
    });
    match counted {
        Some((entry, decoded, failed)) => {
            let rate = failed as f64 / (decoded + failed) as f64;
            let game = entry.version.as_deref().unwrap_or("unknown");
            let schema = entry.schema_version.as_deref().unwrap_or("unknown");
            if rate > MEMORYPACK_FAIL_RATE {
                findings.push(Finding::new(
                    Level::Warn,
                    format!(
                        "MemoryPack: {failed} of {} files failed to decode ({:.1}%) on game {game} with schema {schema}; regenerate the schema (see {SCHEMA_README})",
                        decoded + failed,
                        rate * 100.0
                    ),
                ));
            } else {
                findings.push(Finding::new(
                    Level::Ok,
                    format!(
                        "MemoryPack: {decoded} decoded, {failed} failed ({:.2}%)",
                        rate * 100.0
                    ),
                ));
            }
        }
        None => findings.push(Finding::new(
            Level::Ok,
            "MemoryPack: no run has recorded decode counts yet",
        )),
    }

    let versions = entries
        .iter()
        .rev()
        .find_map(|e| Some((e.version.as_deref()?, e.schema_version.as_deref()?)));
    if let Some((game, schema)) = versions {
        if major_minor(game) != major_minor(schema) {
            findings.push(Finding::new(
                Level::Warn,
                format!(
                    "MemoryPack schema {schema} was compiled for a different game version than {game}; regenerate it (see {SCHEMA_README})"
                ),
            ));
        } else {
            findings.push(Finding::new(
                Level::Ok,
                format!("MemoryPack schema {schema} matches game {game}"),
            ));
        }
    }
    findings
}

/// Compare the newest two entries carrying the counter; a jump beyond max(50, 10%) warns.
fn check_regression(
    entries: &[LogEntry],
    what: &str,
    failed: impl Fn(&LogEntry) -> Option<u64>,
) -> Finding {
    let mut carrying = entries.iter().rev().filter_map(&failed);
    let (Some(newest), Some(previous)) = (carrying.next(), carrying.next()) else {
        return Finding::new(
            Level::Ok,
            format!("{what} failures: not enough runs to compare yet"),
        );
    };
    let allowed = 50.max(previous.div_ceil(10));
    if newest > previous + allowed {
        Finding::new(
            Level::Warn,
            format!("{what} failures rose from {previous} to {newest} since the previous run"),
        )
    } else {
        Finding::new(
            Level::Ok,
            format!("{what} failures: {newest} (previous run {previous})"),
        )
    }
}
