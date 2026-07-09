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

use std::num::NonZero;

use cbh_detect::{Direction, Finding, FindingMethod, SeriesValue, short_commit};
use cbh_model::{BenchmarkId, DiscriminantSet};
use colored::{Color, Colorize};
use rasciigraph::{Config, plot_colored, plot_many_colored};
use serde::Serialize;

/// Height, in rows, of a history-mode finding chart.
const CHART_HEIGHT: u32 = 4;
/// Width, in columns, of a history-mode finding chart.
const CHART_WIDTH: u32 = 48;

/// The number of findings a Markdown summary retains by default.
///
/// Enough to convey the most significant movers while keeping the rendered report
/// comfortably within a GitHub issue body's size limit even when an analysis flags
/// many changes.
pub const DEFAULT_SUMMARY_LIMIT: NonZero<usize> = NonZero::new(20).expect("20 is non-zero");

/// The selectable output format of an analysis report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportFormat {
    /// A compact, human-readable plain-text summary.
    Text,
    /// A machine-readable JSON document.
    Json,
    /// A Markdown summary mirroring the text report, with charts as fenced blocks.
    Markdown,
}

impl ReportFormat {
    /// Parses a format from its command-line name, if recognized.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
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
pub struct SetSummary<'a> {
    /// The comparable partition this slice covers.
    pub set: &'a DiscriminantSet,
    /// Number of stored runs loaded for this set.
    pub runs: usize,
    /// Number of distinct series compared in this set.
    pub series: usize,
    /// The set's findings, in the same global ranking as the top level.
    pub findings: Vec<&'a Finding>,
}

/// The inputs a report is rendered from.
#[derive(Clone, Debug)]
pub struct ReportInput<'a> {
    /// The project the history belongs to.
    pub project: &'a str,
    /// The commit the analysis was run against (the resolved `--context`, HEAD by
    /// default) — the tip whose line of history the report describes, so a reader
    /// can identify exactly which state the findings pertain to.
    pub tip_commit: &'a str,
    /// Whether the working tree carried uncommitted changes when the analysis ran.
    /// When set, the tip is annotated `+ uncommitted changes` because the analyzed
    /// checkout differs from the committed tip. False for a clean tree (the CI
    /// collection case).
    pub tip_dirty: bool,
    /// The analysis mode the report was produced in (`history`/`branch`).
    pub mode: &'a str,
    /// Whether any finding survived — the at-a-glance signal a downstream
    /// automation reads to decide whether the report is worth surfacing.
    pub notable: bool,
    /// Total stored runs loaded across every set.
    pub runs: usize,
    /// Total distinct series compared across every set. Carried for the JSON report;
    /// the text and Markdown reports omit it as an uninformative tally.
    pub series: usize,
    /// The oldest and newest analyzed commit (full SHAs, first-parent order), so the
    /// header can state the span of history the analysis covered. `None` when no run
    /// entered the analysis.
    pub commit_span: Option<(&'a str, &'a str)>,
    /// Whether this analysis reports improvements. When `false` (history mode's
    /// default regressions-only watch) the text and Markdown reports
    /// omit the improvement tally, which would always be zero. The JSON report always
    /// carries it.
    pub report_improvements: bool,
    /// Every set's findings, globally ranked most-notable first.
    pub findings: &'a [Finding],
    /// The per-set breakdown, one entry per set that contributed data.
    pub sets: &'a [SetSummary<'a>],
    /// A diagnostic hint shown when stored runs existed but none were analyzed,
    /// explaining why the outcome is empty. Absent in the normal case.
    pub hint: Option<&'a str>,
    /// A warning shown when the analysis admitted dirty runs on the base branch's
    /// tip (the working-tree-dirty exception). Absent in the normal case.
    pub warning: Option<&'a str>,
}

/// The JSON shape of a per-set slice.
///
/// Carries only the partition identity and its tallies — the cheap metadata that
/// mirrors the per-set header the text and Markdown reports print. The findings
/// themselves live once in the top-level [`JsonReport::findings`] list (each names
/// its own set), so the document never duplicates a finding per set.
#[derive(Serialize)]
struct JsonSet<'a> {
    /// Engine identifier.
    engine: &'a str,
    /// Resolved target triple.
    target_triple: &'a str,
    /// Machine key (`synthetic` for hardware-independent engines).
    machine_key: &'a str,
    /// Stored runs loaded for this set.
    runs: usize,
    /// Distinct series compared in this set.
    series: usize,
    /// Flagged regressions in this set.
    regressions: usize,
    /// Flagged improvements in this set.
    improvements: usize,
}

/// The JSON shape of one finding: the machine-readable form of a text-report
/// finding paragraph.
///
/// It carries exactly the data the text and Markdown reports show — the partition,
/// the benchmark identity, the metric, the detected move, and the provenance — with
/// no underlying series (the text chart is a presentation concern, not data a
/// consumer reconstructs) and full `f64` precision (the human reports round).
#[derive(Serialize)]
struct JsonFinding<'a> {
    /// The comparable discriminant set, inlined as `engine`/`target_triple`/
    /// `machine_key` so the flat list is self-describing.
    #[serde(flatten)]
    set: &'a DiscriminantSet,
    /// The benchmark identity, inlined as `segments`.
    #[serde(flatten)]
    id: &'a BenchmarkId,
    /// The metric kind that moved (its `snake_case` wire name).
    kind: &'static str,
    /// Which detector produced the finding.
    method: FindingMethod,
    /// Whether the move is a regression or an improvement.
    direction: Direction,
    /// The before-regime representative value.
    baseline: f64,
    /// The after-regime representative value.
    latest: f64,
    /// The change relative to the baseline (`(latest - baseline) / baseline`).
    relative_delta: f64,
    /// The detector's confidence (`1 - p_value`), approaching `1.0` as the change
    /// becomes statistically unambiguous.
    confidence: f64,
    /// Commit the change is attributed to, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<&'a str>,
    /// Whether the change is still reflected in the latest measured state.
    active: bool,
    /// Where, within a branch, the latest regime began (branch mode) or where the
    /// level recovered (history + inactive). Present only when located.
    #[serde(skip_serializing_if = "Option::is_none")]
    flipped_at: Option<&'a str>,
    /// Abbreviated commit of the blessing that re-baselined the series, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    blessed_at: Option<&'a str>,
    /// Effective (committer) time of the blessed commit, RFC 3339, if blessed.
    #[serde(skip_serializing_if = "Option::is_none")]
    blessed_commit_time: Option<&'a str>,
}

