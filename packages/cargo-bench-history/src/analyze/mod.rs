//! The `analyze` command: load a project's stored history, reconstruct each
//! benchmark's time series, and report the notable changes.
//!
//! Like `run`, the pure logic (series reconstruction, finding detection, report
//! rendering) is sync and Miri-safe; only loading the stored objects touches an
//! async [`Storage`]. [`execute`] wires the configured storage backend, while
//! [`analyze_with`] is the storage-generic orchestrator the in-memory tests drive.

mod findings;
mod report;
mod series;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use crate::comparability::EngineSystem;
use crate::config::load_config;
use crate::model::ResultSet;
use crate::storage::{Storage, build_storage};
use crate::wiring::{default_config_path, resolve_project_id};
use crate::{AnalyzeOptions, RunError, RunOutcome};

use findings::{RegressionConfig, find_changes};
use report::{ReportFormat, ReportInput, render};
use series::{SeriesFilter, build_series};

/// The real `analyze`: load configuration, wire the configured storage, and orchestrate.
pub(crate) async fn execute(options: &AnalyzeOptions) -> Result<RunOutcome, RunError> {
    let config_path = options
        .config_path
        .clone()
        .unwrap_or_else(default_config_path);
    let config = load_config(&config_path).await?;

    let workspace_dir = std::env::current_dir().map_err(RunError::Io)?;
    let project_id = resolve_project_id(&config, &workspace_dir);
    let storage = build_storage(&config)?;

    analyze_with(&storage, &project_id, options).await
}

/// Storage-generic `analyze`: load the project's objects, build series, detect
/// changes, and render a report for the requested format.
pub(crate) async fn analyze_with<S: Storage>(
    storage: &S,
    project_id: &str,
    options: &AnalyzeOptions,
) -> Result<RunOutcome, RunError> {
    let format = parse_format(options.format.as_deref())?;
    let since = parse_since(options.since.as_deref())?;
    let system = parse_system(options.system.as_deref())?;

    let prefix = match system {
        Some(engine) => format!("v1/{project_id}/{}/", engine.as_str()),
        None => format!("v1/{project_id}/"),
    };

    let keys = storage.list(&prefix).await.map_err(RunError::Storage)?;

    let mut objects: Vec<(String, ResultSet)> = Vec::new();
    for key in keys {
        if !key.ends_with(".json") {
            continue;
        }
        let bytes = storage.get(&key).await.map_err(RunError::Storage)?;
        let text = String::from_utf8(bytes).map_err(|error| RunError::Analyze {
            message: format!("stored object {key} is not valid UTF-8: {error}"),
        })?;
        let set = ResultSet::from_json(&text).map_err(|error| RunError::Analyze {
            message: format!("stored object {key} is not a valid result set: {error}"),
        })?;
        objects.push((key, set));
    }

    // Scope the whole analysis to the requested time window: dropping out-of-window
    // runs here means the run count and every series reflect the same `--since`.
    if let Some(since) = since {
        objects.retain(|(_, set)| set.context.timestamps.effective >= since);
    }

    let runs = objects.len();
    let filter = SeriesFilter {
        metric: options.metric.as_deref(),
    };
    let series = build_series(&objects, &filter);
    let findings = find_changes(&series, &RegressionConfig::default());
    let regressions = findings
        .iter()
        .filter(|finding| finding.is_regression())
        .count();

    let input = ReportInput {
        project: project_id,
        runs,
        series: series.len(),
        findings: &findings,
    };
    let report = render(&input, format);

    Ok(RunOutcome::Analyzed {
        report,
        regressions,
        fail_on_regression: options.fail_on_regression,
    })
}

/// Parses the `--format` option, defaulting to text.
fn parse_format(name: Option<&str>) -> Result<ReportFormat, RunError> {
    match name {
        None => Ok(ReportFormat::Text),
        Some(name) => ReportFormat::from_name(name).ok_or_else(|| RunError::Analyze {
            message: format!("unknown report format {name:?}; expected text, json, or markdown"),
        }),
    }
}

/// Parses the `--system` option into an [`EngineSystem`], if set.
fn parse_system(name: Option<&str>) -> Result<Option<EngineSystem>, RunError> {
    match name {
        None => Ok(None),
        Some(name) => EngineSystem::from_name(name)
            .map(Some)
            .ok_or_else(|| RunError::Analyze {
                message: format!("unknown engine system {name:?}; expected criterion or callgrind"),
            }),
    }
}

