//! Parsing of Gungraun's Callgrind `summary.json` (schema version 6) into the
//! engine-neutral [`ResultRecord`] model.
//!
//! Only the fields the tool needs are modelled; serde ignores the rest. The
//! committed fixtures under `tests/fixtures/callgrind/` are real Gungraun output
//! and act as a schema-drift canary: if Gungraun changes its format, parsing the
//! fixtures fails and the mismatch is caught immediately.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::Deserialize;

use crate::model::{BenchmarkId, Metric, MetricKind, ResultRecord};

/// The Gungraun summary schema version this parser understands.
const SUPPORTED_VERSION: &str = "6";

/// The unit recorded for Callgrind event counts.
const COUNT_UNIT: &str = "count";

/// An error encountered while parsing a Callgrind `summary.json`.
#[derive(Debug)]
pub(crate) enum CallgrindParseError {
    /// The text was not valid JSON or did not match the expected shape.
    Json(serde_json::Error),
    /// The summary declared a schema version the tool does not support.
    UnsupportedVersion(String),
}

impl fmt::Display for CallgrindParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "failed to parse Callgrind summary: {error}"),
            Self::UnsupportedVersion(version) => write!(
                f,
                "unsupported Gungraun summary schema version {version:?} \
                 (expected {SUPPORTED_VERSION:?})"
            ),
        }
    }
}

impl Error for CallgrindParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::UnsupportedVersion(_) => None,
        }
    }
}

/// Parses one Callgrind `summary.json` into a [`ResultRecord`].
///
/// # Errors
///
/// Returns [`CallgrindParseError`] if the JSON is malformed or declares an
/// unsupported schema version.
pub(crate) fn parse_callgrind_summary(json: &str) -> Result<ResultRecord, CallgrindParseError> {
    let summary = parse_summary(json)?;
    Ok(summary_to_record(&summary))
}

/// Deserializes and version-checks a summary without mapping it.
fn parse_summary(json: &str) -> Result<Summary, CallgrindParseError> {
    let summary: Summary = serde_json::from_str(json).map_err(CallgrindParseError::Json)?;
    if summary.version != SUPPORTED_VERSION {
        return Err(CallgrindParseError::UnsupportedVersion(summary.version));
    }
    Ok(summary)
}

/// Maps a parsed summary to a [`ResultRecord`] (pure).
fn summary_to_record(summary: &Summary) -> ResultRecord {
    let id = BenchmarkId::new(
        summary
            .package_dir
            .as_deref()
            .and_then(package_name_from_dir),
        summary.module_path.clone(),
        Some(summary.function_name.clone()),
        summary.id.clone(),
    );

    let mut metrics = Vec::new();
    for profile in &summary.profiles {
        let Some(events) = &profile.summaries.total.summary.callgrind else {
            continue;
        };
        for (event_kind, entry) in events {
            let Some(kind) = classify(event_kind) else {
                continue;
            };
            let Some(value) = entry.metrics.new_value() else {
                continue;
            };
            metrics.push(Metric::new(
                event_kind.clone(),
                kind,
                value.as_f64(),
                Some(COUNT_UNIT.to_owned()),
            ));
        }
    }

    ResultRecord::new(id, metrics)
}

/// Extracts the package name from a Gungraun `package_dir` path.
///
/// The directory is an absolute path whose final component is the package
/// directory name (for example `/mnt/c/Source/folo/packages/fast_time` ->
/// `fast_time`). Only the final component is used because the full path is
/// machine-specific (it differs between, say, a WSL guest and a CI runner) and
/// would break comparability across machines. Both `/` and `\` are treated as
/// separators so Windows-style paths resolve correctly, and trailing separators
/// are ignored. Returns `None` when no non-empty component remains.
fn package_name_from_dir(package_dir: &str) -> Option<String> {
    package_dir
        .rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
}

