use crate::extraction::health::{self, Finding, Level, LogEntry, Thresholds};
use chrono::{DateTime, Duration, Utc};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-20T12:00:00+00:00")
        .unwrap()
        .with_timezone(&Utc)
}

fn thresholds() -> Thresholds {
    Thresholds {
        max_age: Duration::hours(26),
    }
}

/// A clean run `hours_ago` hours before `now()`, in the pre-health log format.
fn run(hours_ago: i64) -> LogEntry {
    LogEntry {
        at: Some((now() - Duration::hours(hours_ago)).to_rfc3339()),
        version: Some("1.6.4".into()),
        chk_failed: Some(0),
        sort_failed: Some(0),
        blc_failed: Some(0),
        ..LogEntry::default()
    }
}

fn evaluate(entries: &[LogEntry]) -> Vec<Finding> {
    health::evaluate(entries, now(), thresholds())
}

/// The finding whose message contains `needle`.
fn finding<'a>(findings: &'a [Finding], needle: &str) -> &'a Finding {
    findings
        .iter()
        .find(|f| f.message.contains(needle))
        .unwrap_or_else(|| panic!("no finding containing {needle:?} in {findings:?}"))
}

#[test]
fn parse_log_skips_blank_and_garbled_lines() {
    let text = concat!(
        r#"{"at":"2026-10-19T08:00:00+00:00","version":"1.6.4","chk_failed":0}"#,
        "\n\n   \n",
        "{not json\n",
        r#"{"at":"2026-10-20T08:00:00+00:00","version":"1.6.4","tex_failed":3,"schema_version":"1.6.4"}"#,
        "\n",
        r#"{"at":"2026-10-20T09:00:00+00:00","chk_fa"#,
    );
    let entries = health::parse_log(text);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].tex_failed, None);
    assert_eq!(entries[1].tex_failed, Some(3));
    assert_eq!(entries[1].schema_version.as_deref(), Some("1.6.4"));
}

#[test]
fn empty_log_fails() {
    let findings = evaluate(&[]);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].level, Level::Fail);
    assert_eq!(health::worst(&findings), Level::Fail);
}

#[test]
fn healthy_old_format_log_is_ok() {
    let findings = evaluate(&[run(30), run(2)]);
    assert_eq!(health::worst(&findings), Level::Ok, "{findings:?}");
    assert_eq!(health::worst(&findings).exit_code(), 0);
}

#[test]
fn stale_newest_entry_fails() {
    let findings = evaluate(&[run(30)]);
    let age = finding(&findings, "last update ran");
    assert_eq!(age.level, Level::Fail);
    assert!(age.message.contains("30.0 h ago"));
    assert!(age.message.contains("scheduler"));
}

#[test]
fn unparseable_timestamp_fails() {
    let mut entry = run(1);
    entry.at = Some("yesterday".into());
    let findings = evaluate(&[entry]);
    assert_eq!(finding(&findings, "timestamp").level, Level::Fail);
}

#[test]
fn newest_run_failures_warn() {
    let mut failing = run(1);
    failing.blc_failed = Some(2);
    failing.sort_failed = Some(1);
    let findings = evaluate(&[run(3), run(2), failing]);
    let check = finding(&findings, "failures");
    assert_eq!(check.level, Level::Warn);
    assert!(check.message.contains('3'));
    assert_eq!(health::worst(&findings).exit_code(), 1);
}

#[test]
fn three_failing_runs_fail() {
    let failing = |hours| {
        let mut entry = run(hours);
        entry.chk_failed = Some(1);
        entry
    };
    let findings = evaluate(&[run(4), failing(3), failing(2), failing(1)]);
    assert_eq!(finding(&findings, "last 3 runs").level, Level::Fail);
    assert_eq!(health::worst(&findings).exit_code(), 2);

    // Only two runs logged: a warning, not a streak.
    let findings = evaluate(&[failing(2), failing(1)]);
    assert_eq!(finding(&findings, "last run had 1").level, Level::Warn);
}

