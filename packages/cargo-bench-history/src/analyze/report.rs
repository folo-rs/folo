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

use colored::{Color, Colorize};
use rasciigraph::{Config, plot_colored, plot_many_colored};
use serde::Serialize;

use crate::analyze::discriminant::DiscriminantSet;
use crate::analyze::findings::{Direction, Finding, FindingMethod, SeriesValue};
use crate::model::BenchmarkId;

/// Height, in rows, of a history-mode finding chart.
const CHART_HEIGHT: u32 = 4;
/// Width, in columns, of a history-mode finding chart.
const CHART_WIDTH: u32 = 48;

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
    /// The analysis mode the report was produced in (`history`/`branch`/`tip`).
    pub(crate) mode: &'a str,
    /// Whether any finding survived — the at-a-glance signal a downstream
    /// automation reads to decide whether the report is worth surfacing.
    pub(crate) notable: bool,
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
    /// The analysis mode (`history`/`branch`/`tip`).
    mode: &'a str,
    /// Whether any finding survived — the downstream automation signal.
    notable: bool,
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
///
/// `color` enables ANSI styling in the text format (the headline percentage and
/// the per-finding chart). The caller decides it from the output terminal so tests
/// and pipes stay plain; `markdown` and `json` ignore it.
pub(crate) fn render(input: &ReportInput<'_>, format: ReportFormat, color: bool) -> String {
    match format {
        ReportFormat::Text => render_text(input, color),
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

fn render_text(input: &ReportInput<'_>, color: bool) -> String {
    // Force `colored` and `rasciigraph` to honor this explicit decision rather
    // than their own ambient terminal auto-detection, so tests and pipes are
    // deterministic regardless of how the process is run.
    colored::control::set_override(color);

    let regressions = count_top(input.findings, Direction::Regression);
    let improvements = count_top(input.findings, Direction::Improvement);

    let mut lines = vec![
        format!("Analyzed project {} ({} mode)", input.project, input.mode),
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

    // A per-commit chart is meaningful only for a history (`master`) timeline; the
    // branch/tip modes compare against a baseline rather than walking a series.
    let chart_enabled = input.mode == "history";
    for summary in input.sets {
        if summary.findings.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(format!("Set {}", set_label(summary.set)));
        for finding in &summary.findings {
            push_finding_block(&mut lines, finding, chart_enabled);
        }
    }
    push_warning(&mut lines, input.warning);
    finish(&lines)
}

/// Appends one finding as a paragraph: a leading, direction-colored percentage
/// headline, a dimmed detail line, an optional blessing/recovery note, and — in
/// history mode — a chart of the metric over commits.
fn push_finding_block(lines: &mut Vec<String>, finding: &Finding, chart_enabled: bool) {
    lines.push(String::new());

    let percent = format_percent(finding.relative_delta);
    let headline = match finding.direction {
        Direction::Regression => percent.red().bold(),
        Direction::Improvement => percent.green().bold(),
    };
    let status = if finding.active {
        String::new()
    } else {
        format!(" {}", "(recovered)".dimmed())
    };
    lines.push(format!(
        "{headline}  {} · {}{status}",
        describe_id(&finding.id).bold(),
        finding.metric
    ));

    let mut detail = format!(
        "    {} via {} · {} confidence · {} → {} · @ {}",
        direction_label(finding.direction),
        method_label(finding.method),
        format_confidence(finding.confidence),
        format_value(finding.baseline),
        format_value(finding.latest),
        finding.commit.as_deref().unwrap_or("unknown"),
    );
    if let Some(flipped_at) = &finding.flipped_at {
        use std::fmt::Write as _;
        let verb = if finding.active {
            "flips at"
        } else {
            "recovers at"
        };
        write!(detail, " · {verb} {flipped_at}").expect("writing to a String is infallible");
    }
    lines.push(detail.dimmed().to_string());

    // Name the blessing that re-baselined the series, so the reader knows the
    // history before it is intentionally excluded from detection.
    if let Some(blessed_at) = &finding.blessed_at {
        let date = finding
            .blessed_effective
            .as_deref()
            .map_or_else(String::new, |effective| format!(" ({effective})"));
        lines.push(
            format!("    blessed at {blessed_at}{date}")
                .dimmed()
                .to_string(),
        );
    }

    if chart_enabled
        && let Some(chart) = chart_of(&finding.series, finding.direction, finding.active_from)
    {
        lines.push(chart);
    }
}

/// Whether the active window spans the whole series, so no greyed prefix is drawn.
///
/// An `active_from` of zero means every point is active; one at or past the end
/// means there is nothing to grey out. Either way the chart is a single series.
fn covers_whole_series(active_from: usize, len: usize) -> bool {
    active_from == 0 || active_from >= len
}

/// Masks `values` to the greyed pre-blessing prefix: points strictly after
/// `active_from` become `NaN` (a gap), while the boundary point is kept so the
/// grey and active lines join.
fn grey_prefix(values: &[f64], active_from: usize) -> Vec<f64> {
    values
        .iter()
        .enumerate()
        .map(|(index, &value)| {
            if index <= active_from {
                value
            } else {
                f64::NAN
            }
        })
        .collect()
}

/// Masks `values` to the active window: points strictly before `active_from`
/// become `NaN` (a gap), while the boundary point is kept so the grey and active
/// lines join.
fn active_window(values: &[f64], active_from: usize) -> Vec<f64> {
    values
        .iter()
        .enumerate()
        .map(|(index, &value)| {
            if index >= active_from {
                value
            } else {
                f64::NAN
            }
        })
        .collect()
}

/// Renders a compact line chart of a finding's series values over commits, colored
/// by direction. Returns `None` when there are too few points to plot.
///
/// When `active_from` is past the start, the pre-blessing prefix is drawn greyed
/// (the level that was re-baselined away) and the active window in the direction
/// color, so the chart shows the full history while making clear which part the
/// detector judged.
fn chart_of(series: &[SeriesValue], direction: Direction, active_from: usize) -> Option<String> {
    let values: Vec<f64> = series.iter().map(|point| point.value).collect();
    if values.len() < 2 {
        return None;
    }
    let line_color = match direction {
        Direction::Regression => Color::Red,
        Direction::Improvement => Color::Green,
    };
    let config = Config::default()
        .with_height(CHART_HEIGHT)
        .with_width(CHART_WIDTH);
    let chart = if covers_whole_series(active_from, values.len()) {
        plot_colored(values, config.with_series_colors(vec![line_color])).to_string()
    } else {
        // Two overlaid series: the greyed pre-blessing prefix and the
        // direction-colored active window. They share the boundary point so the
        // line reads as continuous; `NaN` renders as a gap elsewhere.
        let grey = grey_prefix(&values, active_from);
        let active = active_window(&values, active_from);
        let config = config.with_series_colors(vec![Color::BrightBlack, line_color]);
        plot_many_colored(vec![grey, active], config).to_string()
    };
    Some(chart.trim_end_matches('\n').to_owned())
}

fn render_markdown(input: &ReportInput<'_>) -> String {
    let regressions = count_top(input.findings, Direction::Regression);
    let improvements = count_top(input.findings, Direction::Improvement);

    let mut lines = vec![
        format!("# Benchmark history analysis: {}", input.project),
        String::new(),
        format!("- Mode: {}", input.mode),
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
            "| Change | Direction | Method | Confidence | Engine | Benchmark | Metric | Baseline | Latest |"
                .to_owned(),
        );
        lines.push("| --- | --- | --- | --- | --- | --- | --- | --- | --- |".to_owned());
        for finding in &summary.findings {
            lines.push(format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                format_percent(finding.relative_delta),
                direction_label(finding.direction),
                method_label(finding.method),
                format_confidence(finding.confidence),
                finding.set.engine,
                describe_id(&finding.id),
                finding.metric,
                format_value(finding.baseline),
                format_value(finding.latest),
            ));
        }
    }
    push_warning(&mut lines, input.warning);
    finish(&lines)
}

// Pure serialization glue, fully exercised by `json_report_is_structured` and
// `report_renders_direction_labels` in regular CI. Skipped for mutation only: the
// empty-string mutant is caught fast by those lib tests locally, but tips the 60s
// cargo-mutants timeout on the slower Windows shards.
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
        mode: input.mode,
        notable: input.notable,
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

/// Formats a measured value for human-readable display.
///
/// Integer-valued counts print whole. Other values keep four significant figures,
/// counting the integer-part digits toward the four, so the whole number before
/// the decimal point is never truncated: `0.20970324` becomes `0.2097`,
/// `96.7664` becomes `96.77`, `0.000001234` keeps all four (`0.000001234`), and a
/// large `1234567.89` drops its fraction entirely (`1234568`). Trailing zeros are
/// trimmed. The machine-readable JSON keeps full precision.
fn format_value(value: f64) -> String {
    if value.fract().abs() <= f64::EPSILON {
        return format!("{value:.0}");
    }
    let decimals = significant_decimals(value.abs());
    let formatted = format!("{value:.decimals$}");
    trim_trailing_zeros(&formatted)
}

/// The number of decimal places that yields four significant figures for
/// `magnitude` (a non-negative, non-integer value), while always keeping every
/// integer-part digit.
fn significant_decimals(magnitude: f64) -> usize {
    // Order of magnitude `e` with 10^e <= magnitude < 10^(e+1), found by a bounded
    // search over the exponent range that covers realistic measurement magnitudes.
    let mut exponent: i32 = -12;
    for candidate in (-12..=12).rev() {
        if magnitude >= 10_f64.powi(candidate) {
            exponent = candidate;
            break;
        }
    }
    // Four significant figures: the least significant digit sits three places below
    // the most significant one. Negative exponents (values below one) add decimal
    // places; large exponents need none.
    let places = 3_i32.saturating_sub(exponent).max(0);
    usize::try_from(places).unwrap_or(0)
}

/// Trims trailing zeros (and a dangling decimal point) from a formatted decimal.
fn trim_trailing_zeros(formatted: &str) -> String {
    if formatted.contains('.') {
        formatted
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    } else {
        formatted.to_owned()
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
            baseline: 100.0,
            latest: 130.0,
            delta: 30.0,
            relative_delta: 0.30,
            confidence: 1.0,
            commit: Some("deadbee".to_owned()),
            flipped_at: None,
            active: true,
            active_from: 0,
            blessed_at: None,
            blessed_effective: None,
            series: Vec::new(),
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
            mode: "history",
            notable: !findings.is_empty(),
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
            mode: "history",
            notable: false,
            runs: 3,
            series: 1,
            findings: &[],
            sets: &[],
            hint: None,
            warning: None,
        };
        let report = render(&input, ReportFormat::Text, false);
        assert!(report.contains("Analyzed project folo"), "{report}");
        assert!(report.contains("regressions: 0"), "{report}");
        assert!(report.contains("No notable changes detected."), "{report}");
    }

    #[test]
    fn text_report_renders_hint_when_present() {
        let input = ReportInput {
            project: "folo",
            mode: "history",
            notable: false,
            runs: 0,
            series: 0,
            findings: &[],
            sets: &[],
            hint: Some("Found 2 stored runs ... dirty snapshots"),
            warning: None,
        };
        let report = render(&input, ReportFormat::Text, false);
        assert!(report.contains("No notable changes detected."), "{report}");
        assert!(report.contains("Found 2 stored runs"), "{report}");
    }

    #[test]
    fn text_report_lists_a_finding() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let report = render(&input, ReportFormat::Text, false);
        assert!(report.contains("regressions: 1"), "{report}");
        assert!(report.contains("Set callgrind/"), "{report}");
        // The percentage leads the finding paragraph; severity is gone.
        assert!(report.contains("+30.00%"), "{report}");
        assert!(!report.contains("[major]"), "{report}");
        assert!(report.contains("nm::observe/pull · Ir"), "{report}");
        assert!(
            report.contains("regression via change point · 100% confidence · 100 → 130"),
            "{report}"
        );
    }

    #[test]
    fn report_renders_direction_labels() {
        let set = discriminant_set();
        let mut improvement = regression();
        improvement.direction = Direction::Improvement;
        improvement.delta = -5.0;
        improvement.relative_delta = -0.05;
        let findings = vec![regression(), improvement];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);

        let text = render(&input, ReportFormat::Text, false);
        assert!(text.contains("regression via change point"), "{text}");
        assert!(text.contains("improvement via change point"), "{text}");

        let markdown = render(&input, ReportFormat::Markdown, false);
        assert!(markdown.contains("| +30.00% | regression |"), "{markdown}");
        assert!(markdown.contains("| -5.00% | improvement |"), "{markdown}");

        // The per-set JSON tallies count each direction independently: this set
        // holds one regression and one improvement.
        let json = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("report is JSON");
        let set_json = &parsed["sets"][0];
        assert_eq!(set_json["regressions"], 1, "{json}");
        assert_eq!(set_json["improvements"], 1, "{json}");
    }

    #[test]
    fn json_per_set_tally_counts_each_direction_independently() {
        // Two regressions and no improvements in one set: the per-set JSON tally
        // must report the real counts, not a constant.
        let set = discriminant_set();
        let mut second = regression();
        second.id = BenchmarkId::new(
            Some("nm".to_owned()),
            "nm::other".to_owned(),
            Some("push".to_owned()),
            None,
        );
        let findings = vec![regression(), second];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let json = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("report is JSON");
        let set_json = &parsed["sets"][0];
        assert_eq!(set_json["regressions"], 2, "{json}");
        assert_eq!(set_json["improvements"], 0, "{json}");
    }

    #[test]
    fn markdown_report_renders_a_table_per_set() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let report = render(&input, ReportFormat::Markdown, false);
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
        assert!(report.contains("| Change | Direction |"), "{report}");
        assert!(report.contains("| +30.00% | regression |"), "{report}");
    }

    #[test]
    fn markdown_report_with_no_findings() {
        let input = ReportInput {
            project: "folo",
            mode: "history",
            notable: false,
            runs: 0,
            series: 0,
            findings: &[],
            sets: &[],
            hint: Some("Found 2 stored runs ... commit your working tree"),
            warning: None,
        };
        let report = render(&input, ReportFormat::Markdown, false);
        assert!(report.contains("No notable changes detected."), "{report}");
        assert!(report.contains("commit your working tree"), "{report}");
    }

    #[test]
    fn json_report_includes_hint_field_when_present() {
        let input = ReportInput {
            project: "folo",
            mode: "history",
            notable: false,
            runs: 0,
            series: 0,
            findings: &[],
            sets: &[],
            hint: Some("dirty snapshots on base-branch commits"),
            warning: None,
        };
        let report = render(&input, ReportFormat::Json, false);
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

        let text = render(&input, ReportFormat::Text, false);
        assert!(text.trim_end().ends_with("(ephemeral)."), "{text}");

        let markdown = render(&input, ReportFormat::Markdown, false);
        assert!(markdown.trim_end().ends_with("(ephemeral)."), "{markdown}");

        let json = render(&input, ReportFormat::Json, false);
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
            mode: "history",
            notable: false,
            runs: 1,
            series: 1,
            findings: &[],
            sets: &[],
            hint: None,
            warning: Some("Warning: dirty runs were included."),
        };
        let text = render(&input, ReportFormat::Text, false);
        assert!(text.contains("No notable changes detected."), "{text}");
        assert!(text.trim_end().ends_with("included."), "{text}");

        let markdown = render(&input, ReportFormat::Markdown, false);
        assert!(markdown.trim_end().ends_with("included."), "{markdown}");
    }

    #[test]
    fn omitted_warning_is_absent_from_json() {
        let input = ReportInput {
            project: "folo",
            mode: "history",
            notable: false,
            runs: 0,
            series: 0,
            findings: &[],
            sets: &[],
            hint: None,
            warning: None,
        };
        let report = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&report).expect("report is JSON");
        assert!(parsed.get("warning").is_none(), "{report}");
    }

    #[test]
    fn json_report_is_structured() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let report = render(&input, ReportFormat::Json, false);
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
    fn format_value_keeps_four_significant_figures() {
        // Counting integer-part digits toward the four significant figures.
        assert_eq!(format_value(0.209_703_243_360_777_45), "0.2097");
        assert_eq!(format_value(96.766_413_608_934_1), "96.77");
        assert_eq!(format_value(507.428_215_753_575_5), "507.4");
        // Sub-decimal values keep all four significant digits past the zeros.
        assert_eq!(format_value(0.000_001_234), "0.000001234");
        // A large value drops its fraction entirely; the integer part is never cut.
        assert_eq!(format_value(1_234_567.89), "1234568");
        // Trailing zeros are trimmed.
        assert_eq!(format_value(0.25), "0.25");
    }

    /// Builds a regression finding whose series steps up over four commits, so a
    /// history-mode chart has enough points to draw.
    fn regression_with_series() -> Finding {
        let mut finding = regression();
        finding.series = vec![
            SeriesValue {
                commit: Some("c0".to_owned()),
                value: 100.0,
                dirty: false,
            },
            SeriesValue {
                commit: Some("c1".to_owned()),
                value: 100.0,
                dirty: false,
            },
            SeriesValue {
                commit: Some("c2".to_owned()),
                value: 130.0,
                dirty: false,
            },
            SeriesValue {
                commit: Some("c3".to_owned()),
                value: 130.0,
                dirty: false,
            },
        ];
        finding
    }

    #[test]
    fn history_mode_text_draws_a_chart() {
        let set = discriminant_set();
        let findings = vec![regression_with_series()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.mode = "history";
        let text = render(&input, ReportFormat::Text, false);
        // The rasciigraph axis marker proves a chart was drawn under the finding.
        assert!(text.contains('┤') || text.contains('┼'), "{text}");
    }

    #[test]
    fn branch_mode_text_has_no_chart() {
        let set = discriminant_set();
        let findings = vec![regression_with_series()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.mode = "branch";
        let text = render(&input, ReportFormat::Text, false);
        assert!(!text.contains('┤') && !text.contains('┼'), "{text}");
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

    #[test]
    fn chart_of_needs_at_least_two_points() {
        // Deterministic, Miri-safe color state for the rasciigraph plot.
        colored::control::set_override(false);
        let point = |value: f64| SeriesValue {
            commit: None,
            value,
            dirty: false,
        };
        // A single point cannot be plotted.
        assert!(chart_of(&[point(1.0)], Direction::Regression, 0).is_none());
        // Exactly two points is the minimum that plots (a `< 2` -> `<= 2` slip
        // would reject it).
        assert!(chart_of(&[point(1.0), point(2.0)], Direction::Regression, 0).is_some());
    }

    #[test]
    fn covers_whole_series_only_at_the_boundaries() {
        // `active_from == 0` (everything active) and `active_from >= len` (nothing
        // to grey) both render a single series; anything in between splits.
        assert!(covers_whole_series(0, 4), "zero covers the whole series");
        assert!(covers_whole_series(4, 4), "the end covers the whole series");
        assert!(
            covers_whole_series(5, 4),
            "past the end covers the whole series"
        );
        assert!(
            !covers_whole_series(2, 4),
            "a mid split does not cover the whole series"
        );
        assert!(
            !covers_whole_series(1, 4),
            "a near-start split does not cover the whole series"
        );
    }

    /// Compares two `f64` slices treating `NaN` as equal to `NaN`, so masked
    /// (gap) positions can be asserted exactly.
    fn masks_equal(actual: &[f64], expected: &[f64]) -> bool {
        actual.len() == expected.len()
            && actual
                .iter()
                .zip(expected)
                .all(|(a, b)| (a.is_nan() && b.is_nan()) || (a - b).abs() < f64::EPSILON)
    }

    #[test]
    fn grey_prefix_keeps_up_to_the_boundary_and_gaps_the_rest() {
        let values = [0.0, 10.0, 20.0, 30.0];
        // Points after the boundary become gaps; the boundary itself is kept so
        // the grey line meets the active line.
        assert!(
            masks_equal(&grey_prefix(&values, 2), &[0.0, 10.0, 20.0, f64::NAN]),
            "{:?}",
            grey_prefix(&values, 2)
        );
        assert!(
            masks_equal(
                &grey_prefix(&values, 0),
                &[0.0, f64::NAN, f64::NAN, f64::NAN]
            ),
            "{:?}",
            grey_prefix(&values, 0)
        );
    }

    #[test]
    fn active_window_gaps_before_the_boundary_and_keeps_the_rest() {
        let values = [0.0, 10.0, 20.0, 30.0];
        // Points before the boundary become gaps; from the boundary on they are
        // kept (the boundary is shared with the grey prefix).
        assert!(
            masks_equal(
                &active_window(&values, 2),
                &[f64::NAN, f64::NAN, 20.0, 30.0]
            ),
            "{:?}",
            active_window(&values, 2)
        );
        assert!(
            masks_equal(
                &active_window(&values, 3),
                &[f64::NAN, f64::NAN, f64::NAN, 30.0]
            ),
            "{:?}",
            active_window(&values, 3)
        );
    }

    #[test]
    fn significant_decimals_handles_a_value_below_the_search_floor() {
        // A magnitude smaller than 10^-12 never satisfies the exponent search, so
        // the exponent stays at its -12 floor, yielding 15 decimal places. This
        // pins the sign of the `-12` initializer.
        assert_eq!(significant_decimals(1e-13), 15);
    }
}