/// Gungraun cache event-kind names. Only L1 hits are higher-is-better: an access
/// served by L1 is the cheap outcome. Last-level and RAM hits are the expensive
/// tiers — an access that falls through to them is a cache miss escalating to
/// slower memory, so more of them is worse.
pub(crate) const L1_HITS_EVENT: &str = "L1hits";
pub(crate) const LL_HITS_EVENT: &str = "LLhits";
pub(crate) const RAM_HITS_EVENT: &str = "RamHits";

/// Maps a Callgrind event-kind name to the metric category the tool tracks.
///
/// Returns `None` for events that are not tracked (derived rates, raw cache
/// reads/writes, totals) so they are skipped rather than misclassified.
fn classify(event_kind: &str) -> Option<MetricKind> {
    match event_kind {
        "Ir" => Some(MetricKind::InstructionCount),
        "EstimatedCycles" => Some(MetricKind::EstimatedCycles),
        L1_HITS_EVENT | LL_HITS_EVENT | RAM_HITS_EVENT => Some(MetricKind::CacheEvents),
        "Bc" | "Bcm" | "Bi" | "Bim" => Some(MetricKind::Branches),
        _ => None,
    }
}

/// The subset of the Gungraun `BenchmarkSummary` the tool reads.
#[derive(Debug, Deserialize)]
struct Summary {
    version: String,
    module_path: String,
    function_name: String,
    id: Option<String>,
    #[serde(default)]
    package_dir: Option<String>,
    profiles: Vec<Profile>,
}

#[derive(Debug, Deserialize)]
struct Profile {
    summaries: ProfileData,
}

#[derive(Debug, Deserialize)]
struct ProfileData {
    total: ProfileTotal,
}

#[derive(Debug, Deserialize)]
struct ProfileTotal {
    summary: ToolSummaries,
}

/// The per-tool metric summaries Gungraun externally tags by tool name. Only the
/// Callgrind tool is read; any other tool's summary is ignored as an absent field.
#[derive(Debug, Deserialize)]
struct ToolSummaries {
    #[serde(rename = "Callgrind", default)]
    callgrind: Option<BTreeMap<String, MetricEntry>>,
}

#[derive(Debug, Deserialize)]
struct MetricEntry {
    metrics: MetricPair,
}

/// One metric as Gungraun serializes its `EitherOrBoth<MetricValue>`: the current
/// run's value under `Left`, a `[new, old]` pair under `Both`, or only a baseline
/// value under `Right` (modelled as both fields absent).
#[derive(Debug, Deserialize)]
struct MetricPair {
    #[serde(rename = "Left", default)]
    left: Option<MetricValue>,
    #[serde(rename = "Both", default)]
    both: Option<Vec<MetricValue>>,
}

impl MetricPair {
    /// The current-run value, if this metric has one (a `Right`-only metric — a
    /// value present only in a baseline — has none and is skipped).
    fn new_value(&self) -> Option<MetricValue> {
        if let Some(value) = self.left {
            return Some(value);
        }
        self.both.as_ref()?.first().copied()
    }
}

/// A single metric value, either an integer count or a floating-point rate.
#[derive(Clone, Copy, Debug, Deserialize)]
enum MetricValue {
    Int(u64),
    Float(f64),
}

