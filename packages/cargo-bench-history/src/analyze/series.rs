//! The series engine: reconstruct per-benchmark, per-metric time series from the
//! stored result sets so the finding algorithms can reason over them.
//!
//! A series is identified by the comparable [`Location`] its runs share (parsed
//! from the storage key) together with the [`BenchmarkId`] and metric name. Its
//! points are ordered by *effective* time (never ingest time), with the storage
//! key as a deterministic tie-break so equal-timestamp backfills sort stably.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::Serialize;

use crate::model::{BenchmarkId, MetricKind, ResultSet};

/// The comparable location a series belongs to, parsed from its storage key.
///
/// Within a single project all runs that share a location are comparable; runs
/// in different locations (a different engine system, target triple, or machine
/// key) never share a series.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct Location {
    /// Engine system identifier (for example, `callgrind`).
    pub(crate) system: String,
    /// Resolved target triple the run was recorded under.
    pub(crate) target_triple: String,
    /// Machine partition (`synthetic` for hardware-independent engines).
    pub(crate) machine: String,
}

/// A single observation in a series.
#[derive(Clone, Debug)]
pub(crate) struct SeriesPoint {
    /// Timeline position of this observation.
    pub(crate) effective: Timestamp,
    /// Storage key the observation came from (tie-break and provenance).
    pub(crate) object_key: String,
    /// Abbreviated commit the run was measured against, if known.
    pub(crate) commit: Option<String>,
    /// The measured value.
    pub(crate) value: f64,
}

/// A per-`(location, benchmark, metric)` time series ordered by effective time.
#[derive(Clone, Debug)]
pub(crate) struct Series {
    /// The comparable location all points share.
    pub(crate) location: Location,
    /// The benchmark identity all points share.
    pub(crate) id: BenchmarkId,
    /// The metric name all points share.
    pub(crate) metric: String,
    /// The metric category (governs comparison semantics).
    pub(crate) kind: MetricKind,
    /// Observations ordered by `(effective, object_key)`.
    pub(crate) points: Vec<SeriesPoint>,
}

/// Filters applied while building series from stored runs.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SeriesFilter<'a> {
    /// Keep only metrics with this exact name, if set.
    pub(crate) metric: Option<&'a str>,
}

/// Parses the comparable [`Location`] from a storage object key.
///
/// Keys have the form `v1/{project}/{system}/{triple}/{machine}/{file}` — exactly
/// six non-empty segments. Any key that does not match that shape exactly (wrong
/// version, too few or too many segments, or an empty segment) is ignored (returns
/// `None`) rather than misattributed to a series.
pub(crate) fn location_from_key(key: &str) -> Option<Location> {
    let parts: Vec<&str> = key.split('/').collect();
    if parts.len() != 6 {
        return None;
    }
    if *parts.first()? != "v1" {
        return None;
    }
    let project = *parts.get(1)?;
    let system = *parts.get(2)?;
    let triple = *parts.get(3)?;
    let machine = *parts.get(4)?;
    let file = *parts.get(5)?;
    if project.is_empty()
        || system.is_empty()
        || triple.is_empty()
        || machine.is_empty()
        || file.is_empty()
    {
        return None;
    }
    Some(Location {
        system: system.to_owned(),
        target_triple: triple.to_owned(),
        machine: machine.to_owned(),
    })
}

