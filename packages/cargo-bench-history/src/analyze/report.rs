//! Rendering analysis results into a human- or machine-readable report.
//!
//! Three formats are offered: a compact `text` summary for terminals, a
//! `markdown` table for pasting into pull requests, and a `json` document for
//! programmatic consumption.

use serde::Serialize;

use crate::analyze::findings::{Direction, Finding, Severity};
use crate::model::BenchmarkId;

/// The selectable output format of an analysis report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReportFormat {
    /// A compact, human-readable plain-text summary.
    Text,
    /// A machine-readable JSON document.
    Json,
    /// A Markdown summary with a findings table.
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

/// The inputs a report is rendered from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReportInput<'a> {
    /// The project the history belongs to.
    pub(crate) project: &'a str,
    /// Number of stored runs that were loaded.
    pub(crate) runs: usize,
    /// Number of distinct series that were compared.
    pub(crate) series: usize,
    /// The findings, already ranked most-notable first.
    pub(crate) findings: &'a [Finding],
}

/// The JSON shape of a rendered report.
#[derive(Serialize)]
struct JsonReport<'a> {
    /// The project the history belongs to.
    project: &'a str,
    /// Number of stored runs that were loaded.
    runs: usize,
    /// Number of distinct series that were compared.
    series: usize,
    /// Number of flagged regressions.
    regressions: usize,
    /// Number of flagged improvements.
    improvements: usize,
    /// The findings, ranked most-notable first.
    findings: &'a [Finding],
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
fn count_direction(findings: &[Finding], direction: Direction) -> usize {
    findings
        .iter()
        .filter(|finding| finding.direction == direction)
        .count()
}

/// Joins report lines into the final string with a trailing newline.
fn finish(lines: &[String]) -> String {
    format!("{}\n", lines.join("\n"))
}

fn render_text(input: &ReportInput<'_>) -> String {
    let regressions = count_direction(input.findings, Direction::Regression);
    let improvements = count_direction(input.findings, Direction::Improvement);

    let mut lines = vec![
        format!("Analyzed project {}", input.project),
        format!(
            "  runs: {}  series: {}  regressions: {regressions}  improvements: {improvements}",
            input.runs, input.series
        ),
    ];

    if input.findings.is_empty() {
        lines.push("No notable changes detected.".to_owned());
        return finish(&lines);
    }

    lines.push(String::new());
    lines.push("Findings:".to_owned());
    for finding in input.findings {
        lines.push(format!(
            "  [{}] {} {} {}/{}: {} -> {} ({})",
            severity_label(finding.severity),
            direction_label(finding.direction),
            finding.location.system,
            describe_id(&finding.id),
            finding.metric,
            format_value(finding.baseline),
            format_value(finding.latest),
            format_percent(finding.relative_delta),
        ));
    }
    finish(&lines)
}

fn render_markdown(input: &ReportInput<'_>) -> String {
    let regressions = count_direction(input.findings, Direction::Regression);
    let improvements = count_direction(input.findings, Direction::Improvement);

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
        return finish(&lines);
    }

    lines.push(String::new());
    lines.push(
        "| Severity | Direction | System | Benchmark | Metric | Baseline | Latest | Change |"
            .to_owned(),
    );
    lines.push("| --- | --- | --- | --- | --- | --- | --- | --- |".to_owned());
    for finding in input.findings {
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            severity_label(finding.severity),
            direction_label(finding.direction),
            finding.location.system,
            describe_id(&finding.id),
            finding.metric,
            format_value(finding.baseline),
            format_value(finding.latest),
            format_percent(finding.relative_delta),
        ));
    }
    finish(&lines)
}