impl MetricValue {
    /// The value as `f64`, the model's storage type.
    #[expect(
        clippy::cast_precision_loss,
        reason = "instruction counts well below 2^53; precision loss is irrelevant"
    )]
    fn as_f64(self) -> f64 {
        match self {
            Self::Int(value) => value as f64,
            Self::Float(value) => value,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "metric values are exact integer-derived counts"
    )]

    use super::*;

    const SINGLE_FIXTURE: &str =
        include_str!("../../tests/fixtures/callgrind/single_unparametrized.summary.json");
    const PARAMETRIZED_FIXTURE: &str =
        include_str!("../../tests/fixtures/callgrind/parametrized.summary.json");

    fn metric<'a>(record: &'a ResultRecord, name: &str) -> &'a Metric {
        record
            .metrics
            .iter()
            .find(|metric| metric.name == name)
            .unwrap_or_else(|| panic!("metric {name:?} should be present"))
    }

    #[test]
    fn parses_unparametrized_identity() {
        let record = parse_callgrind_summary(SINGLE_FIXTURE).unwrap();
        assert_eq!(
            record.id,
            BenchmarkId::new(
                Some("fast_time".to_owned()),
                "fast_time_timestamp_performance_cg::timestamp_capture::timestamp_capture_std_now"
                    .to_owned(),
                Some("timestamp_capture_std_now".to_owned()),
                None,
            )
        );
    }

    #[test]
    fn parses_package_from_package_dir() {
        let record = parse_callgrind_summary(SINGLE_FIXTURE).unwrap();
        assert_eq!(record.id.package.as_deref(), Some("fast_time"));
    }

    #[test]
    fn parses_parametrized_identity_with_value() {
        let record = parse_callgrind_summary(PARAMETRIZED_FIXTURE).unwrap();
        assert_eq!(record.id.value.as_deref(), Some("two_instants"));
        assert_eq!(
            record.id.case.as_deref(),
            Some("timestamp_capture_instant_saturating_duration_since")
        );
    }

    #[test]
    fn maps_the_tracked_metric_kinds() {
        let record = parse_callgrind_summary(SINGLE_FIXTURE).unwrap();

        assert_eq!(metric(&record, "Ir").kind, MetricKind::InstructionCount);
        assert_eq!(metric(&record, "Ir").value, 36.0);
        assert_eq!(
            metric(&record, "EstimatedCycles").kind,
            MetricKind::EstimatedCycles
        );
        assert_eq!(metric(&record, "EstimatedCycles").value, 193.0);
        assert_eq!(metric(&record, "L1hits").kind, MetricKind::CacheEvents);
        assert_eq!(metric(&record, "RamHits").kind, MetricKind::CacheEvents);
        assert_eq!(metric(&record, "Bc").kind, MetricKind::Branches);
        assert_eq!(metric(&record, "Bim").kind, MetricKind::Branches);
    }

    #[test]
    fn skips_untracked_events() {
        let record = parse_callgrind_summary(SINGLE_FIXTURE).unwrap();
        let names: Vec<&str> = record.metrics.iter().map(|m| m.name.as_str()).collect();

        assert!(!names.contains(&"Dr"), "raw cache reads should be skipped");
        assert!(
            !names.contains(&"L1HitRate"),
            "derived rates should be skipped"
        );
        assert!(
            !names.contains(&"TotalRW"),
            "totals should be skipped: {names:?}"
        );
    }

    #[test]
    fn tracks_exactly_the_canonical_event_set() {
        let record = parse_callgrind_summary(SINGLE_FIXTURE).unwrap();
        let mut names: Vec<&str> = record.metrics.iter().map(|m| m.name.as_str()).collect();
        names.sort_unstable();

        assert_eq!(
            names,
            vec![
                "Bc",
                "Bcm",
                "Bi",
                "Bim",
                "EstimatedCycles",
                "Ir",
                "L1hits",
                "LLhits",
                "RamHits",
            ]
        );
    }

    #[test]
    fn rejects_unsupported_version() {
        let altered = SINGLE_FIXTURE.replace("\"version\": \"6\"", "\"version\": \"7\"");
        let error = parse_callgrind_summary(&altered).unwrap_err();
        match error {
            CallgrindParseError::UnsupportedVersion(version) => assert_eq!(version, "7"),
            CallgrindParseError::Json(error) => panic!("unexpected json error: {error}"),
        }
    }

    #[test]
    fn rejects_malformed_json() {
        let error = parse_callgrind_summary("{ not json").unwrap_err();
        assert!(matches!(error, CallgrindParseError::Json(_)));
    }

    #[test]
    fn error_display_and_source() {
        let json_error = parse_callgrind_summary("{ not json").unwrap_err();
        assert!(
            json_error.to_string().contains("failed to parse Callgrind"),
            "{json_error}"
        );
        assert!(json_error.source().is_some());

        let version_error = CallgrindParseError::UnsupportedVersion("9".to_owned());
        assert!(
            version_error.to_string().contains("\"9\""),
            "{version_error}"
        );
        assert!(version_error.source().is_none());
    }

    fn summary_json(callgrind_body: &str) -> String {
        format!(
            "{{\"version\":\"6\",\"module_path\":\"m\",\"function_name\":\"f\",\
             \"profiles\":[{{\"summaries\":{{\"total\":{{\"summary\":{callgrind_body}}}}}}}]}}"
        )
    }

    fn summary_with_package_dir(package_dir: &str) -> String {
        format!(
            "{{\"version\":\"6\",\"module_path\":\"a::bench\",\"function_name\":\"f\",\
             \"package_dir\":\"{package_dir}\",\
             \"profiles\":[{{\"summaries\":{{\"total\":{{\"summary\":{{}}}}}}}}]}}"
        )
    }

    #[test]
    fn package_name_from_dir_extracts_final_component() {
        assert_eq!(
            package_name_from_dir("/mnt/c/Source/folo/packages/fast_time"),
            Some("fast_time".to_owned())
        );
        assert_eq!(package_name_from_dir("/a/b/pkg/"), Some("pkg".to_owned()));
        assert_eq!(package_name_from_dir(r"C:\x\pkg"), Some("pkg".to_owned()));
        assert_eq!(package_name_from_dir("/a\\b\\pkg"), Some("pkg".to_owned()));
        assert_eq!(package_name_from_dir("pkg"), Some("pkg".to_owned()));
        assert_eq!(package_name_from_dir(""), None);
        assert_eq!(package_name_from_dir("/"), None);
    }

    #[test]
    fn summary_without_package_dir_has_no_package() {
        let record = parse_callgrind_summary(&summary_json("{}")).unwrap();
        assert_eq!(record.id.package, None);
    }

    #[test]
    fn same_module_path_in_different_packages_yields_distinct_ids() {
        let foo = parse_callgrind_summary(&summary_with_package_dir("/work/packages/foo")).unwrap();
        let bar = parse_callgrind_summary(&summary_with_package_dir("/work/packages/bar")).unwrap();

        assert_eq!(foo.id.group, bar.id.group);
        assert_eq!(foo.id.case, bar.id.case);
        assert_ne!(foo.id, bar.id);
        assert_eq!(foo.id.package.as_deref(), Some("foo"));
        assert_eq!(bar.id.package.as_deref(), Some("bar"));
    }

    #[test]
    fn skips_profile_without_callgrind_summary() {
        let record = parse_callgrind_summary(&summary_json("{}")).unwrap();
        assert!(record.metrics.is_empty());
    }

    #[test]
    fn skips_metric_present_only_in_baseline() {
        let body = "{\"Callgrind\":{\"Ir\":{\"metrics\":{}}}}";
        let record = parse_callgrind_summary(&summary_json(body)).unwrap();
        assert!(record.metrics.is_empty());
    }

    #[test]
    fn reads_new_value_from_both_pair() {
        let body = "{\"Callgrind\":{\"Ir\":{\"metrics\":{\"Both\":[{\"Int\":10},{\"Int\":9}]}}}}";
        let record = parse_callgrind_summary(&summary_json(body)).unwrap();
        assert_eq!(metric(&record, "Ir").value, 10.0);
    }

    #[test]
    fn reads_float_metric_value() {
        let body = "{\"Callgrind\":{\"Ir\":{\"metrics\":{\"Left\":{\"Float\":1.5}}}}}";
        let record = parse_callgrind_summary(&summary_json(body)).unwrap();
        assert_eq!(metric(&record, "Ir").value, 1.5);
    }
}