/// Reconstructs every series from the loaded `(object key, result set)` pairs.
///
/// Each metric of each record becomes one point in the series for its
/// `(location, benchmark id, metric name)`. The result is sorted deterministically
/// (by series identity, then points by effective time and storage key) so reports
/// and findings do not depend on storage-listing order.
pub(crate) fn build_series(
    objects: &[(String, ResultSet)],
    filter: &SeriesFilter<'_>,
) -> Vec<Series> {
    let mut groups: BTreeMap<(Location, BenchmarkId, String), (MetricKind, Vec<SeriesPoint>)> =
        BTreeMap::new();

    for (key, set) in objects {
        let Some(location) = location_from_key(key) else {
            continue;
        };
        let effective = set.context.timestamps.effective;
        let commit = set.context.git.short_commit.clone();

        for record in &set.results {
            for metric in &record.metrics {
                if filter.metric.is_some_and(|want| metric.name != want) {
                    continue;
                }
                let point = SeriesPoint {
                    effective,
                    object_key: key.clone(),
                    commit: commit.clone(),
                    value: metric.value,
                };
                groups
                    .entry((location.clone(), record.id.clone(), metric.name.clone()))
                    .or_insert_with(|| (metric.kind, Vec::new()))
                    .1
                    .push(point);
            }
        }
    }

    groups
        .into_iter()
        .map(|((location, id, metric), (kind, mut points))| {
            points.sort_by(|left, right| {
                left.effective
                    .cmp(&right.effective)
                    .then_with(|| left.object_key.cmp(&right.object_key))
            });
            Series {
                location,
                id,
                metric,
                kind,
                points,
            }
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "metric values are exact integer-derived counts"
    )]
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use crate::comparability::{ComparabilityKey, EngineSystem};
    use crate::context::{CiInfo, GitInfo, RunContext, Timestamps, ToolchainInfo};
    use crate::model::{Metric, ResultRecord};

    use super::*;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).expect("seconds within range")
    }

    /// Builds a stored result set with one record carrying one `Ir` metric.
    fn result_set(effective: Timestamp, commit: &str, value: f64) -> ResultSet {
        result_set_for_package(effective, commit, value, None)
    }

    /// Builds a stored result set whose single record is scoped to `package`,
    /// keeping every other identity component fixed.
    fn result_set_for_package(
        effective: Timestamp,
        commit: &str,
        value: f64,
        package: Option<&str>,
    ) -> ResultSet {
        let context = RunContext::new(
            Timestamps::new(effective, effective, effective),
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
                package.map(ToOwned::to_owned),
                "group".to_owned(),
                Some("case".to_owned()),
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

    fn key_for(effective: i64, commit: &str, run: &str) -> String {
        format!(
            "v1/proj/callgrind/x86_64-unknown-linux-gnu/synthetic/{effective}-{commit}-{run}.json"
        )
    }

    #[test]
    fn location_parses_components() {
        let location =
            location_from_key("v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/1-a-b.json")
                .expect("a well-formed key should parse");
        assert_eq!(location.system, "callgrind");
        assert_eq!(location.target_triple, "x86_64-unknown-linux-gnu");
        assert_eq!(location.machine, "synthetic");
    }

    #[test]
    fn location_rejects_malformed_keys() {
        assert!(location_from_key("v2/folo/callgrind/t/m/f.json").is_none());
        assert!(location_from_key("v1/folo/callgrind").is_none());
        assert!(location_from_key("v1/folo/callgrind/t/m/").is_none());
        // A single empty segment in any position is rejected, never misattributed.
        assert!(location_from_key("v1//callgrind/t/m/f.json").is_none());
        assert!(location_from_key("v1/folo//t/m/f.json").is_none());
        assert!(location_from_key("v1/folo/callgrind//m/f.json").is_none());
        assert!(location_from_key("v1/folo/callgrind/t//f.json").is_none());
        // A deeper key (more than six segments) is rejected rather than treating
        // an interior directory as the file segment and misattributing the run.
        assert!(location_from_key("v1/folo/callgrind/t/m/sub/f.json").is_none());
    }

    #[test]
    fn build_series_orders_points_by_effective_time() {
        // Insert out of order; expect ascending effective ordering on output.
        let objects = vec![
            (key_for(300, "ccc", "r3"), result_set(ts(300), "ccc", 30.0)),
            (key_for(100, "aaa", "r1"), result_set(ts(100), "aaa", 10.0)),
            (key_for(200, "bbb", "r2"), result_set(ts(200), "bbb", 20.0)),
        ];

        let series = build_series(&objects, &SeriesFilter::default());
        assert_eq!(series.len(), 1);
        let values: Vec<f64> = series[0].points.iter().map(|point| point.value).collect();
        assert_eq!(values, vec![10.0, 20.0, 30.0]);
    }

    #[test]
    fn build_series_breaks_effective_ties_by_object_key() {
        // Two runs at the same effective second; the storage key (which embeds the
        // run id) is the deterministic tie-break, so `r1` precedes `r2`.
        let objects = vec![
            (key_for(100, "aaa", "r2"), result_set(ts(100), "aaa", 22.0)),
            (key_for(100, "aaa", "r1"), result_set(ts(100), "aaa", 11.0)),
        ];

        let series = build_series(&objects, &SeriesFilter::default());
        let values: Vec<f64> = series[0].points.iter().map(|point| point.value).collect();
        assert_eq!(values, vec![11.0, 22.0]);
    }

    #[test]
    fn build_series_separates_locations() {
        // A second run in a different triple is a different (incomparable) series.
        let other_key =
            "v1/proj/callgrind/aarch64-unknown-linux-gnu/synthetic/200-bbb-r2.json".to_owned();
        let objects = vec![
            (key_for(100, "aaa", "r1"), result_set(ts(100), "aaa", 10.0)),
            (other_key, result_set(ts(200), "bbb", 20.0)),
        ];

        let series = build_series(&objects, &SeriesFilter::default());
        assert_eq!(series.len(), 2, "different triples are different series");
    }

    #[test]
    fn build_series_separates_packages() {
        // Two records share group/case/metric and location but belong to different
        // packages, so they must form two series rather than silently merging.
        let objects = vec![
            (
                key_for(100, "aaa", "r1"),
                result_set_for_package(ts(100), "aaa", 10.0, Some("foo")),
            ),
            (
                key_for(200, "bbb", "r2"),
                result_set_for_package(ts(200), "bbb", 20.0, Some("bar")),
            ),
        ];

        let series = build_series(&objects, &SeriesFilter::default());
        assert_eq!(series.len(), 2, "different packages are different series");
    }

    #[test]
    fn build_series_attributes_a_key_with_a_sanitized_project() {
        // A project containing a stray separator is mangled into a single segment
        // when the key is built, so the resulting key still parses and the run is
        // attributed (rather than silently dropped by `location_from_key`).
        let key = ComparabilityKey::new(
            "team/app",
            EngineSystem::Callgrind,
            "x86_64-unknown-linux-gnu",
            None,
        )
        .object_key(100, "aaa", "r1");
        assert!(
            location_from_key(&key).is_some(),
            "sanitized key should remain attributable: {key}"
        );

        let objects = vec![(key, result_set(ts(100), "aaa", 10.0))];
        assert_eq!(build_series(&objects, &SeriesFilter::default()).len(), 1);
    }

    #[test]
    fn build_series_applies_metric_filter() {
        let mut set = result_set(ts(100), "aaa", 10.0);
        set.results[0].metrics.push(Metric::new(
            "EstimatedCycles".to_owned(),
            MetricKind::EstimatedCycles,
            99.0,
            Some("count".to_owned()),
        ));
        let objects = vec![(key_for(100, "aaa", "r1"), set)];

        let filter = SeriesFilter { metric: Some("Ir") };
        let series = build_series(&objects, &filter);
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].metric, "Ir");
    }

    #[test]
    fn build_series_skips_unparseable_keys() {
        let objects = vec![(
            "not-a-valid-key.json".to_owned(),
            result_set(ts(100), "aaa", 10.0),
        )];
        assert!(build_series(&objects, &SeriesFilter::default()).is_empty());
    }
}