/// Parses the `--since` option as an RFC 3339 timestamp or a bare `YYYY-MM-DD`
/// date (interpreted at UTC midnight), if set.
fn parse_since(value: Option<&str>) -> Result<Option<Timestamp>, RunError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if let Ok(timestamp) = value.parse::<Timestamp>() {
        return Ok(Some(timestamp));
    }
    if let Ok(date) = value.parse::<Date>() {
        // UTC has no DST transitions, so civil midnight always maps to an instant.
        let zoned = date
            .to_zoned(TimeZone::UTC)
            .expect("UTC midnight is always a valid instant");
        return Ok(Some(zoned.timestamp()));
    }
    Err(RunError::Analyze {
        message: format!(
            "invalid --since value {value:?}; expected an RFC 3339 timestamp or a YYYY-MM-DD date"
        ),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use futures::executor::block_on;
    use jiff::Timestamp;

    use crate::context::{CiInfo, GitInfo, RunContext, Timestamps, ToolchainInfo};
    use crate::model::{BenchmarkId, Metric, MetricKind, ResultRecord};
    use crate::storage::{MemoryStorage, Storage};

    use super::*;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).expect("seconds within range")
    }

    /// Builds a stored result set carrying one record with one `Ir` metric.
    fn ir_set(effective: i64, commit: &str, value: f64) -> ResultSet {
        let time = ts(effective);
        let context = RunContext::new(
            Timestamps::new(time, time, time),
            GitInfo {
                commit: Some(format!("{commit}full")),
                short_commit: Some(commit.to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            CiInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        let record = ResultRecord::new(
            BenchmarkId::new(
                Some("nm".to_owned()),
                "nm::observe".to_owned(),
                Some("pull".to_owned()),
                None,
            ),
            vec![Metric::new(
                "Ir".to_owned(),
                MetricKind::InstructionCount,
                value,
                Some("count".to_owned()),
            )],
        );
        ResultSet::new(context, vec![record])
    }

    fn callgrind_key(effective: i64, commit: &str, run: &str) -> String {
        format!(
            "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{effective}-{commit}-{run}.json"
        )
    }

    /// Stores a value at `key` in `storage`, panicking on failure (test helper).
    fn store(storage: &MemoryStorage, key: &str, set: &ResultSet) {
        let json = set.to_json().expect("result set serializes");
        block_on(storage.put(key, json.as_bytes())).expect("store succeeds");
    }

    fn options() -> AnalyzeOptions {
        AnalyzeOptions::default()
    }

    #[test]
    fn empty_history_reports_no_changes() {
        let storage = MemoryStorage::new();
        let outcome = block_on(analyze_with(&storage, "folo", &options())).expect("analysis runs");
        let RunOutcome::Analyzed {
            report,
            regressions,
            ..
        } = outcome
        else {
            panic!("expected an analyzed outcome");
        };
        assert_eq!(regressions, 0);
        assert!(report.contains("No notable changes detected."), "{report}");
    }

    #[test]
    fn regression_in_history_is_detected() {
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0, 130.0].into_iter().enumerate() {
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &callgrind_key(second, &format!("c{index}"), &format!("r{index}")),
                &ir_set(second, &format!("c{index}"), value),
            );
        }

        let outcome = block_on(analyze_with(&storage, "folo", &options())).expect("analysis runs");
        let RunOutcome::Analyzed {
            report,
            regressions,
            ..
        } = outcome
        else {
            panic!("expected an analyzed outcome");
        };
        assert_eq!(regressions, 1);
        assert!(report.contains("regression"), "{report}");
        assert!(report.contains("nm::observe/pull/Ir"), "{report}");
    }

    #[test]
    fn fail_on_regression_is_threaded_into_outcome() {
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0, 130.0].into_iter().enumerate() {
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &callgrind_key(second, &format!("c{index}"), &format!("r{index}")),
                &ir_set(second, &format!("c{index}"), value),
            );
        }

        let opts = AnalyzeOptions {
            fail_on_regression: true,
            ..AnalyzeOptions::default()
        };
        let outcome = block_on(analyze_with(&storage, "folo", &opts)).expect("analysis runs");
        assert!(!outcome.is_success(), "a gated regression must fail");
    }

    #[test]
    fn json_format_is_rendered() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &callgrind_key(1, "c1", "r1"),
            &ir_set(1, "c1", 10.0),
        );

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..AnalyzeOptions::default()
        };
        let outcome = block_on(analyze_with(&storage, "folo", &opts)).expect("analysis runs");
        let RunOutcome::Analyzed { report, .. } = outcome else {
            panic!("expected an analyzed outcome");
        };
        let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
        assert_eq!(parsed["project"], "folo");
        assert_eq!(parsed["runs"], 1);
    }

    #[test]
    fn system_filter_narrows_the_listing_prefix() {
        let storage = MemoryStorage::new();
        // One callgrind object and one (hypothetical) criterion object.
        store(
            &storage,
            &callgrind_key(1, "c1", "r1"),
            &ir_set(1, "c1", 10.0),
        );
        store(
            &storage,
            "v1/folo/criterion/x86_64-pc-windows-msvc/abc123/1-c1-r1.json",
            &ir_set(1, "c1", 10.0),
        );

        let opts = AnalyzeOptions {
            system: Some("callgrind".to_owned()),
            format: Some("json".to_owned()),
            ..AnalyzeOptions::default()
        };
        let outcome = block_on(analyze_with(&storage, "folo", &opts)).expect("analysis runs");
        let RunOutcome::Analyzed { report, .. } = outcome else {
            panic!("expected an analyzed outcome");
        };
        let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
        assert_eq!(parsed["runs"], 1, "only the callgrind object is loaded");
    }

    #[test]
    fn non_json_objects_are_skipped() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &callgrind_key(1, "c1", "r1"),
            &ir_set(1, "c1", 10.0),
        );
        // A stray non-result object under the prefix must be ignored, not parsed.
        block_on(storage.put("v1/folo/callgrind/README.txt", b"not json")).unwrap();

        let outcome = block_on(analyze_with(&storage, "folo", &options())).expect("analysis runs");
        let RunOutcome::Analyzed { .. } = outcome else {
            panic!("expected an analyzed outcome");
        };
    }

    #[test]
    fn malformed_stored_object_is_an_analyze_error() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&callgrind_key(1, "c1", "r1"), b"{ not valid")).unwrap();

        let error = block_on(analyze_with(&storage, "folo", &options())).unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn invalid_utf8_object_is_an_analyze_error() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&callgrind_key(1, "c1", "r1"), &[0xff, 0xfe])).unwrap();

        let error = block_on(analyze_with(&storage, "folo", &options())).unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn unknown_format_is_rejected() {
        let storage = MemoryStorage::new();
        let opts = AnalyzeOptions {
            format: Some("yaml".to_owned()),
            ..AnalyzeOptions::default()
        };
        let error = block_on(analyze_with(&storage, "folo", &opts)).unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn unknown_system_is_rejected() {
        let storage = MemoryStorage::new();
        let opts = AnalyzeOptions {
            system: Some("dhat".to_owned()),
            ..AnalyzeOptions::default()
        };
        let error = block_on(analyze_with(&storage, "folo", &opts)).unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn since_accepts_timestamp_and_date() {
        assert_eq!(
            parse_since(Some("2024-01-01T00:00:00Z")).unwrap(),
            Some("2024-01-01T00:00:00Z".parse().unwrap())
        );
        assert_eq!(
            parse_since(Some("2024-01-01")).unwrap(),
            Some("2024-01-01T00:00:00Z".parse().unwrap())
        );
        assert_eq!(parse_since(None).unwrap(), None);
    }

    #[test]
    fn since_rejects_garbage() {
        let error = parse_since(Some("not-a-date")).unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn metric_filter_limits_series() {
        let storage = MemoryStorage::new();
        let mut set = ir_set(1, "c1", 10.0);
        set.results[0].metrics.push(Metric::new(
            "EstimatedCycles".to_owned(),
            MetricKind::EstimatedCycles,
            20.0,
            Some("count".to_owned()),
        ));
        store(&storage, &callgrind_key(1, "c1", "r1"), &set);

        let opts = AnalyzeOptions {
            metric: Some("Ir".to_owned()),
            format: Some("json".to_owned()),
            ..AnalyzeOptions::default()
        };
        let outcome = block_on(analyze_with(&storage, "folo", &opts)).expect("analysis runs");
        let RunOutcome::Analyzed { report, .. } = outcome else {
            panic!("expected an analyzed outcome");
        };
        let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
        assert_eq!(parsed["series"], 1, "only the Ir metric forms a series");
    }
}
