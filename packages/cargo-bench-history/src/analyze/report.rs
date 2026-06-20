//! Rendering analysis results into a human- or machine-readable report.
//!
//! Three formats are offered: a compact `text` summary for terminals, a
//! `markdown` document for pasting into pull requests, and a `json` document for
//! programmatic consumption.
//!
//! A report covers one or more *discriminant sets* (engine / triple / machine
//! partitions). The top level carries the project-wide totals and the globally
//! ranked findings, and a per-set breakdown follows so each comparable partition
//! reads as its own section.

use serde::Serialize;

use crate::analyze::discriminant::DiscriminantSet;
use crate::analyze::findings::{Direction, Finding, FindingMethod, Severity};
use crate::model::BenchmarkId;

/// The selectable output format of an analysis report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReportFormat {
    /// A compact, human-readable plain-text summary.
    Text,
    /// A machine-readable JSON document.
    Json,
    /// A Markdown summary with a findings table per set.
    Markdown,
}

impl ReportFormat {
    /// Parses a format from its command-line name, if recognized.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            "markdown" | "md" => Some(Self::Markdown),
            _ => None,
        }
    }
}

/// One discriminant set's slice of the report.
#[derive(Clone, Debug)]
pub(crate) struct SetSummary<'a> {
    /// The comparable partition this slice covers.
    pub(crate) set: &'a DiscriminantSet,
    /// Number of stored runs loaded for this set.
    pub(crate) runs: usize,
    /// Number of distinct series compared in this set.
    pub(crate) series: usize,
    /// The set's findings, in the same global ranking as the top level.
    pub(crate) findings: Vec<&'a Finding>,
}

/// The inputs a report is rendered from.
#[derive(Clone, Debug)]
pub(crate) struct ReportInput<'a> {
    /// The project the history belongs to.
    pub(crate) project: &'a str,
    /// Total stored runs loaded across every set.
    pub(crate) runs: usize,
    /// Total distinct series compared across every set.
    pub(crate) series: usize,
    /// Every set's findings, globally ranked most-notable first.
    pub(crate) findings: &'a [Finding],
    /// The per-set breakdown, one entry per set that contributed data.
    pub(crate) sets: &'a [SetSummary<'a>],
    /// A diagnostic hint shown when stored runs existed but none were analyzed,
    /// explaining why the outcome is empty. Absent in the normal case.
    pub(crate) hint: Option<&'a str>,
    /// A warning shown when the analysis admitted dirty runs on the base branch's
    /// tip (the working-tree-dirty exception). Absent in the normal case.
    pub(crate) warning: Option<&'a str>,
}

/// The JSON shape of a per-set slice.
#[derive(Serialize)]
struct JsonSet<'a> {
    /// Engine identifier.
    engine: &'a str,
    /// Resolved target triple.
    target_triple: &'a str,
    /// Operating-system facet derived from the triple.
    os: &'a str,
    /// CPU-architecture facet derived from the triple.
    architecture: &'a str,
    /// Machine partition (`synthetic` for hardware-independent engines).
    machine: &'a str,
    /// Stored runs loaded for this set.
    runs: usize,
    /// Distinct series compared in this set.
    series: usize,
    /// Flagged regressions in this set.
    regressions: usize,
    /// Flagged improvements in this set.
    improvements: usize,
    /// This set's findings, ranked most-notable first.
    findings: Vec<&'a Finding>,
}

/// The JSON shape of a rendered report.
#[derive(Serialize)]
struct JsonReport<'a> {
    /// The project the history belongs to.
    project: &'a str,
    /// Total stored runs loaded.
    runs: usize,
    /// Total distinct series compared.
    series: usize,
    /// Number of flagged regressions.
    regressions: usize,
    /// Number of flagged improvements.
    improvements: usize,
    /// A diagnostic hint when stored runs existed but none were analyzed.
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'a str>,
    /// A warning when dirty base-branch-tip runs were admitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<&'a str>,
    /// Every set's findings, globally ranked.
    findings: &'a [Finding],
    /// The per-set breakdown.
    sets: Vec<JsonSet<'a>>,
}

