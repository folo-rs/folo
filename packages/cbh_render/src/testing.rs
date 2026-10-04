//! Preconstructed reports shared by the in-workspace rendering benchmarks.

#![cfg_attr(coverage_nightly, coverage(off))]

use cbh_detect::{
    AnalysisMode, Direction, Finding, FindingMethod, SeriesCensus, SeriesValue, Testability,
};
use cbh_model::{BenchmarkId, DiscriminantSet, Engine, MetricKind};
use nonempty::nonempty;
use serde_json::{Value, from_str, json};

use crate::{
    DEFAULT_SUMMARY_LIMIT, ReportFormat, ReportInput, SetSummary, chart_series, render,
    render_markdown_summary,
};

/// Owns bounded finding and partition data, without running detection.
///
/// Summaries and the borrowing report input are assembled before measurement. The
/// production renderer remains the measured operation; see docs/implementation.md.
#[derive(Debug)]
pub struct ReportFixture {
    sets: [DiscriminantSet; 2],
    findings: Vec<Finding>,
}

impl ReportFixture {
    /// Builds a globally ranked history report with alternating machine partitions.
    ///
    /// # Panics
    ///
    /// `finding_count` must not exceed [`HIGH_FINDINGS`].
    #[must_use]
    pub fn new(finding_count: usize) -> Self {
        assert!(finding_count <= HIGH_FINDINGS);
        // Keep partition count fixed so only finding count drives the scaling comparison.
        let sets = ["machine_a", "machine_b"].map(|machine_key| DiscriminantSet {
            engine: Engine::Callgrind,
            target_triple: "x86_64-unknown-linux-gnu".into(),
            machine_key: machine_key.into(),
        });
        let findings: Vec<_> = (0..finding_count)
            .zip(sets.iter().cycle())
            .map(|(index, set)| finding(index, set))
            .collect();
        assert_eq!(findings.len(), finding_count);
        Self { sets, findings }
    }

    /// Builds the per-partition references outside the measured operation.
    #[must_use]
    pub fn summaries(&self) -> Vec<SetSummary<'_>> {
        self.sets
            .iter()
            .map(|set| SetSummary {
                set,
                runs: CHART_POINTS,
                series: self.series_per_set(),
                findings: self
                    .findings
                    .iter()
                    .filter(|finding| finding.set == *set)
                    .collect(),
                comparison_base_lags: Vec::new(),
                branch_comparison: None,
            })
            .collect()
    }

    /// Builds report metadata outside the measured operation.
    #[must_use]
    pub fn input<'a>(&'a self, summaries: &'a [SetSummary<'a>]) -> ReportInput<'a> {
        let series = self.series_per_set().saturating_mul(self.sets.len());
        let mut census = SeriesCensus::default();
        for _ in 0..series {
            census.record(Testability::Judged);
        }
        ReportInput {
            project: "render_benchmarks",
            tip_commit: LAST_COMMIT,
            tip_dirty: false,
            mode: AnalysisMode::History,
            runs: CHART_POINTS.saturating_mul(self.sets.len()),
            series,
            commit_span: Some((FIRST_COMMIT, LAST_COMMIT)),
            report_improvements: false,
            findings: &self.findings,
            sets: summaries,
            hint: None,
            warning: None,
            ghosts_excluded: 0,
            census,
        }
    }

    fn series_per_set(&self) -> usize {
        // A no-findings report still describes judged series, not missing input.
        self.findings.len().div_ceil(self.sets.len()).max(1)
    }
}

