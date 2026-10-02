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