impl<'a> JsonFinding<'a> {
    /// Projects a [`Finding`] onto its cheap, series-free JSON shape.
    fn from_finding(finding: &'a Finding) -> Self {
        Self {
            set: &finding.set,
            id: &finding.id,
            kind: finding.kind.as_str(),
            method: finding.method,
            direction: finding.direction,
            baseline: finding.baseline,
            latest: finding.latest,
            relative_delta: finding.relative_delta,
            confidence: finding.confidence,
            commit: finding.commit.as_deref(),
            active: finding.active,
            flipped_at: finding.flipped_at.as_deref(),
            blessed_at: finding.blessed_at.as_deref(),
            blessed_commit_time: finding.blessed_commit_time.as_deref(),
        }
    }
}

/// The JSON shape of a rendered report.
#[derive(Serialize)]
struct JsonReport<'a> {
    /// The project the history belongs to.
    project: &'a str,
    /// The commit the analysis was run against (resolved `--context`, HEAD by
    /// default): the full commit ID, so a consumer can link the report to a commit.
    tip_commit: &'a str,
    /// Whether the working tree carried uncommitted changes when the analysis ran.
    tip_dirty: bool,
    /// The analysis mode (`history`/`branch`).
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
    /// Every finding, globally ranked most-notable first; each names its own set.
    findings: Vec<JsonFinding<'a>>,
    /// The per-set breakdown (partition identity and tallies only).
    sets: Vec<JsonSet<'a>>,
}

/// Renders `input` in the requested `format`.
///
/// `color` enables ANSI styling in the text format (the headline percentage and
/// the per-finding chart). The caller decides it from the output terminal so tests
/// and pipes stay plain; `markdown` and `json` ignore it.
#[must_use]
pub fn render(input: &ReportInput<'_>, format: ReportFormat, color: bool) -> String {
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

/// A one-line label for a set, naming its `engine / triple / machine` partition.
fn set_label(set: &DiscriminantSet) -> String {
    set.to_string()
}

/// The `analyze` facet flags that select exactly this discriminant set, ready to
/// paste into a follow-up query. Naming every facet explicitly pins the one set, so a
/// reader who spots a finding can drill into that partition without having to guess
/// which engine / triple / machine it came from.
fn set_filter_flags(set: &DiscriminantSet) -> String {
    format!(
        "--engine {} --target-triple {} --machine-key {}",
        set.engine, set.target_triple, set.machine_key
    )
}

/// The commit label shared by the text and Markdown headers: the analyzed tip
/// commit, annotated `+ uncommitted changes` when the working tree carried
/// uncommitted changes so a reader knows the analyzed checkout differed from the
/// committed tip.
fn tip_label(commit: &str, dirty: bool) -> String {
    if dirty {
        format!("{commit} + uncommitted changes")
    } else {
        commit.to_owned()
    }
}

/// Forces `colored`'s process-global override to `value` until dropped, then
/// restores ambient auto-detection. Bundling the set with the matching restore keeps
/// the override scoped to a single render call so it can never leak into later output,
/// even on an early return or panic.
struct ColorOverride;

impl ColorOverride {
    #[must_use]
    fn force(value: bool) -> Self {
        colored::control::set_override(value);
        Self
    }
}

impl Drop for ColorOverride {
    // Restoring `colored`'s process-global override is exercised by every render test
    // (each builds and drops a guard), but asserting it requires observing that global
    // state, which races with the rest of the parallel test suite and which `colored`
    // exposes no override-state getter to read deterministically. Skipped for mutation
    // only; the restore itself is covered behaviourally.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        colored::control::unset_override();
    }
}

fn render_text(input: &ReportInput<'_>, color: bool) -> String {
    // Force `colored` and `rasciigraph` to honor this explicit decision rather than
    // their own ambient terminal auto-detection, so tests and pipes are deterministic
    // regardless of how the process is run. The guard restores auto-detection on return.
    let _color = ColorOverride::force(color);

    let regressions = count_top(input.findings, Direction::Regression);

    let mut header = vec![
        format!("runs: {}", runs_with_span(input.runs, input.commit_span)),
        format!("regressions: {regressions}"),
    ];
    if input.report_improvements {
        header.push(format!(
            "improvements: {}",
            count_top(input.findings, Direction::Improvement)
        ));
    }

    let mut lines = vec![
        format!("Analyzed project {} ({} mode)", input.project, input.mode),
        format!("  commit: {}", tip_label(input.tip_commit, input.tip_dirty)),
        format!("  {}", header.join("  ")),
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
    // branch mode compares against a baseline rather than walking a series.
    let chart_enabled = input.mode == "history";
    for summary in input.sets {
        if summary.findings.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(set_label(summary.set));
        lines.push(set_counts_line(summary, input.report_improvements));
        lines.push(format!("  filter: {}", set_filter_flags(summary.set)));
        for finding in &summary.findings {
            push_finding_block(&mut lines, finding, chart_enabled);
        }
    }
    push_warning(&mut lines, input.warning);
    finish(&lines)
}

/// Appends one finding as a paragraph: the benchmark id on its own line as a
/// chapter title, then a direction-colored `percentage metric (confidence)`
/// headline, a dimmed detail line, an optional blessing/recovery note, and (in
/// history mode) a chart of the metric over commits.
fn push_finding_block(lines: &mut Vec<String>, finding: &Finding, chart_enabled: bool) {
    lines.push(String::new());

    // Lead with the benchmark id on its own line, like a chapter title: some ids are
    // long, so a dedicated line keeps the change headline that follows readable.
    lines.push(describe_id(&finding.id).bold().to_string());

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
        "  {headline} {} ({} confidence){status}",
        finding.kind.as_str(),
        format_confidence(finding.confidence),
    ));

    lines.push(format!("    {}", detail_text(finding)).dimmed().to_string());

    // Name the blessing that re-baselined the series, so the reader knows the
    // history before it is intentionally excluded from detection.
    if let Some(blessing) = blessing_text(finding) {
        lines.push(format!("    {blessing}").dimmed().to_string());
    }

    if chart_enabled
        && let Some(chart) = chart_of(&finding.series, finding.direction, finding.active_from)
    {
        lines.push(chart);
    }
}

