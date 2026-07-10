//! Combining several runs of the same suite into one best-of-N result set.
//!
//! The `--best-of N` collection mode runs the whole benchmark suite `N` times and
//! reduces the runs to a single stored result set by keeping, per metric, the
//! **minimum** observed value. Benchmark interference is one-sided — a contended
//! CI runner only ever makes a benchmark *slower* — so the minimum is the sample
//! least perturbed by transient noise. The winning [`Metric`] is carried over
//! wholesale (its own dispersion travels with it), so a stored result may blend
//! metrics selected from different physical runs.
//!
//! The reduction is a pure function over the stored [`BenchmarkResult`] shape, so
//! it lives here with the data model where it is cheap to mutation-test in
//! isolation.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use crate::{BenchmarkId, BenchmarkResult, MetricKind, MetricList};

/// The best-of-N reduction: combined results plus per-metric provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct Combined {
    /// One result per benchmark case, each metric the minimum across the runs.
    pub results: Vec<BenchmarkResult>,
    /// Which run each stored metric was selected from, for verbose diagnostics.
    ///
    /// One entry per stored metric, in the same order the metrics appear in
    /// [`results`](Self::results).
    pub selections: Vec<Selection>,
}

/// Provenance of one selected metric: the samples seen and which run won.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    /// The benchmark case the metric belongs to.
    pub id: BenchmarkId,
    /// The metric kind that was reduced.
    pub kind: MetricKind,
    /// The value observed for this metric in each run, in run order.
    pub samples: Vec<f64>,
    /// Zero-based index of the run whose value was kept (the minimum; ties resolve
    /// to the earliest run).
    pub chosen_run: usize,
}

/// A cross-run inconsistency that makes a best-of-N reduction ill-defined.
///
/// Every run must measure the same set of cases and the same metrics per case, so
/// that each metric has exactly one sample per run to minimize over. A missing or
/// extra case or metric in any run is a hard error rather than something to paper
/// over, because it means the runs did not exercise the same work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AggregateError {
    /// A benchmark case is present in some runs but absent from `run_index`.
    MissingCase {
        /// The case that is not measured uniformly across the runs.
        id: BenchmarkId,
        /// Zero-based index of the run the case is missing from.
        run_index: usize,
    },
    /// A metric of `kind` for `id` is present in some runs but absent from
    /// `run_index`.
    MissingMetric {
        /// The case whose metrics differ across the runs.
        id: BenchmarkId,
        /// The metric kind that is not reported uniformly across the runs.
        kind: MetricKind,
        /// Zero-based index of the run the metric is missing from.
        run_index: usize,
    },
}

impl fmt::Display for AggregateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCase { id, run_index } => write!(
                f,
                "benchmark case '{id}' is missing from run {}; every best-of run must \
                 measure the same set of cases",
                run_index.saturating_add(1)
            ),
            Self::MissingMetric {
                id,
                kind,
                run_index,
            } => write!(
                f,
                "metric '{}' for benchmark case '{id}' is missing from run {}; every \
                 best-of run must report the same metrics per case",
                kind.as_str(),
                run_index.saturating_add(1)
            ),
        }
    }
}

impl Error for AggregateError {}