fn render_json(input: &ReportInput<'_>) -> String {
    let report = JsonReport {
        project: input.project,
        runs: input.runs,
        series: input.series,
        regressions: count_direction(input.findings, Direction::Regression),
        improvements: count_direction(input.findings, Direction::Improvement),
        findings: input.findings,
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

/// Renders a benchmark identity as `group/case/value`, omitting absent parts.
fn describe_id(id: &BenchmarkId) -> String {
    let mut parts = vec![id.group.clone()];
    if let Some(case) = &id.case {
        parts.push(case.clone());
    }
    if let Some(value) = &id.value {
        parts.push(value.clone());
    }
    parts.join("/")
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

    use crate::analyze::series::Location;
    use crate::model::MetricKind;

    use super::*;

    fn location() -> Location {
        Location {
            system: "callgrind".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine: "synthetic".to_owned(),
        }
    }

    fn regression() -> Finding {
        Finding {
            location: location(),
            id: BenchmarkId::new("nm::observe".to_owned(), Some("pull".to_owned()), None),
            metric: "Ir".to_owned(),
            kind: MetricKind::InstructionCount,
            direction: Direction::Regression,
            severity: Severity::Major,
            baseline: 100.0,
            latest: 130.0,
            delta: 30.0,
            relative_delta: 0.30,
            commit: Some("deadbee".to_owned()),
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
        };
        let report = render(&input, ReportFormat::Text);
        assert!(report.contains("Analyzed project folo"), "{report}");
        assert!(report.contains("regressions: 0"), "{report}");
        assert!(report.contains("No notable changes detected."), "{report}");
    }

    #[test]
    fn text_report_lists_a_finding() {
        let findings = vec![regression()];
        let input = ReportInput {
            project: "folo",
            runs: 4,
            series: 1,
            findings: &findings,
        };
        let report = render(&input, ReportFormat::Text);
        assert!(report.contains("regressions: 1"), "{report}");
        assert!(report.contains("[major] regression"), "{report}");
        assert!(report.contains("nm::observe/pull/Ir"), "{report}");
        assert!(report.contains("100 -> 130"), "{report}");
        assert!(report.contains("+30.00%"), "{report}");
    }

    #[test]
    fn report_renders_every_severity_and_direction_label() {
        let mut moderate = regression();
        moderate.severity = Severity::Moderate;
        let mut improvement = regression();
        improvement.severity = Severity::Minor;
        improvement.direction = Direction::Improvement;
        improvement.delta = -5.0;
        improvement.relative_delta = -0.05;
        let findings = vec![regression(), moderate, improvement];
        let input = ReportInput {
            project: "folo",
            runs: 6,
            series: 3,
            findings: &findings,
        };

        let text = render(&input, ReportFormat::Text);
        assert!(text.contains("[major] regression"), "{text}");
        assert!(text.contains("[moderate] regression"), "{text}");
        assert!(text.contains("[minor] improvement"), "{text}");

        let markdown = render(&input, ReportFormat::Markdown);
        assert!(markdown.contains("| moderate | regression |"), "{markdown}");
        assert!(markdown.contains("| minor | improvement |"), "{markdown}");
    }

    #[test]
    fn markdown_report_renders_a_table() {
        let findings = vec![regression()];
        let input = ReportInput {
            project: "folo",
            runs: 4,
            series: 1,
            findings: &findings,
        };
        let report = render(&input, ReportFormat::Markdown);
        assert!(
            report.contains("# Benchmark history analysis: folo"),
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
        };
        let report = render(&input, ReportFormat::Markdown);
        assert!(report.contains("No notable changes detected."), "{report}");
    }

    #[test]
    fn json_report_is_structured() {
        let findings = vec![regression()];
        let input = ReportInput {
            project: "folo",
            runs: 4,
            series: 1,
            findings: &findings,
        };
        let report = render(&input, ReportFormat::Json);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["project"], "folo");
        assert_eq!(parsed["regressions"], 1);
        assert_eq!(parsed["improvements"], 0);
        let finding = &parsed["findings"][0];
        // Flattened Location and BenchmarkId fields appear inline.
        assert_eq!(finding["system"], "callgrind");
        assert_eq!(finding["group"], "nm::observe");
        assert_eq!(finding["direction"], "regression");
        assert_eq!(finding["severity"], "major");
    }

    #[test]
    fn format_value_drops_integer_fraction() {
        assert_eq!(format_value(36.0), "36");
        assert_eq!(format_value(12.5), "12.5");
    }

    #[test]
    fn describe_id_joins_present_parts() {
        let id = BenchmarkId::new(
            "group".to_owned(),
            Some("case".to_owned()),
            Some("value".to_owned()),
        );
        assert_eq!(describe_id(&id), "group/case/value");
        let bare = BenchmarkId::new("group".to_owned(), None, None);
        assert_eq!(describe_id(&bare), "group");
    }
}