/// The per-set summary line — the cheap tally the JSON `sets` block carries, shown
/// under each set header so the text and Markdown reports surface it too. The
/// improvement tally is omitted when the analysis does not report improvements.
fn set_counts_line(summary: &SetSummary<'_>, report_improvements: bool) -> String {
    let mut fields = vec![
        format!("runs: {}", summary.runs),
        format!(
            "regressions: {}",
            count_direction(&summary.findings, Direction::Regression)
        ),
    ];
    if report_improvements {
        fields.push(format!(
            "improvements: {}",
            count_direction(&summary.findings, Direction::Improvement)
        ));
    }
    format!("  {}", fields.join("  "))
}

/// Formats the run tally with the analyzed commit span appended, so the report
/// header states both how many runs entered the analysis and the stretch of history
/// they cover. A single analyzed commit collapses the range to that one commit; with
/// no runs the count stands alone.
fn runs_with_span(runs: usize, span: Option<(&str, &str)>) -> String {
    match span {
        Some((first, last)) if first == last => format!("{runs} ({})", short_commit(first)),
        Some((first, last)) => {
            format!("{runs} ({} → {})", short_commit(first), short_commit(last))
        }
        None => runs.to_string(),
    }
}

/// The plain-text detail body shared by the text and Markdown reports: the
/// direction, detector, the `baseline → latest` move, the attributed commit, and
/// (when located) the flip/recovery commit. Confidence rides on the headline line
/// instead. Carries no styling and no leading indent; each format applies its own.
fn detail_text(finding: &Finding) -> String {
    let mut detail = format!(
        "{} via {} · {} → {} · @ {}",
        direction_label(finding.direction),
        method_label(finding.method),
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
    detail
}

/// The plain-text blessing/recovery note, when the series was re-baselined by a
/// blessing. Carries no styling and no leading indent.
fn blessing_text(finding: &Finding) -> Option<String> {
    let blessed_at = finding.blessed_at.as_deref()?;
    let date = finding
        .blessed_commit_time
        .as_deref()
        .map_or_else(String::new, |effective| format!(" ({effective})"));
    Some(format!("blessed at {blessed_at}{date}"))
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
    // Charts embed ANSI color when `colored` is active; force it off while rendering
    // so the fenced code blocks carry plain characters that render in any Markdown
    // viewer. The guard restores ambient auto-detection on return so this override
    // never leaks into later output in the same process.
    let _color = ColorOverride::force(false);

    let regressions = count_top(input.findings, Direction::Regression);

    let mut lines = vec![
        format!("# Benchmark history analysis: {}", input.project),
        String::new(),
        format!("- Commit: {}", tip_label(input.tip_commit, input.tip_dirty)),
        format!("- Mode: {}", input.mode),
        format!(
            "- Runs analyzed: {}",
            runs_with_span(input.runs, input.commit_span)
        ),
        format!("- Regressions: {regressions}"),
    ];
    if input.report_improvements {
        lines.push(format!(
            "- Improvements: {}",
            count_top(input.findings, Direction::Improvement)
        ));
    }

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

    // A per-commit chart is meaningful only for a history timeline, matching the
    // text report; branch mode compares against a baseline.
    let chart_enabled = input.mode == "history";
    for summary in input.sets {
        if summary.findings.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(format!("## {}", set_label(summary.set)));
        lines.push(String::new());
        lines.push(format!("- Runs: {}", summary.runs));
        lines.push(format!(
            "- Regressions: {}",
            count_direction(&summary.findings, Direction::Regression)
        ));
        if input.report_improvements {
            lines.push(format!(
                "- Improvements: {}",
                count_direction(&summary.findings, Direction::Improvement)
            ));
        }
        lines.push(format!("- Filter: `{}`", set_filter_flags(summary.set)));
        for finding in &summary.findings {
            push_finding_markdown(&mut lines, finding, "###", chart_enabled);
        }
    }
    push_warning(&mut lines, input.warning);
    finish(&lines)
}

/// Renders a condensed Markdown report carrying only the `limit` most significant
/// findings, so a large analysis still fits within a downstream size limit (a GitHub
/// issue body caps at 65,536 characters).
///
/// The header repeats the full Markdown report's totals — computed from *every*
/// finding, not the retained subset — then a flat, globally-ranked list of the top
/// `limit` findings follows. The per-set breakdown the full Markdown report groups by
/// is deliberately dropped: a summary is a single ranked list, and that grouping is the
/// main length driver. When findings were dropped, a note states how many of the
/// total are shown so a reader knows the report is partial and to consult the full
/// report for the rest.
#[must_use]
pub fn render_markdown_summary(input: &ReportInput<'_>, limit: NonZero<usize>) -> String {
    // Charts embed ANSI color when `colored` is active; force it off so the fenced
    // blocks carry plain characters, matching `render_markdown`. The guard restores
    // ambient auto-detection on return.
    let _color = ColorOverride::force(false);

    let regressions = count_top(input.findings, Direction::Regression);

    let mut lines = vec![
        format!("# Benchmark history analysis: {}", input.project),
        String::new(),
        format!("- Commit: {}", tip_label(input.tip_commit, input.tip_dirty)),
        format!("- Mode: {}", input.mode),
        format!(
            "- Runs analyzed: {}",
            runs_with_span(input.runs, input.commit_span)
        ),
        format!("- Regressions: {regressions}"),
    ];
    if input.report_improvements {
        lines.push(format!(
            "- Improvements: {}",
            count_top(input.findings, Direction::Improvement)
        ));
    }

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

    // The findings are already globally ranked by descending magnitude, so the leading
    // `limit` are the top movers. When any were dropped, name the total so a reader
    // knows to reach for the full report (the total exceeds `limit`, which is at least
    // one, so the total is at least two and "findings" is unconditionally plural).
    if input.findings.len() > limit.get() {
        lines.push(String::new());
        lines.push(format!(
            "> Showing the top {limit} of {} findings by magnitude.",
            input.findings.len()
        ));
    }

    // A per-commit chart is meaningful only for a history timeline, matching the full
    // reports; branch mode compares against a baseline.
    let chart_enabled = input.mode == "history";
    for finding in input.findings.iter().take(limit.get()) {
        push_finding_markdown(&mut lines, finding, "##", chart_enabled);
        push_set_filter_footer(&mut lines, &finding.set);
    }
    push_warning(&mut lines, input.warning);
    finish(&lines)
}

/// Appends the discriminant-set filter flags as a de-emphasized footer on a summary
/// finding, after its chart. The summary drops the per-set grouping to stay within a
/// downstream size cap, so without this a reader could not tell which partition a
/// finding came from — or, when the same benchmark moved in several sets, tell the
/// otherwise near-identical blocks apart. The flags trail the block rather than lead
/// it because they are reference material for a follow-up query, not the headline.
fn push_set_filter_footer(lines: &mut Vec<String>, set: &DiscriminantSet) {
    lines.push(String::new());
    lines.push(format!("_Filter:_ `{}`", set_filter_flags(set)));
}

/// Appends one finding as a Markdown block mirroring the text report: the benchmark
/// id as a `heading` (a chapter title), then a bold `percentage metric (confidence)`
/// line, the shared detail line, an optional blessing note, and (in history mode) the
/// metric chart in a fenced `text` block so it survives Markdown rendering. `heading`
/// carries the ATX prefix (`##`/`###`) so the block nests correctly — top-level in the
/// summary, one level under the set heading in the full report.
fn push_finding_markdown(
    lines: &mut Vec<String>,
    finding: &Finding,
    heading: &str,
    chart_enabled: bool,
) {
    lines.push(String::new());

    // The benchmark id is a heading of its own, so a long id reads as a chapter title
    // rather than crowding the change headline that follows.
    lines.push(format!("{heading} `{}`", describe_id(&finding.id)));

    let status = if finding.active {
        String::new()
    } else {
        " _(recovered)_".to_owned()
    };
    lines.push(format!(
        "**{}** `{}` ({} confidence){status}",
        format_percent(finding.relative_delta),
        finding.kind.as_str(),
        format_confidence(finding.confidence),
    ));

    lines.push(String::new());
    lines.push(detail_text(finding));

    if let Some(blessing) = blessing_text(finding) {
        lines.push(String::new());
        lines.push(blessing);
    }

    if chart_enabled
        && let Some(chart) = chart_of(&finding.series, finding.direction, finding.active_from)
    {
        lines.push(String::new());
        lines.push("```text".to_owned());
        lines.push(chart);
        lines.push("```".to_owned());
    }
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
            machine_key: &summary.set.machine_key,
            runs: summary.runs,
            series: summary.series,
            regressions: count_direction(&summary.findings, Direction::Regression),
            improvements: count_direction(&summary.findings, Direction::Improvement),
        })
        .collect();

    let report = JsonReport {
        project: input.project,
        tip_commit: input.tip_commit,
        tip_dirty: input.tip_dirty,
        mode: input.mode,
        notable: input.notable,
        runs: input.runs,
        series: input.series,
        regressions: count_top(input.findings, Direction::Regression),
        improvements: count_top(input.findings, Direction::Improvement),
        hint: input.hint,
        warning: input.warning,
        findings: input
            .findings
            .iter()
            .map(JsonFinding::from_finding)
            .collect(),
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
#[must_use]
pub fn format_value(value: f64) -> String {
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

    use cbh_model::MetricKind;
    use nonempty::nonempty;

    use super::*;

    fn discriminant_set() -> DiscriminantSet {
        DiscriminantSet {
            engine: "callgrind".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine_key: "synthetic".to_owned(),
        }
    }

    fn regression() -> Finding {
        Finding {
            set: discriminant_set(),
            id: BenchmarkId::new(nonempty![
                "nm".to_owned(),
                "nm::observe".to_owned(),
                "pull".to_owned(),
            ]),
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
            blessed_commit_time: None,
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
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: !findings.is_empty(),
            runs: findings.len().saturating_add(3),
            series: findings.len().max(1),
            commit_span: None,
            report_improvements: true,
            findings,
            sets: summaries,
            hint: None,
            warning: None,
        }
    }

    /// Wraps a findings slice into a report with no per-set breakdown, for the
    /// summary tests (which render a flat, globally-ranked list and never read
    /// `sets`). Regressions only, so the header carries a single tally.
    fn flat_input(findings: &[Finding]) -> ReportInput<'_> {
        ReportInput {
            project: "folo",
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: !findings.is_empty(),
            runs: findings.len().saturating_add(3),
            series: findings.len().max(1),
            commit_span: None,
            report_improvements: false,
            findings,
            sets: &[],
            hint: None,
            warning: None,
        }
    }

    /// A regression finding named `name` (a single-segment id) with the given
    /// relative move, so a summary test can tell which findings the render kept by
    /// their distinctive ids and magnitudes.
    fn named_regression(name: &str, relative_delta: f64) -> Finding {
        Finding {
            id: BenchmarkId::new(nonempty![name.to_owned()]),
            relative_delta,
            ..regression()
        }
    }

    /// An improvement finding named `name` with the given (negative) relative move, so
    /// a summary test can exercise the optional improvements tally alongside
    /// regressions.
    fn named_improvement(name: &str, relative_delta: f64) -> Finding {
        Finding {
            id: BenchmarkId::new(nonempty![name.to_owned()]),
            direction: Direction::Improvement,
            relative_delta,
            ..regression()
        }
    }

    /// Five regressions in the descending-magnitude order the ranking produces, so a
    /// summary render can cap the leading few and drop the tail.
    fn ranked_five() -> Vec<Finding> {
        vec![
            named_regression("mover_a", 0.50),
            named_regression("mover_b", 0.40),
            named_regression("mover_c", 0.30),
            named_regression("dropped_d", 0.20),
            named_regression("dropped_e", 0.10),
        ]
    }

    #[test]
    fn markdown_summary_caps_to_the_limit_and_keeps_full_totals() {
        let findings = ranked_five();
        let input = flat_input(&findings);

        let report = render_markdown_summary(&input, NonZero::new(3).unwrap());

        // The header tally counts every finding, not the retained subset.
        assert!(report.contains("- Regressions: 5"), "{report}");
        // The truncation note names how many of the total are shown.
        assert!(
            report.contains("> Showing the top 3 of 5 findings by magnitude."),
            "{report}"
        );
        // The three largest movers are kept; the two smallest are dropped.
        assert!(report.contains("mover_a"), "{report}");
        assert!(report.contains("mover_b"), "{report}");
        assert!(report.contains("mover_c"), "{report}");
        assert!(!report.contains("dropped_d"), "{report}");
        assert!(!report.contains("dropped_e"), "{report}");
        // The per-set breakdown the full Markdown report groups by is dropped: a
        // summary is a single flat list, so no `## engine/triple/machine` heading.
        assert!(!report.contains("## callgrind"), "{report}");
    }

    #[test]
    fn markdown_summary_omits_the_note_when_within_the_limit() {
        let findings = ranked_five();
        let input = flat_input(&findings);

        // A limit at or above the finding count keeps every finding and shows no
        // truncation note.
        let report = render_markdown_summary(&input, NonZero::new(5).unwrap());

        assert!(!report.contains("Showing the top"), "{report}");
        assert!(report.contains("mover_a"), "{report}");
        assert!(report.contains("dropped_e"), "{report}");

        // A limit beyond the count behaves the same as an exact fit.
        let generous = render_markdown_summary(&input, NonZero::new(20).unwrap());
        assert_eq!(generous, report);
    }

    #[test]
    fn markdown_summary_with_no_findings_matches_the_empty_message() {
        let input = ReportInput {
            hint: Some("Found 2 stored runs ... dirty snapshots"),
            ..flat_input(&[])
        };

        let report = render_markdown_summary(&input, DEFAULT_SUMMARY_LIMIT);

        assert!(report.contains("No notable changes detected."), "{report}");
        assert!(report.contains("Found 2 stored runs"), "{report}");
        assert!(!report.contains("Showing the top"), "{report}");
    }

    #[test]
    fn markdown_summary_draws_a_fenced_chart_in_history_mode() {
        // `flat_input` fixes the mode to `history`, where a retained finding with a
        // series is rendered with its per-commit chart — so the summary reuses the full
        // report's chart-in-history behaviour rather than dropping it.
        let findings = vec![regression_with_series()];
        let input = flat_input(&findings);

        let report = render_markdown_summary(&input, DEFAULT_SUMMARY_LIMIT);

        // The chart sits inside a fenced `text` block (its axis marker proves a chart was
        // drawn) and carries no ANSI escapes.
        assert!(report.contains("```text"), "{report}");
        assert!(report.contains('┤') || report.contains('┼'), "{report}");
        assert!(!report.contains('\u{1b}'), "{report}");
        // The set-filter footer trails the fenced chart, not precedes it: it is
        // reference material for a follow-up query, not the headline.
        let chart_close = report.rfind("```").expect("fenced chart present");
        let footer_at = report.find("_Filter:_").expect("filter footer present");
        assert!(
            footer_at > chart_close,
            "the filter footer must follow the chart: {report}"
        );
    }

    #[test]
    fn markdown_summary_footers_each_finding_with_its_set_filter() {
        // The summary drops the per-set grouping, so each finding carries the facet
        // flags that isolate its partition as a trailing footer — enough to query that
        // exact set without leading the block.
        let findings = vec![named_regression("mover_a", 0.50)];
        let input = flat_input(&findings);

        let report = render_markdown_summary(&input, DEFAULT_SUMMARY_LIMIT);

        let footer = "_Filter:_ `--engine callgrind --target-triple x86_64-unknown-linux-gnu --machine-key synthetic`";
        assert!(report.contains(footer), "{report}");
        // The footer trails the finding headline rather than leading it.
        let headline_at = report.find("mover_a").expect("headline present");
        let footer_at = report.find(footer).expect("footer present");
        assert!(
            footer_at > headline_at,
            "footer must follow the finding: {report}"
        );
    }

    #[test]
    fn markdown_summary_distinguishes_the_same_benchmark_across_sets() {
        // The same benchmark id regresses in two discriminant sets. Without the set
        // footer their flat summary blocks would be indistinguishable; with it, each
        // names the filter that isolates its own partition.
        let linux = named_regression("shared", 0.50);
        let windows = Finding {
            set: DiscriminantSet {
                engine: "criterion".to_owned(),
                target_triple: "x86_64-pc-windows-msvc".to_owned(),
                machine_key: "m1".to_owned(),
            },
            ..named_regression("shared", 0.40)
        };
        let findings = vec![linux, windows];
        let input = flat_input(&findings);

        let report = render_markdown_summary(&input, DEFAULT_SUMMARY_LIMIT);

        assert!(
            report.contains(
                "--engine callgrind --target-triple x86_64-unknown-linux-gnu --machine-key synthetic"
            ),
            "{report}"
        );
        assert!(
            report.contains(
                "--engine criterion --target-triple x86_64-pc-windows-msvc --machine-key m1"
            ),
            "{report}"
        );
    }

    #[test]
    fn markdown_summary_reports_improvements_only_when_enabled() {
        // Magnitudes descend so the list already reads as globally ranked; two
        // regressions and three improvements are interleaved.
        let findings = vec![
            named_regression("reg_a", 0.50),
            named_improvement("imp_b", -0.40),
            named_regression("reg_c", 0.30),
            named_improvement("imp_d", -0.20),
            named_improvement("imp_e", -0.10),
        ];

        // Disabled (the `flat_input` default): the header carries no improvements tally.
        let without = render_markdown_summary(&flat_input(&findings), NonZero::new(2).unwrap());
        assert!(!without.contains("Improvements:"), "{without}");

        // Enabled: the header carries an improvements tally counted from *every*
        // finding — like the regressions tally — even though the cap of two drops
        // `imp_d` and `imp_e` from the rendered list.
        let input = ReportInput {
            report_improvements: true,
            ..flat_input(&findings)
        };
        let with = render_markdown_summary(&input, NonZero::new(2).unwrap());
        assert!(with.contains("- Regressions: 2"), "{with}");
        assert!(with.contains("- Improvements: 3"), "{with}");
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
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: false,
            runs: 3,
            series: 1,
            commit_span: None,
            report_improvements: false,
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
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: false,
            runs: 0,
            series: 0,
            commit_span: None,
            report_improvements: false,
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
        assert!(
            report.contains("callgrind/x86_64-unknown-linux-gnu/synthetic"),
            "the set heading drops the redundant `Set ` prefix: {report}"
        );
        // The set header names the facet flags that reproduce exactly this partition,
        // so a reader who spots a finding knows how to query it directly.
        assert!(
            report.contains(
                "  filter: --engine callgrind --target-triple x86_64-unknown-linux-gnu --machine-key synthetic"
            ),
            "{report}"
        );
        assert!(report.contains("+30.00%"), "{report}");
        assert!(!report.contains("[major]"), "{report}");
        // The benchmark id leads on its own chapter-title line; the change headline
        // that follows carries the metric and confidence, no longer the id.
        assert!(report.contains("nm/nm::observe/pull"), "{report}");
        assert!(
            report.contains("+30.00% instruction_count (100% confidence)"),
            "{report}"
        );
        // Confidence rides on the headline now, so the detail line drops it.
        assert!(
            report.contains("regression via change point · 100 → 130"),
            "{report}"
        );
        assert!(!report.contains("100% confidence · 100"), "{report}");
    }

    #[test]
    fn text_report_shows_per_set_counts_distinct_from_totals() {
        // Give the set tallies that differ from the top-level aggregate so the per-set
        // counts line is identifiable on its own — a blanked-out line would no longer
        // match, unlike when the set and total counts coincide.
        let set = discriminant_set();
        let findings = vec![regression()];
        let summaries = vec![SetSummary {
            set: &set,
            runs: 7,
            series: 5,
            findings: findings.iter().collect(),
        }];
        let input = ReportInput {
            project: "folo",
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: true,
            runs: 99,
            series: 88,
            commit_span: None,
            report_improvements: true,
            findings: &findings,
            sets: &summaries,
            hint: None,
            warning: None,
        };
        let report = render(&input, ReportFormat::Text, false);
        // The per-set counts line carries the set's own tallies, distinct from the
        // top-level totals (`runs: 99`). The series count is no longer surfaced.
        assert!(
            report.contains("  runs: 7  regressions: 1  improvements: 0"),
            "{report}"
        );
        assert!(!report.contains("series:"), "{report}");
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
        assert!(
            markdown.contains("regression via change point"),
            "{markdown}"
        );
        assert!(
            markdown.contains("improvement via change point"),
            "{markdown}"
        );
        assert!(markdown.contains("**+30.00%**"), "{markdown}");
        assert!(markdown.contains("**-5.00%**"), "{markdown}");

        // The per-set JSON tallies count each direction independently: this set
        // holds one regression and one improvement.
        let json = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let set_json = &parsed["sets"][0];
        assert_eq!(set_json["regressions"], 1, "{json}");
        assert_eq!(set_json["improvements"], 1, "{json}");
    }

    #[test]
    fn text_report_marks_an_inactive_recovered_finding() {
        let set = discriminant_set();
        let mut recovered = regression();
        recovered.active = false;
        recovered.flipped_at = Some("c4".to_owned());
        let findings = vec![recovered];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let report = render(&input, ReportFormat::Text, false);
        assert!(report.contains("(recovered)"), "{report}");
        assert!(report.contains("recovers at c4"), "{report}");
    }

    #[test]
    fn text_report_annotates_a_blessed_finding() {
        let set = discriminant_set();
        let mut blessed = regression();
        blessed.blessed_at = Some("c3".to_owned());
        blessed.blessed_commit_time = Some("2024-01-01T00:00:00Z".to_owned());
        let findings = vec![blessed];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let report = render(&input, ReportFormat::Text, false);
        assert!(
            report.contains("blessed at c3 (2024-01-01T00:00:00Z)"),
            "{report}"
        );
    }

    #[test]
    fn json_per_set_tally_counts_each_direction_independently() {
        // Two regressions and no improvements in one set: the per-set JSON tally
        // must report the real counts, not a constant.
        let set = discriminant_set();
        let mut second = regression();
        second.id = BenchmarkId::new(nonempty![
            "nm".to_owned(),
            "nm::other".to_owned(),
            "push".to_owned(),
        ]);
        let findings = vec![regression(), second];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let json = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let set_json = &parsed["sets"][0];
        assert_eq!(set_json["regressions"], 2, "{json}");
        assert_eq!(set_json["improvements"], 0, "{json}");
    }

    #[test]
    fn markdown_report_renders_a_block_per_set() {
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
            report.contains("## callgrind/x86_64-unknown-linux-gnu/synthetic"),
            "the set heading drops the redundant `Set ` prefix: {report}"
        );
        // The per-set tally mirrors the JSON metadata and the text header.
        assert!(report.contains("- Regressions: 1"), "{report}");
        // The set header names the facet flags that reproduce exactly this partition.
        assert!(
            report.contains(
                "- Filter: `--engine callgrind --target-triple x86_64-unknown-linux-gnu --machine-key synthetic`"
            ),
            "{report}"
        );
        // Findings render as heading + bold-headline blocks, not a table.
        assert!(!report.contains("| Change | Direction |"), "{report}");
        // The benchmark id is its own heading, nested one level under the set heading
        // (`##`), so it reads as a chapter title; the change headline follows.
        assert!(report.contains("### `nm/nm::observe/pull`"), "{report}");
        assert!(
            report.contains("**+30.00%** `instruction_count` (100% confidence)"),
            "{report}"
        );
        // The old inline em-dash headline is gone.
        assert!(!report.contains("—"), "{report}");
        // An active finding carries no recovered suffix.
        assert!(!report.contains("_(recovered)_"), "{report}");
        // Confidence rides on the headline now, so the detail line drops it.
        assert!(
            report.contains("regression via change point · 100 → 130"),
            "{report}"
        );
        assert!(!report.contains("100% confidence · 100"), "{report}");
    }

    #[test]
    fn markdown_report_marks_an_inactive_recovered_finding() {
        let set = discriminant_set();
        let mut recovered = regression();
        recovered.active = false;
        recovered.flipped_at = Some("c4".to_owned());
        let findings = vec![recovered];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let report = render(&input, ReportFormat::Markdown, false);
        // The headline suffix flags a recovered finding; the shared detail line names
        // the recovery commit.
        assert!(
            report.contains("`instruction_count` (100% confidence) _(recovered)_"),
            "{report}"
        );
        assert!(report.contains("recovers at c4"), "{report}");
    }

    #[test]
    fn markdown_report_annotates_a_blessed_finding() {
        let set = discriminant_set();
        let mut blessed = regression();
        blessed.blessed_at = Some("c3".to_owned());
        blessed.blessed_commit_time = Some("2024-01-01T00:00:00Z".to_owned());
        let findings = vec![blessed];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let report = render(&input, ReportFormat::Markdown, false);
        assert!(
            report.contains("blessed at c3 (2024-01-01T00:00:00Z)"),
            "{report}"
        );
    }

    #[test]
    fn markdown_history_mode_draws_a_fenced_chart() {
        let set = discriminant_set();
        let findings = vec![regression_with_series()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.mode = "history";
        let report = render(&input, ReportFormat::Markdown, false);
        // The chart sits inside a fenced `text` block and carries no ANSI escapes.
        assert!(report.contains("```text"), "{report}");
        assert!(report.contains('┤') || report.contains('┼'), "{report}");
        assert!(!report.contains('\u{1b}'), "{report}");
    }

    #[test]
    fn markdown_report_with_no_findings() {
        let input = ReportInput {
            project: "folo",
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: false,
            runs: 0,
            series: 0,
            commit_span: None,
            report_improvements: false,
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
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: false,
            runs: 0,
            series: 0,
            commit_span: None,
            report_improvements: false,
            findings: &[],
            sets: &[],
            hint: Some("dirty snapshots on base-branch commits"),
            warning: None,
        };
        let report = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
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
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["warning"], "Warning: analysis included dirty runs (ephemeral).",
            "{json}"
        );
    }

    #[test]
    fn warning_renders_even_when_there_are_no_findings() {
        let input = ReportInput {
            project: "folo",
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: false,
            runs: 1,
            series: 1,
            commit_span: None,
            report_improvements: false,
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
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: false,
            runs: 0,
            series: 0,
            commit_span: None,
            report_improvements: false,
            findings: &[],
            sets: &[],
            hint: None,
            warning: None,
        };
        let report = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
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
        assert_eq!(parsed["tip_commit"], "1234567890abcdef1234");
        assert_eq!(parsed["tip_dirty"], false);
        assert_eq!(parsed["regressions"], 1);
        assert_eq!(parsed["improvements"], 0);
        let finding = &parsed["findings"][0];
        // Flattened DiscriminantSet and BenchmarkId fields appear inline.
        assert_eq!(finding["engine"], "callgrind");
        assert_eq!(finding["segments"][0], "nm");
        assert_eq!(finding["segments"][1], "nm::observe");
        assert_eq!(finding["direction"], "regression");
        assert_eq!(finding["kind"], "instruction_count");
        // The bulky per-commit series is no longer carried: JSON mirrors the text
        // data, not the chart it draws from.
        assert!(finding.get("series").is_none(), "{report}");
        // The per-set breakdown carries the partition triple and tallies only — no
        // duplicated findings array.
        let set_json = &parsed["sets"][0];
        assert_eq!(set_json["engine"], "callgrind");
        assert_eq!(set_json["target_triple"], "x86_64-unknown-linux-gnu");
        assert_eq!(set_json["regressions"], 1);
        assert!(set_json.get("findings").is_none(), "{report}");
    }

    #[test]
    fn report_header_names_the_analyzed_tip_commit() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);

        // A clean tip is named without annotation in both human formats.
        let text = render(&input, ReportFormat::Text, false);
        assert!(text.contains("commit: 1234567890abcdef1234"), "{text}");
        assert!(
            !text.contains("uncommitted changes"),
            "a clean tip must not be annotated: {text}"
        );
        let markdown = render(&input, ReportFormat::Markdown, false);
        assert!(
            markdown.contains("- Commit: 1234567890abcdef1234"),
            "{markdown}"
        );
    }

    #[test]
    fn dirty_tip_is_annotated_with_uncommitted_changes() {
        let input = ReportInput {
            project: "folo",
            tip_commit: "1234567890abcdef1234",
            tip_dirty: true,
            mode: "history",
            notable: false,
            runs: 1,
            series: 1,
            commit_span: None,
            report_improvements: true,
            findings: &[],
            sets: &[],
            hint: None,
            warning: None,
        };

        let text = render(&input, ReportFormat::Text, false);
        assert!(
            text.contains("commit: 1234567890abcdef1234 + uncommitted changes"),
            "{text}"
        );
        let markdown = render(&input, ReportFormat::Markdown, false);
        assert!(
            markdown.contains("- Commit: 1234567890abcdef1234 + uncommitted changes"),
            "{markdown}"
        );
        let json = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["tip_commit"], "1234567890abcdef1234");
        assert_eq!(parsed["tip_dirty"], true);
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
        let id = BenchmarkId::new(nonempty![
            "pkg".to_owned(),
            "group".to_owned(),
            "case".to_owned(),
            "value".to_owned(),
        ]);
        assert_eq!(describe_id(&id), "pkg/group/case/value");
        let bare = BenchmarkId::new(nonempty!["group".to_owned()]);
        assert_eq!(describe_id(&bare), "group");
    }

    #[test]
    fn chart_of_needs_at_least_two_points() {
        // Deterministic, Miri-safe color state for the rasciigraph plot.
        let _color = ColorOverride::force(false);
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
    fn chart_of_plots_improvements_and_overlays_a_partial_active_window() {
        // Deterministic, Miri-safe color state for the rasciigraph plot.
        let _color = ColorOverride::force(false);
        let point = |value: f64| SeriesValue {
            commit: None,
            value,
            dirty: false,
        };
        let series = [point(1.0), point(2.0), point(3.0)];
        // An improvement plots through the green color arm and the whole-series path.
        assert!(chart_of(&series, Direction::Improvement, 0).is_some());
        // A partial active window (active_from in the interior) overlays the greyed
        // pre-blessing prefix and the active tail as two series.
        assert!(chart_of(&series, Direction::Improvement, 1).is_some());
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

    fn drift() -> Finding {
        Finding {
            method: FindingMethod::Drift,
            ..regression()
        }
    }

    fn darwin_set() -> DiscriminantSet {
        DiscriminantSet {
            engine: "callgrind".to_owned(),
            target_triple: "aarch64-apple-darwin".to_owned(),
            machine_key: "synthetic".to_owned(),
        }
    }

    #[test]
    fn text_report_labels_a_drift_finding_and_skips_an_empty_set() {
        let set_a = discriminant_set();
        let set_b = darwin_set();
        let findings = [drift()];
        let summaries = vec![
            SetSummary {
                set: &set_a,
                runs: 6,
                series: 1,
                findings: vec![&findings[0]],
            },
            SetSummary {
                set: &set_b,
                runs: 4,
                series: 1,
                findings: Vec::new(),
            },
        ];
        let input = ReportInput {
            project: "folo",
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: true,
            runs: 10,
            series: 2,
            commit_span: None,
            report_improvements: false,
            findings: &findings,
            sets: &summaries,
            hint: None,
            warning: None,
        };
        let report = render(&input, ReportFormat::Text, false);
        assert!(report.contains("drift"), "{report}");
        assert!(report.contains("x86_64-unknown-linux-gnu"), "{report}");
        assert!(
            !report.contains("aarch64-apple-darwin"),
            "the empty set is skipped: {report}"
        );
    }

    #[test]
    fn markdown_report_labels_a_drift_finding_and_skips_an_empty_set() {
        let set_a = discriminant_set();
        let set_b = darwin_set();
        let findings = [drift()];
        let summaries = vec![
            SetSummary {
                set: &set_a,
                runs: 6,
                series: 1,
                findings: vec![&findings[0]],
            },
            SetSummary {
                set: &set_b,
                runs: 4,
                series: 1,
                findings: Vec::new(),
            },
        ];
        let input = ReportInput {
            project: "folo",
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "history",
            notable: true,
            runs: 10,
            series: 2,
            commit_span: None,
            report_improvements: false,
            findings: &findings,
            sets: &summaries,
            hint: None,
            warning: None,
        };
        let report = render(&input, ReportFormat::Markdown, false);
        assert!(report.contains("drift"), "{report}");
        assert!(report.contains("x86_64-unknown-linux-gnu"), "{report}");
        assert!(
            !report.contains("aarch64-apple-darwin"),
            "the empty set is skipped: {report}"
        );
    }

    #[test]
    fn header_shows_the_analyzed_commit_span() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.runs = 12;
        input.commit_span = Some(("a1b2c3d4e5f60000", "f6e5d4c3b2a10000"));

        // Both ends abbreviate to 12 hex digits, joined by an arrow, so the reader can
        // see the stretch of history the analysis covered.
        let text = render(&input, ReportFormat::Text, false);
        assert!(
            text.contains("runs: 12 (a1b2c3d4e5f6 → f6e5d4c3b2a1)"),
            "{text}"
        );
        let markdown = render(&input, ReportFormat::Markdown, false);
        assert!(
            markdown.contains("- Runs analyzed: 12 (a1b2c3d4e5f6 → f6e5d4c3b2a1)"),
            "{markdown}"
        );
    }

    #[test]
    fn header_collapses_a_single_commit_span() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.runs = 1;
        input.commit_span = Some(("abcdef0123456789", "abcdef0123456789"));

        // One analyzed commit reads as a single anchor, not `x → x`.
        let text = render(&input, ReportFormat::Text, false);
        assert!(text.contains("runs: 1 (abcdef012345)"), "{text}");
    }

    #[test]
    fn improvement_tally_is_omitted_when_improvements_are_not_reported() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.report_improvements = false;

        // Neither the top-level header nor the per-set breakdown counts improvements
        // when the analysis reports none (an always-zero tally).
        let text = render(&input, ReportFormat::Text, false);
        assert!(text.contains("regressions: 1"), "{text}");
        assert!(!text.contains("improvements:"), "{text}");

        let markdown = render(&input, ReportFormat::Markdown, false);
        assert!(markdown.contains("- Regressions: 1"), "{markdown}");
        assert!(!markdown.contains("- Improvements:"), "{markdown}");
    }

    #[test]
    fn improvement_tally_is_shown_when_improvements_are_reported() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.report_improvements = true;

        let text = render(&input, ReportFormat::Text, false);
        assert!(text.contains("improvements: 0"), "{text}");
        let markdown = render(&input, ReportFormat::Markdown, false);
        assert!(markdown.contains("- Improvements: 0"), "{markdown}");
    }

    #[test]
    fn series_tally_is_omitted_from_text_and_markdown_but_kept_in_json() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);

        let text = render(&input, ReportFormat::Text, false);
        assert!(!text.contains("series:"), "{text}");
        let markdown = render(&input, ReportFormat::Markdown, false);
        assert!(!markdown.contains("- Series"), "{markdown}");

        // JSON keeps the series count for machine consumers (e.g. the stress harness).
        let json = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["series"].is_number(), "{json}");
        assert!(parsed["sets"][0]["series"].is_number(), "{json}");
    }

    #[test]
    fn markdown_wraps_the_metric_name_in_inline_code() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);

        // The metric name is a keyword-like identifier, so Markdown renders it as inline
        // code (backticks) rather than bare prose. The text report carries no such markup.
        let markdown = render(&input, ReportFormat::Markdown, false);
        assert!(
            markdown.contains("`instruction_count` (100% confidence)"),
            "{markdown}"
        );

        let text = render(&input, ReportFormat::Text, false);
        assert!(
            text.contains("instruction_count (100% confidence)"),
            "{text}"
        );
        assert!(!text.contains("`instruction_count`"), "{text}");
    }
}