/// Reduces `runs` to a single result set, keeping the minimum value per metric.
///
/// Each element of `runs` is one run's harvested results; the slice therefore has
/// one entry per benchmark repetition. Every run must measure the same cases and
/// the same metrics per case (see [`AggregateError`]); given that, each metric is
/// reduced to its minimum across the runs, with ties resolved to the earliest run.
/// The winning [`Metric`](crate::Metric) is kept wholesale, so its dispersion
/// travels with its value. Case order and per-case metric order follow the first
/// run, so `N == 1` reproduces that run's results unchanged.
///
/// An empty slice yields an empty result set: with no runs there is nothing to
/// reduce and no reference to check consistency against.
///
/// # Errors
///
/// Returns [`AggregateError`] if any run measures a different set of cases, or a
/// different set of metrics for a shared case, than the first run.
pub fn min_per_metric(runs: &[Vec<BenchmarkResult>]) -> Result<Combined, AggregateError> {
    let Some((reference, rest)) = runs.split_first() else {
        return Ok(Combined {
            results: Vec::new(),
            selections: Vec::new(),
        });
    };

    // Index every run by case identity so cross-run lookups are direct. The
    // reference run also fixes the output order of cases and, per case, of metrics.
    let indexed: Vec<HashMap<&BenchmarkId, &BenchmarkResult>> = runs
        .iter()
        .map(|results| results.iter().map(|result| (&result.id, result)).collect())
        .collect();

    check_case_consistency(reference, rest, &indexed)?;
    check_metric_consistency(reference, &indexed)?;

    let mut results = Vec::with_capacity(reference.len());
    let mut selections = Vec::new();
    for reference_result in reference {
        let id = &reference_result.id;
        let mut metrics = MetricList::new();
        for reference_metric in &reference_result.metrics {
            let kind = reference_metric.kind;

            // Consistency is already verified, so every run has this metric. Track
            // the smallest sample and the run it came from in one pass, updating
            // only on a strictly smaller value so ties resolve to the earliest run.
            let mut samples = Vec::with_capacity(indexed.len());
            let mut winner: Option<&crate::Metric> = None;
            let mut chosen_run = 0_usize;
            for (run_index, run) in indexed.iter().enumerate() {
                let metric = run
                    .get(id)
                    .and_then(|result| find_metric(result, kind))
                    .expect("consistency check guarantees this metric is in every run");
                let is_better = match winner {
                    Some(best) => metric.value < best.value,
                    None => true,
                };
                if is_better {
                    winner = Some(metric);
                    chosen_run = run_index;
                }
                samples.push(metric.value);
            }
            let winner = winner
                .expect("a reduced metric is measured in at least one run")
                .clone();
            metrics.push(winner);
            selections.push(Selection {
                id: id.clone(),
                kind,
                samples,
                chosen_run,
            });
        }
        results.push(BenchmarkResult {
            id: id.clone(),
            metrics,
        });
    }

    Ok(Combined {
        results,
        selections,
    })
}

/// Verifies every run measures exactly the reference run's set of cases.
fn check_case_consistency(
    reference: &[BenchmarkResult],
    rest: &[Vec<BenchmarkResult>],
    indexed: &[HashMap<&BenchmarkId, &BenchmarkResult>],
) -> Result<(), AggregateError> {
    // The reference run's own lookup fixes what every later run must contain; the
    // remaining lookups line up one-to-one with `rest` (run 1 onward).
    let Some((reference_lookup, rest_lookups)) = indexed.split_first() else {
        return Ok(());
    };
    for (offset, (run, lookup)) in rest.iter().zip(rest_lookups).enumerate() {
        // `rest` starts at run 1, so its zero-based index is the offset plus one.
        let run_index = offset.saturating_add(1);
        for reference_result in reference {
            if !lookup.contains_key(&reference_result.id) {
                return Err(AggregateError::MissingCase {
                    id: reference_result.id.clone(),
                    run_index,
                });
            }
        }
        // A case in this run but not the reference is equally inconsistent: report
        // it as missing from the reference run (index zero).
        for result in run {
            if !reference_lookup.contains_key(&result.id) {
                return Err(AggregateError::MissingCase {
                    id: result.id.clone(),
                    run_index: 0,
                });
            }
        }
    }
    Ok(())
}