/// Renders `input` in the requested `format`.
pub(crate) fn render(input: &ReportInput<'_>, format: ReportFormat) -> String {
    match format {
        ReportFormat::Text => render_text(input),
        ReportFormat::Markdown => render_markdown(input),
        ReportFormat::Json => render_json(input),
    }
}

/// Counts findings matching `direction`.
fn count_direction(findings: &[&Finding], direction: Direction) -> usize {
    findings
        .iter()
        .filter(|finding| finding.direction == direction)
        .count()
}

/// Counts top-level findings matching `direction`.
fn count_top(findings: &[Finding], direction: Direction) -> usize {
    findings
        .iter()
        .filter(|finding| finding.direction == direction)
        .count()
}

/// Joins report lines into the final string with a trailing newline.
fn finish(lines: &[String]) -> String {
    format!("{}\n", lines.join("\n"))
}

/// Appends the ephemeral-data warning (if any) as a trailing, blank-line-separated
/// block, so it reads at the very end of the report.
fn push_warning(lines: &mut Vec<String>, warning: Option<&str>) {
    if let Some(warning) = warning {
        lines.push(String::new());
        lines.push(warning.to_owned());
    }
}

/// A one-line label for a set, naming its partition and derived facets.
fn set_label(set: &DiscriminantSet) -> String {
    format!("{set} (os={} arch={})", set.os(), set.architecture())
}

fn render_text(input: &ReportInput<'_>) -> String {
    let regressions = count_top(input.findings, Direction::Regression);
    let improvements = count_top(input.findings, Direction::Improvement);

    let mut lines = vec![
        format!("Analyzed project {}", input.project),
        format!(
            "  runs: {}  series: {}  regressions: {regressions}  improvements: {improvements}",
            input.runs, input.series
        ),
    ];

    if input.findings.is_empty() {
        lines.push("No notable changes detected.".to_owned());
        if let Some(hint) = input.hint {
            lines.push(String::new());
            lines.push(hint.to_owned());
        }
        push_warning(&mut lines, input.warning);
        return finish(&lines);
    }

    for summary in input.sets {
        if summary.findings.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(format!("Set {}", set_label(summary.set)));
        for finding in &summary.findings {
            lines.push(format!(
                "  [{}] {} via {} ({} confidence) {} {}/{}: {} -> {} ({})",
                severity_label(finding.severity),
                direction_label(finding.direction),
                method_label(finding.method),
                format_confidence(finding.confidence),
                finding.set.engine,
                describe_id(&finding.id),
                finding.metric,
                format_value(finding.baseline),
                format_value(finding.latest),
                format_percent(finding.relative_delta),
            ));
        }
    }
    push_warning(&mut lines, input.warning);
    finish(&lines)
}

fn render_markdown(input: &ReportInput<'_>) -> String {
    let regressions = count_top(input.findings, Direction::Regression);
    let improvements = count_top(input.findings, Direction::Improvement);

    let mut lines = vec![
        format!("# Benchmark history analysis: {}", input.project),
        String::new(),
        format!("- Runs analyzed: {}", input.runs),
        format!("- Series compared: {}", input.series),
        format!("- Regressions: {regressions}"),
        format!("- Improvements: {improvements}"),
    ];

    if input.findings.is_empty() {
        lines.push(String::new());
        lines.push("No notable changes detected.".to_owned());
        if let Some(hint) = input.hint {
            lines.push(String::new());
            lines.push(hint.to_owned());
        }
        push_warning(&mut lines, input.warning);
        return finish(&lines);
    }

    for summary in input.sets {
        if summary.findings.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(format!(
            "## {} (os={}, arch={})",
            summary.set,
            summary.set.os(),
            summary.set.architecture()
        ));
        lines.push(String::new());
        lines.push(
            "| Severity | Direction | Method | Confidence | Engine | Benchmark | Metric | Baseline | Latest | Change |"
                .to_owned(),
        );
        lines.push("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |".to_owned());
        for finding in &summary.findings {
            lines.push(format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                severity_label(finding.severity),
                direction_label(finding.direction),
                method_label(finding.method),
                format_confidence(finding.confidence),
                finding.set.engine,
                describe_id(&finding.id),
                finding.metric,
                format_value(finding.baseline),
                format_value(finding.latest),
                format_percent(finding.relative_delta),
            ));
        }
    }
    push_warning(&mut lines, input.warning);
    finish(&lines)
}