/// Keeps both partitions populated in the low case.
pub const LOW_FINDINGS: usize = 2;
/// Reveals report-size scaling and exceeds the default summary cap without a large history.
pub const HIGH_FINDINGS: usize = 20;
/// Exercises the quiet history report's early return, independently of scaling cases.
pub const NO_FINDINGS: usize = 0;
/// Short before/after regimes keep chart construction representative and bounded.
const CHART_POINTS: usize = 8;
/// Ordinary instruction-count baseline; exact ties keep chart shape deterministic.
const BASELINE: f64 = 100.0;
/// Descending changes remain well above a negligible fluctuation throughout the fixture.
const LARGEST_DELTA: f64 = 40.0;
/// Synthetic full commit identities keep header and finding attribution realistic.
const FIRST_COMMIT: &str = "0000000000000000000000000000000000000001";
const CHANGE_COMMIT: &str = "0000000000000000000000000000000000000005";
const LAST_COMMIT: &str = "0000000000000000000000000000000000000008";

fn finding(index: usize, set: &DiscriminantSet) -> Finding {
    let delta = LARGEST_DELTA
        - f64::from(u32::try_from(index).expect("fixture indices are bounded by HIGH_FINDINGS"));
    let latest = BASELINE + delta;
    Finding {
        set: set.clone(),
        id: BenchmarkId::new(nonempty![
            "render".to_owned(),
            "group".to_owned(),
            format!("finding_{index:03}"),
        ]),
        kind: MetricKind::InstructionCount,
        method: FindingMethod::ChangePoint,
        direction: Direction::Regression,
        baseline: BASELINE,
        latest,
        delta,
        relative_delta: delta / BASELINE,
        commit: Some(CHANGE_COMMIT.to_owned()),
        window_start_commit: None,
        blessed_at: None,
        blessed_commit_time: None,
        series: (0..CHART_POINTS)
            .map(|topo_index| SeriesValue {
                commit: Some(format!("{:040x}", topo_index.saturating_add(1))),
                value: if topo_index < CHART_POINTS.div_ceil(2) {
                    BASELINE
                } else {
                    latest
                },
                dirty: false,
                topo_index,
            })
            .collect(),
        comparison_base_index: None,
        chart_base_ref: Some(CHART_POINTS.saturating_sub(1)),
        branch: None,
    }
}

/// Checks that a full rendering actually includes its findings, partitions and charts.
///
/// # Panics
///
/// The rendered output must retain the fixture's complete workload.
#[expect(
    clippy::indexing_slicing,
    reason = "Benchmark output assertions require the expected JSON structure"
)]
pub fn assert_full_report(input: &ReportInput<'_>, format: ReportFormat) {
    let output = render(input, format, false);
    assert_eq!(input.census.judged(), input.series);
    assert_eq!(input.census.unjudged(), 0);
    assert_eq!(
        input
            .sets
            .iter()
            .map(|set| set.findings.len())
            .sum::<usize>(),
        input.findings.len()
    );
    if format == ReportFormat::Json {
        let json: Value = from_str(&output).expect("the renderer emits valid JSON");
        let findings = json["findings"].as_array().expect("findings is an array");
        assert_eq!(findings.len(), input.findings.len());
        assert_eq!(
            json["sets"].as_array().map(Vec::len),
            Some(input.sets.len())
        );
        assert_eq!(json["census"]["judged"], input.series);
        for (actual, expected) in findings.iter().zip(input.findings) {
            let segments: Vec<_> = expected.id.segments.iter().map(String::as_str).collect();
            assert_eq!(actual["segments"], json!(segments));
        }
        return;
    }
    if input.findings.is_empty() {
        assert!(output.contains("No notable changes detected."));
        assert!(!output.contains("finding_"));
        return;
    }
    for summary in input.sets {
        assert!(output.contains(summary.set.machine_key.as_str()));
        assert!(!summary.findings.is_empty());
    }
    for finding in input.findings {
        assert_eq!(output.matches(&finding.id.qualified()).count(), 1);
        let points: Vec<_> = finding
            .series
            .iter()
            .map(|point| (point.topo_index, point.value))
            .collect();
        assert_eq!(points.len(), CHART_POINTS);
        let chart = chart_series(&points, finding.chart_base_ref)
            .expect("the fixture has nonempty before and after chart regimes");
        assert!(!chart.is_empty());
        assert!(output.contains(&chart));
    }
    if format == ReportFormat::Markdown {
        assert_eq!(output.matches("```text").count(), input.findings.len());
        assert_eq!(output.matches("\n## ").count(), input.sets.len());
    }
}