/// Verifies every run reports the reference run's metrics for each shared case.
///
/// Case consistency is assumed already checked, so every run holds each reference
/// case.
fn check_metric_consistency(
    reference: &[BenchmarkResult],
    indexed: &[HashMap<&BenchmarkId, &BenchmarkResult>],
) -> Result<(), AggregateError> {
    for (run_index, lookup) in indexed.iter().enumerate() {
        for reference_result in reference {
            let id = &reference_result.id;
            let result = lookup
                .get(id)
                .expect("case consistency guarantees every run holds this case");
            for reference_metric in &reference_result.metrics {
                if find_metric(result, reference_metric.kind).is_none() {
                    return Err(AggregateError::MissingMetric {
                        id: id.clone(),
                        kind: reference_metric.kind,
                        run_index,
                    });
                }
            }
            // A metric in this run but not the reference is equally inconsistent:
            // report it as missing from the reference run (index zero).
            for metric in &result.metrics {
                if find_metric(reference_result, metric.kind).is_none() {
                    return Err(AggregateError::MissingMetric {
                        id: id.clone(),
                        kind: metric.kind,
                        run_index: 0,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Finds the metric of `kind` in `result`, or `None` when it carries no such kind.
fn find_metric(result: &BenchmarkResult, kind: MetricKind) -> Option<&crate::Metric> {
    result.metrics.iter().find(|metric| metric.kind == kind)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
    #![allow(
        clippy::float_cmp,
        reason = "aggregated values are exact copies of the inputs, not computed"
    )]

    use nonempty::nonempty;

    use super::*;
    use crate::Metric;

    fn id(name: &str) -> BenchmarkId {
        BenchmarkId::new(nonempty![name.to_owned()])
    }

    fn result(name: &str, metrics: Vec<Metric>) -> BenchmarkResult {
        BenchmarkResult::new(id(name), metrics)
    }

    fn wall(value: f64) -> Metric {
        Metric::new(MetricKind::WallTime, value)
    }

    #[test]
    fn empty_input_yields_empty_output() {
        let combined = min_per_metric(&[]).unwrap();
        assert!(combined.results.is_empty());
        assert!(combined.selections.is_empty());
    }

    #[test]
    fn single_run_is_returned_unchanged() {
        // N == 1 must reproduce the run byte-for-byte, dispersion included.
        let metric = wall(7.4).with_dispersion(Some(0.5), Some(7.0), Some(7.9));
        let runs = vec![vec![result("case", vec![metric.clone()])]];

        let combined = min_per_metric(&runs).unwrap();

        assert_eq!(combined.results, runs[0]);
        assert_eq!(combined.results[0].metrics[0], metric);
        assert_eq!(combined.selections.len(), 1);
        assert_eq!(combined.selections[0].chosen_run, 0);
        assert_eq!(combined.selections[0].samples, vec![7.4]);
    }

    #[test]
    fn minimum_value_is_selected_with_its_own_dispersion() {
        // The smallest value wins and drags along its own dispersion, not the
        // reference run's.
        let low = wall(5.0).with_dispersion(Some(0.1), Some(4.9), Some(5.1));
        let runs = vec![
            vec![result("case", vec![wall(9.0)])],
            vec![result("case", vec![low.clone()])],
            vec![result("case", vec![wall(7.0)])],
        ];

        let combined = min_per_metric(&runs).unwrap();

        assert_eq!(combined.results.len(), 1);
        assert_eq!(combined.results[0].metrics[0], low);
        let selection = &combined.selections[0];
        assert_eq!(selection.samples, vec![9.0, 5.0, 7.0]);
        assert_eq!(selection.chosen_run, 1);
    }

    #[test]
    fn each_metric_is_minimized_independently() {
        // A result's two metrics can take their minima from different runs.
        let runs = vec![
            vec![result(
                "case",
                vec![wall(5.0), Metric::new(MetricKind::ProcessorTime, 20.0)],
            )],
            vec![result(
                "case",
                vec![wall(8.0), Metric::new(MetricKind::ProcessorTime, 11.0)],
            )],
        ];

        let combined = min_per_metric(&runs).unwrap();

        let metrics = &combined.results[0].metrics;
        assert_eq!(metrics[0].value, 5.0);
        assert_eq!(metrics[1].value, 11.0);
        assert_eq!(combined.selections[0].chosen_run, 0);
        assert_eq!(combined.selections[1].chosen_run, 1);
    }

    #[test]
    fn ties_resolve_to_the_earliest_run() {
        let runs = vec![
            vec![result("case", vec![wall(5.0)])],
            vec![result("case", vec![wall(5.0)])],
        ];

        let combined = min_per_metric(&runs).unwrap();

        assert_eq!(combined.selections[0].chosen_run, 0);
    }

    #[test]
    fn case_and_metric_order_follow_the_first_run() {
        let runs = vec![
            vec![
                result("beta", vec![wall(2.0)]),
                result("alpha", vec![wall(1.0)]),
            ],
            vec![
                result("alpha", vec![wall(1.0)]),
                result("beta", vec![wall(2.0)]),
            ],
        ];

        let combined = min_per_metric(&runs).unwrap();

        let names: Vec<String> = combined
            .results
            .iter()
            .map(|result| result.id.qualified())
            .collect();
        assert_eq!(names, vec!["beta".to_owned(), "alpha".to_owned()]);
    }

    #[test]
    fn a_case_missing_from_a_later_run_is_an_error() {
        let runs = vec![
            vec![result("a", vec![wall(1.0)]), result("b", vec![wall(1.0)])],
            vec![result("a", vec![wall(1.0)])],
        ];

        let error = min_per_metric(&runs).unwrap_err();

        match error {
            AggregateError::MissingCase { id, run_index } => {
                assert_eq!(id.qualified(), "b");
                assert_eq!(run_index, 1);
            }
            other => panic!("expected a missing-case error, got {other:?}"),
        }
    }

    #[test]
    fn a_case_only_in_a_later_run_is_an_error() {
        // An extra case in a later run is reported as missing from the first run.
        let runs = vec![
            vec![result("a", vec![wall(1.0)])],
            vec![result("a", vec![wall(1.0)]), result("b", vec![wall(1.0)])],
        ];

        let error = min_per_metric(&runs).unwrap_err();

        match error {
            AggregateError::MissingCase { id, run_index } => {
                assert_eq!(id.qualified(), "b");
                assert_eq!(run_index, 0);
            }
            other => panic!("expected a missing-case error, got {other:?}"),
        }
    }

    #[test]
    fn a_metric_missing_from_a_later_run_is_an_error() {
        let runs = vec![
            vec![result(
                "case",
                vec![wall(1.0), Metric::new(MetricKind::ProcessorTime, 2.0)],
            )],
            vec![result("case", vec![wall(1.0)])],
        ];

        let error = min_per_metric(&runs).unwrap_err();

        match error {
            AggregateError::MissingMetric {
                id,
                kind,
                run_index,
            } => {
                assert_eq!(id.qualified(), "case");
                assert_eq!(kind, MetricKind::ProcessorTime);
                assert_eq!(run_index, 1);
            }
            other => panic!("expected a missing-metric error, got {other:?}"),
        }
    }

    #[test]
    fn an_extra_metric_in_a_later_run_is_an_error() {
        let runs = vec![
            vec![result("case", vec![wall(1.0)])],
            vec![result(
                "case",
                vec![wall(1.0), Metric::new(MetricKind::ProcessorTime, 2.0)],
            )],
        ];

        let error = min_per_metric(&runs).unwrap_err();

        match error {
            AggregateError::MissingMetric {
                kind, run_index, ..
            } => {
                assert_eq!(kind, MetricKind::ProcessorTime);
                assert_eq!(run_index, 0);
            }
            other => panic!("expected a missing-metric error, got {other:?}"),
        }
    }

    #[test]
    fn error_messages_name_the_case_and_run() {
        let missing_case = AggregateError::MissingCase {
            id: id("some/case"),
            run_index: 2,
        };
        let text = missing_case.to_string();
        assert!(text.contains("some/case"), "{text}");
        assert!(text.contains("run 3"), "{text}");

        let missing_metric = AggregateError::MissingMetric {
            id: id("some/case"),
            kind: MetricKind::WallTime,
            run_index: 0,
        };
        let text = missing_metric.to_string();
        assert!(text.contains("wall_time"), "{text}");
        assert!(text.contains("run 1"), "{text}");
    }
}