// Pure serialization glue, fully exercised by `json_report_is_structured` and
// `report_renders_every_severity_and_direction_label` in regular CI. Skipped for
// mutation only: the empty-string mutant is caught fast by those lib tests
// locally, but tips the 60s cargo-mutants timeout on the slower Windows shards.
#[cfg_attr(test, mutants::skip)]
fn render_json(input: &ReportInput<'_>) -> String {
    let sets = input
        .sets
        .iter()
        .map(|summary| JsonSet {
            engine: &summary.set.engine,
            target_triple: &summary.set.target_triple,
            os: summary.set.os(),
            architecture: summary.set.architecture(),
            machine: &summary.set.machine,
            runs: summary.runs,
            series: summary.series,
            regressions: count_direction(&summary.findings, Direction::Regression),
            improvements: count_direction(&summary.findings, Direction::Improvement),
            findings: summary.findings.clone(),
        })
        .collect();

    let report = JsonReport {
        project: input.project,
        runs: input.runs,
        series: input.series,
        regressions: count_top(input.findings, Direction::Regression),
        improvements: count_top(input.findings, Direction::Improvement),
        hint: input.hint,
        warning: input.warning,
        findings: input.findings,
        sets,
    };
    // The report is built from plain structs whose only numbers are finite (or
    // serialized as `null` by serde_json), so serialization cannot fail.
    serde_json::to_string_pretty(&report).expect("report structures always serialize to JSON")
}

/// The lowercase label for a severity tier.
fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Major => "major",
        Severity::Moderate => "moderate",
        Severity::Minor => "minor",
    }
}

/// The lowercase label for a change direction.
fn direction_label(direction: Direction) -> &'static str {
    match direction {
        Direction::Regression => "regression",
        Direction::Improvement => "improvement",
    }
}

/// The lowercase label for the detector that produced a finding.
fn method_label(method: FindingMethod) -> &'static str {
    match method {
        FindingMethod::ChangePoint => "change point",
        FindingMethod::Drift => "drift",
    }
}

/// Formats a detector's confidence as a whole-number percentage.
fn format_confidence(confidence: f64) -> String {
    format!("{:.0}%", (confidence * 100.0).clamp(0.0, 100.0))
}

/// Renders a benchmark identity as `package/group/case/value`, omitting absent
/// parts. The package-qualified form keeps benchmarks with the same `module_path`
/// in different packages distinguishable in reports.
fn describe_id(id: &BenchmarkId) -> String {
    id.qualified()
}