/// Checks both retention and omission at the production Markdown summary limit.
///
/// # Panics
///
/// Summary output must retain exactly the leading findings and disclose truncation.
pub fn assert_summary(input: &ReportInput<'_>) {
    let output = render_markdown_summary(input, DEFAULT_SUMMARY_LIMIT);
    let retained = input.findings.len().min(DEFAULT_SUMMARY_LIMIT.get());
    assert!(output.contains(&format!("- Regressions: {}", input.findings.len())));
    assert_eq!(output.matches("```text").count(), retained);
    assert_eq!(output.matches("\n## ").count(), retained);
    assert_eq!(output.matches("_Filter:_").count(), retained);
    assert_eq!(
        output.contains("Showing the top"),
        input.findings.len() > retained
    );
    for (index, finding) in input.findings.iter().enumerate() {
        assert_eq!(output.contains(&finding.id.qualified()), index < retained);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::ptr;

    use ::testing::assert_panics;

    use super::*;

    #[test]
    fn summaries_partition_findings_and_preserve_judged_series() {
        // The odd count exercises rounding independently of the benchmark scaling cases.
        for (count, series_per_set) in [
            (NO_FINDINGS, 1),
            (LOW_FINDINGS, 1),
            (3, 2),
            (HIGH_FINDINGS, 10),
        ] {
            let fixture = ReportFixture::new(count);
            let summaries = fixture.summaries();
            let input = fixture.input(&summaries);

            assert_eq!(summaries.len(), 2);
            assert_eq!(fixture.series_per_set(), series_per_set);
            assert_eq!(input.series, series_per_set * summaries.len());
            assert_eq!(input.census.judged(), input.series);
            assert_eq!(input.census.unjudged(), 0);
            assert_eq!(input.findings.len(), count);
            assert_eq!(input.runs, CHART_POINTS * summaries.len());
            for (partition, summary) in summaries.iter().enumerate() {
                assert_eq!(summary.set, &fixture.sets[partition]);
                assert_eq!(summary.runs, CHART_POINTS);
                assert_eq!(summary.series, series_per_set);
                let expected: Vec<_> = fixture
                    .findings
                    .iter()
                    .skip(partition)
                    .step_by(summaries.len())
                    .collect();
                assert_eq!(summary.findings.len(), expected.len());
                for (actual, expected) in summary.findings.iter().zip(expected) {
                    assert!(ptr::eq(*actual, expected));
                    assert_eq!(&actual.set, summary.set);
                }
            }
        }
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "Deterministic fixture arithmetic must match the exact expected floating-point values"
    )]
    fn findings_rank_regressions_with_exact_before_and_after_regimes() {
        let fixture = ReportFixture::new(HIGH_FINDINGS);
        assert!(
            fixture
                .findings
                .windows(2)
                .all(|pair| pair[0].relative_delta > pair[1].relative_delta)
        );

        // Endpoint values independently check the fixture arithmetic, including a nonzero index.
        for (index, delta, latest, relative_delta) in
            [(0, 40.0, 140.0, 0.40), (19, 21.0, 121.0, 0.21)]
        {
            let finding = &fixture.findings[index];
            assert_eq!(finding.baseline, 100.0);
            assert_eq!(finding.delta, delta);
            assert_eq!(finding.latest, latest);
            assert_eq!(finding.relative_delta, relative_delta);
            assert_eq!(finding.direction, Direction::Regression);
            assert_eq!(finding.kind, MetricKind::InstructionCount);
            assert_eq!(finding.method, FindingMethod::ChangePoint);
            assert_eq!(finding.commit.as_deref(), Some(CHANGE_COMMIT));
            let values: Vec<_> = finding.series.iter().map(|point| point.value).collect();
            assert_eq!(
                values,
                [100.0, 100.0, 100.0, 100.0, latest, latest, latest, latest]
            );
            assert_eq!(finding.chart_base_ref, Some(7));
            for (index, point) in finding.series.iter().enumerate() {
                assert_eq!(point.topo_index, index);
                assert_eq!(point.commit, Some(format!("{:040x}", index + 1)));
                assert!(!point.dirty);
            }
            assert_eq!(finding.series[4].commit, finding.commit);
        }
    }

    #[test]
    #[should_panic]
    fn fixture_rejects_unbounded_workloads() {
        _ = ReportFixture::new(HIGH_FINDINGS + 1);
    }

    #[test]
    fn full_text_assertions_accept_complete_report() {
        let fixture = ReportFixture::new(LOW_FINDINGS);
        let summaries = fixture.summaries();
        assert_full_report(&fixture.input(&summaries), ReportFormat::Text);
    }

    #[test]
    fn full_markdown_assertions_accept_complete_report() {
        let fixture = ReportFixture::new(LOW_FINDINGS);
        let summaries = fixture.summaries();
        assert_full_report(&fixture.input(&summaries), ReportFormat::Markdown);
    }

    #[test]
    fn full_json_assertions_accept_complete_report() {
        let fixture = ReportFixture::new(LOW_FINDINGS);
        let summaries = fixture.summaries();
        assert_full_report(&fixture.input(&summaries), ReportFormat::Json);
    }

    #[test]
    fn full_text_assertions_accept_quiet_report() {
        let fixture = ReportFixture::new(NO_FINDINGS);
        let summaries = fixture.summaries();
        assert_full_report(&fixture.input(&summaries), ReportFormat::Text);
    }

    #[test]
    fn full_report_assertions_reject_missing_partition_findings() {
        let fixture = ReportFixture::new(LOW_FINDINGS);
        let mut summaries = fixture.summaries();
        summaries[0].findings.clear();
        let input = fixture.input(&summaries);
        _ = render(&input, ReportFormat::Text, false);
        assert_panics(|| assert_full_report(&input, ReportFormat::Text));
    }

    #[test]
    fn full_report_assertions_reject_incomplete_chart_workload() {
        let mut fixture = ReportFixture::new(LOW_FINDINGS);
        _ = fixture.findings[0].series.pop();
        let summaries = fixture.summaries();
        let input = fixture.input(&summaries);
        _ = render(&input, ReportFormat::Markdown, false);
        assert_panics(|| assert_full_report(&input, ReportFormat::Markdown));
    }

    #[test]
    fn summary_assertions_accept_retained_findings() {
        assert!(LOW_FINDINGS <= DEFAULT_SUMMARY_LIMIT.get());
        let fixture = ReportFixture::new(LOW_FINDINGS);
        let summaries = fixture.summaries();
        assert_summary(&fixture.input(&summaries));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Rendering the production summary cap exceeds the Miri workload budget; \
                  retained summaries cover the same operations with a small fixture"
    )]
    fn summary_assertions_accept_truncated_findings() {
        assert!(HIGH_FINDINGS > DEFAULT_SUMMARY_LIMIT.get());
        let fixture = ReportFixture::new(HIGH_FINDINGS);
        let summaries = fixture.summaries();
        assert_summary(&fixture.input(&summaries));
    }

    #[test]
    fn summary_assertions_reject_non_regression_workload() {
        let mut fixture = ReportFixture::new(LOW_FINDINGS);
        fixture.findings[0].direction = Direction::Improvement;
        let summaries = fixture.summaries();
        let input = fixture.input(&summaries);
        _ = render_markdown_summary(&input, DEFAULT_SUMMARY_LIMIT);
        assert_panics(|| assert_summary(&input));
    }
}