fn memorypack_run(hours: i64, decoded: u64, failed: u64, schema: &str) -> LogEntry {
    LogEntry {
        memorypack_decoded: Some(decoded),
        memorypack_failed: Some(failed),
        schema_version: Some(schema.into()),
        ..run(hours)
    }
}

#[test]
fn memorypack_failure_rate_warns() {
    let findings = evaluate(&[memorypack_run(1, 980, 20, "1.6.4")]);
    let check = finding(&findings, "failed to decode");
    assert_eq!(check.level, Level::Warn);
    assert!(check.message.contains("game 1.6.4"));
    assert!(check.message.contains("schema 1.6.4"));
    assert!(check.message.contains(health::SCHEMA_README));

    // 1% exactly is still within tolerance.
    let findings = evaluate(&[memorypack_run(1, 990, 10, "1.6.4")]);
    assert_eq!(health::worst(&findings), Level::Ok, "{findings:?}");
}

#[test]
fn memorypack_ignores_runs_without_decodes() {
    // The newest run left JsonData alone (0/0), so the earlier degraded run still counts.
    let findings = evaluate(&[
        memorypack_run(3, 500, 100, "1.6.4"),
        memorypack_run(1, 0, 0, "1.6.4"),
    ]);
    assert_eq!(finding(&findings, "failed to decode").level, Level::Warn);
}

#[test]
fn schema_version_mismatch_warns() {
    let mut entry = memorypack_run(1, 1000, 0, "1.6.4");
    entry.version = Some("1.7.0".into());
    let findings = evaluate(&[entry]);
    let check = finding(&findings, "different game version");
    assert_eq!(check.level, Level::Warn);
    assert!(check.message.contains("1.7.0"));

    // Patch releases share a schema.
    let mut entry = memorypack_run(1, 1000, 0, "1.6.4");
    entry.version = Some("1.6.9".into());
    assert_eq!(health::worst(&evaluate(&[entry])), Level::Ok);
}

fn asset_run(hours: i64, tex_failed: u64, mesh_failed: u64) -> LogEntry {
    LogEntry {
        tex_failed: Some(tex_failed),
        mesh_failed: Some(mesh_failed),
        ..run(hours)
    }
}

#[test]
fn texture_failure_jump_warns() {
    // Previous 1000: allowed growth is 10% (100), so 1101 warns and 1100 does not.
    let findings = evaluate(&[asset_run(2, 1000, 0), asset_run(1, 1101, 0)]);
    let check = finding(&findings, "texture failures rose");
    assert_eq!(check.level, Level::Warn);
    assert!(check.message.contains("1000 to 1101"));

    let findings = evaluate(&[asset_run(2, 1000, 0), asset_run(1, 1100, 0)]);
    assert_eq!(health::worst(&findings), Level::Ok, "{findings:?}");
}

#[test]
fn mesh_failure_jump_uses_floor_of_fifty() {
    let findings = evaluate(&[asset_run(2, 0, 10), asset_run(1, 0, 61)]);
    assert_eq!(finding(&findings, "mesh failures rose").level, Level::Warn);

    let findings = evaluate(&[asset_run(2, 0, 10), asset_run(1, 0, 60)]);
    assert_eq!(health::worst(&findings), Level::Ok, "{findings:?}");
}

#[test]
fn asset_regression_skips_entries_without_counts() {
    // Old-format lines between the two carrying entries are ignored.
    let findings = evaluate(&[asset_run(4, 0, 0), run(3), run(2), asset_run(1, 500, 0)]);
    assert_eq!(
        finding(&findings, "texture failures rose").level,
        Level::Warn
    );

    // A single carrying entry has no baseline.
    let findings = evaluate(&[run(2), asset_run(1, 500, 500)]);
    assert_eq!(health::worst(&findings), Level::Ok, "{findings:?}");
}