/// Formats a measured value, dropping the fraction for integer-valued counts.
fn format_value(value: f64) -> String {
    if value.fract().abs() <= f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

/// Formats a relative delta as a signed percentage with two decimals.
fn format_percent(relative_delta: f64) -> String {
    format!("{:+.2}%", relative_delta * 100.0)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use crate::model::MetricKind;

    use super::*;

    fn discriminant_set() -> DiscriminantSet {
        DiscriminantSet {
            engine: "callgrind".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine: "synthetic".to_owned(),
        }
    }

    fn regression() -> Finding {
        Finding {
            set: discriminant_set(),
            id: BenchmarkId::new(
                Some("nm".to_owned()),
                "nm::observe".to_owned(),
                Some("pull".to_owned()),
                None,
            ),
            metric: "Ir".to_owned(),
            kind: MetricKind::InstructionCount,
            method: FindingMethod::ChangePoint,
            direction: Direction::Regression,
            severity: Severity::Major,
            baseline: 100.0,
            latest: 130.0,
            delta: 30.0,
            relative_delta: 0.30,
            confidence: 1.0,
            commit: Some("deadbee".to_owned()),
        }
    }

    /// Wraps a findings slice into a single-set report over `set`.
    fn single_set_input<'a>(
        project: &'a str,
        set: &'a DiscriminantSet,
        findings: &'a [Finding],
        summaries: &'a mut Vec<SetSummary<'a>>,
    ) -> ReportInput<'a> {
        summaries.push(SetSummary {
            set,
            runs: findings.len().saturating_add(3),
            series: findings.len().max(1),
            findings: findings.iter().collect(),
        });
        ReportInput {
            project,
            runs: findings.len().saturating_add(3),
            series: findings.len().max(1),
            findings,
            sets: summaries,
            hint: None,
            warning: None,
        }
    }

    #[test]
    fn format_from_name_recognizes_known_formats() {
        assert_eq!(ReportFormat::from_name("text"), Some(ReportFormat::Text));
        assert_eq!(ReportFormat::from_name("json"), Some(ReportFormat::Json));
        assert_eq!(
            ReportFormat::from_name("markdown"),
            Some(ReportFormat::Markdown)
        );
        assert_eq!(ReportFormat::from_name("md"), Some(ReportFormat::Markdown));
        assert_eq!(ReportFormat::from_name("yaml"), None);
    }

    #[test]
    fn text_report_with_no_findings_is_explicit() {
        let input = ReportInput {
            project: "folo",
            runs: 3,
            series: 1,
            findings: &[],
            sets: &[],
            hint: None,
            warning: None,
        };
        let report = render(&input, ReportFormat::Text);
        assert!(report.contains("Analyzed project folo"), "{report}");
        assert!(report.contains("regressions: 0"), "{report}");
        assert!(report.contains("No notable changes detected."), "{report}");
    }

    #[test]
    fn text_report_renders_hint_when_present() {
        let input = ReportInput {
            project: "folo",
            runs: 0,
            series: 0,
            findings: &[],
            sets: &[],
            hint: Some("Found 2 stored runs ... dirty snapshots"),
            warning: None,
        };
        let report = render(&input, ReportFormat::Text);
        assert!(report.contains("No notable changes detected."), "{report}");
        assert!(report.contains("Found 2 stored runs"), "{report}");
    }

    #[test]
    fn text_report_lists_a_finding() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let report = render(&input, ReportFormat::Text);
        assert!(report.contains("regressions: 1"), "{report}");
        assert!(report.contains("Set callgrind/"), "{report}");
        assert!(report.contains("[major] regression"), "{report}");
        assert!(report.contains("nm::observe/pull/Ir"), "{report}");
        assert!(report.contains("100 -> 130"), "{report}");
        assert!(report.contains("+30.00%"), "{report}");
    }

    #[test]
    fn report_renders_every_severity_and_direction_label() {
        let set = discriminant_set();
        let mut moderate = regression();
        moderate.severity = Severity::Moderate;
        let mut improvement = regression();
        improvement.severity = Severity::Minor;
        improvement.direction = Direction::Improvement;
        improvement.delta = -5.0;
        improvement.relative_delta = -0.05;
        let findings = vec![regression(), moderate, improvement];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);

        let text = render(&input, ReportFormat::Text);
        assert!(text.contains("[major] regression"), "{text}");
        assert!(text.contains("[moderate] regression"), "{text}");
        assert!(text.contains("[minor] improvement"), "{text}");

        let markdown = render(&input, ReportFormat::Markdown);
        assert!(markdown.contains("| moderate | regression |"), "{markdown}");
        assert!(markdown.contains("| minor | improvement |"), "{markdown}");

        // The per-set JSON tallies count each direction independently: this set
        // holds two regressions and one improvement, so neither count is one.
        let json = render(&input, ReportFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("report is JSON");
        let set_json = &parsed["sets"][0];
        assert_eq!(set_json["regressions"], 2, "{json}");
        assert_eq!(set_json["improvements"], 1, "{json}");
    }

    #[test]
    fn markdown_report_renders_a_table_per_set() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let report = render(&input, ReportFormat::Markdown);
        assert!(
            report.contains("# Benchmark history analysis: folo"),
            "{report}"
        );
        assert!(
            report.contains(
                "## callgrind/x86_64-unknown-linux-gnu/synthetic (os=linux, arch=x86_64)"
            ),
            "{report}"
        );
        assert!(report.contains("| Severity | Direction |"), "{report}");
        assert!(report.contains("| major | regression |"), "{report}");
    }

    #[test]
    fn markdown_report_with_no_findings() {
        let input = ReportInput {
            project: "folo",
            runs: 0,
            series: 0,
            findings: &[],
            sets: &[],
            hint: Some("Found 2 stored runs ... commit your working tree"),
            warning: None,
        };
        let report = render(&input, ReportFormat::Markdown);
        assert!(report.contains("No notable changes detected."), "{report}");
        assert!(report.contains("commit your working tree"), "{report}");
    }

    #[test]
    fn json_report_includes_hint_field_when_present() {
        let input = ReportInput {
            project: "folo",
            runs: 0,
            series: 0,
            findings: &[],
            sets: &[],
            hint: Some("dirty snapshots on base-branch commits"),
            warning: None,
        };
        let report = render(&input, ReportFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&report).expect("report is JSON");
        assert_eq!(
            parsed["hint"], "dirty snapshots on base-branch commits",
            "{report}"
        );
    }

    #[test]
    fn warning_renders_at_the_end_of_every_format() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.warning = Some("Warning: analysis included dirty runs (ephemeral).");

        let text = render(&input, ReportFormat::Text);
        assert!(text.trim_end().ends_with("(ephemeral)."), "{text}");

        let markdown = render(&input, ReportFormat::Markdown);
        assert!(markdown.trim_end().ends_with("(ephemeral)."), "{markdown}");

        let json = render(&input, ReportFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("report is JSON");
        assert_eq!(
            parsed["warning"], "Warning: analysis included dirty runs (ephemeral).",
            "{json}"
        );
    }

    #[test]
    fn warning_renders_even_when_there_are_no_findings() {
        let input = ReportInput {
            project: "folo",
            runs: 1,
            series: 1,
            findings: &[],
            sets: &[],
            hint: None,
            warning: Some("Warning: dirty runs were included."),
        };
        let text = render(&input, ReportFormat::Text);
        assert!(text.contains("No notable changes detected."), "{text}");
        assert!(text.trim_end().ends_with("included."), "{text}");

        let markdown = render(&input, ReportFormat::Markdown);
        assert!(markdown.trim_end().ends_with("included."), "{markdown}");
    }

    #[test]
    fn omitted_warning_is_absent_from_json() {
        let input = ReportInput {
            project: "folo",
            runs: 0,
            series: 0,
            findings: &[],
            sets: &[],
            hint: None,
            warning: None,
        };
        let report = render(&input, ReportFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&report).expect("report is JSON");
        assert!(parsed.get("warning").is_none(), "{report}");
    }

    #[test]
    fn json_report_is_structured() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let report = render(&input, ReportFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["project"], "folo");
        assert_eq!(parsed["regressions"], 1);
        assert_eq!(parsed["improvements"], 0);
        let finding = &parsed["findings"][0];
        // Flattened DiscriminantSet and BenchmarkId fields appear inline.
        assert_eq!(finding["engine"], "callgrind");
        assert_eq!(finding["package"], "nm");
        assert_eq!(finding["group"], "nm::observe");
        assert_eq!(finding["direction"], "regression");
        assert_eq!(finding["severity"], "major");
        // The per-set breakdown carries the derived facets.
        let set_json = &parsed["sets"][0];
        assert_eq!(set_json["engine"], "callgrind");
        assert_eq!(set_json["os"], "linux");
        assert_eq!(set_json["architecture"], "x86_64");
        assert_eq!(set_json["regressions"], 1);
    }

    #[test]
    fn format_value_drops_integer_fraction() {
        assert_eq!(format_value(36.0), "36");
        assert_eq!(format_value(12.5), "12.5");
    }

    #[test]
    fn describe_id_joins_present_parts() {
        let id = BenchmarkId::new(
            Some("pkg".to_owned()),
            "group".to_owned(),
            Some("case".to_owned()),
            Some("value".to_owned()),
        );
        assert_eq!(describe_id(&id), "pkg/group/case/value");
        let bare = BenchmarkId::new(None, "group".to_owned(), None, None);
        assert_eq!(describe_id(&bare), "group");
    }
}
