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

use std::collections::HashSet;
use std::num::NonZero;
use std::sync::{Mutex, MutexGuard, PoisonError};

use cbh_detect::{Direction, Finding, FindingMethod, short_commit};
use cbh_model::{BenchmarkId, DiscriminantSet};
use colored::Colorize;
use rasciigraph::{Config, plot};
use serde::Serialize;

/// Height, in rows, of a finding chart.
const CHART_HEIGHT: u32 = 4;
/// Width, in columns, of a finding chart.
const CHART_WIDTH: u32 = 48;

/// Maximum number of values in a branch-mode chart.
///
/// Branch mode judges a feature branch by its **tip commit alone** against a recent base
/// level, so the tip is the one point that matters. Plotting the whole (often
/// months-long) series would resample it down to [`CHART_WIDTH`] columns, shrinking that
/// tip to a single edge column where it reads as noise. The chart starts with the
/// comparison baseline and fills the remaining slots with the recent observed tail,
/// keeping both sides of the reported change visible. The cap stays below
/// [`CHART_WIDTH`] so every value maps to its own column without resampling.
const BRANCH_CHART_MAX_POINTS: usize = 30;

/// The number of findings a Markdown summary retains by default.
///
/// Enough to convey the most significant movers while keeping the rendered report
/// comfortably within a GitHub issue body's size limit even when an analysis flags
/// many changes.
pub const DEFAULT_SUMMARY_LIMIT: NonZero<usize> = NonZero::new(10).expect("10 is non-zero");

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
    /// How far this set's comparison base(s) sit behind the merge-base, with the
    /// reason for each distinct lag. Branch mode only; empty when every finding's
    /// comparison base reaches the merge-base (the usual whole-suite case) and in
    /// history mode. Partial runs can leave different findings comparing against
    /// different points, so this is a deduplicated, deterministically ordered list
    /// rather than a single value.
    pub comparison_base_lags: Vec<ComparisonBaseLag>,
}

/// How far a discriminant set's comparison base sits behind the merge-base, and why.
///
/// In branch mode each finding is compared against the recent base-side points of its
/// own discriminant set. On rotating CI machine pools the newest base commits may carry
/// data only under a different machine key, so the branch runner's machine key has usable
/// base data only several commits earlier — the comparison silently reaches back in
/// history. This records that lag for one set so the report can disclose it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ComparisonBaseLag {
    /// First-parent distance from the comparison base to the merge-base. Always at
    /// least one: a comparison base that reaches the merge-base is not a lag and is
    /// never recorded.
    pub commits_behind: NonZero<usize>,
    /// Why the comparison base lags.
    pub reason: ComparisonBaseLagReason,
}

/// Why a discriminant set's comparison base lags the merge-base.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonBaseLagReason {
    /// A newer base-side run for the same benchmark and metric exists, but under a
    /// different machine key — machine-pool rotation, not missing measurements. The
    /// comparison base could not use it because counts are not comparable across
    /// machine keys.
    DiscriminantSetMismatch,
    /// No base-side run for the affected series exists at any more recent commit; the
    /// comparison base is simply the newest data available for this partition.
    NoRecentBaseData,
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
    /// How many benchmarks were dropped as "ghosts" — present only for past commits,
    /// not at the context commit — before detection. Zero when nothing was dropped.
    /// Carried for the JSON report so a machine consumer sees that scoping happened;
    /// the text and Markdown reports surface it only through the verbose trail and the
    /// empty-outcome hint.
    pub ghosts_excluded: usize,
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
    /// Machine key: the partition value the runs were stored under (a hardware
    /// fingerprint, or an explicit `--machine-key` override such as a pool label).
    machine_key: &'a str,
    /// Stored runs loaded for this set.
    runs: usize,
    /// Distinct series compared in this set.
    series: usize,
    /// Flagged regressions in this set.
    regressions: usize,
    /// Flagged improvements in this set.
    improvements: usize,
    /// How far this set's comparison base(s) lag the merge-base, with the reason for
    /// each distinct lag (branch mode only). Omitted when the comparison base reaches
    /// the merge-base — the usual case — and in history mode.
    #[serde(skip_serializing_if = "<[ComparisonBaseLag]>::is_empty")]
    comparison_base_lags: &'a [ComparisonBaseLag],
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
    /// Benchmarks dropped as ghosts (present only for past commits, not at the
    /// context commit) before detection. Zero when nothing was dropped.
    ghosts_excluded: usize,
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
/// `color` enables ANSI styling of the text format — the direction-colored headline
/// percentage, the bold benchmark id, and the dimmed detail and blessing lines. The
/// caller decides it from the output terminal so tests and pipes stay plain; charts
/// are always uncolored, and `markdown` and `json` ignore it.
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

/// Formats one comparison-base lag as its exact report warning line, with the
/// singular/plural agreement the text and Markdown reports share.
fn comparison_base_lag_warning(lag: &ComparisonBaseLag) -> String {
    let count = lag.commits_behind.get();
    let commits = if count == 1 { "commit" } else { "commits" };
    let reason = match lag.reason {
        ComparisonBaseLagReason::DiscriminantSetMismatch => "discriminant set mismatch",
        ComparisonBaseLagReason::NoRecentBaseData => "no base data at more recent commits",
    };
    format!("Warning: comparison base is {count} {commits} behind base ({reason})")
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

/// Serializes access to `colored`'s process-global override.
static COLOR_OVERRIDE_LOCK: Mutex<()> = Mutex::new(());

/// Forces `colored`'s process-global override to `value` until dropped, then restores
/// ambient auto-detection.
///
/// Holding [`COLOR_OVERRIDE_LOCK`] for the override's lifetime prevents concurrent
/// renders from changing the process-global state while output is being assembled.
struct ColorOverride {
    _lock: MutexGuard<'static, ()>,
}

impl ColorOverride {
    #[must_use]
    fn force(value: bool) -> Self {
        let lock = COLOR_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        colored::control::set_override(value);
        Self { _lock: lock }
    }
}

impl Drop for ColorOverride {
    // Restoring `colored`'s process-global override is exercised by every render test
    // (each builds and drops a guard), but `colored` exposes no override-state getter
    // to assert the restoration directly. Skipped for mutation only; the restore itself
    // is covered behaviourally.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        colored::control::unset_override();
    }
}

fn render_text(input: &ReportInput<'_>, color: bool) -> String {
    // Force `colored` to honor this explicit decision rather than its own ambient
    // terminal auto-detection, so tests and pipes are deterministic regardless of how
    // the process is run. The guard restores auto-detection on return.
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

    // Both modes draw a per-finding chart; the scope differs. History walks the whole
    // series; branch charts the comparison baseline and recent tail so the tip commit
    // it judges stays legible (see `ChartScope`).
    let scope = chart_scope(input.mode);
    for summary in input.sets {
        if summary.findings.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(set_label(summary.set));
        lines.push(set_counts_line(summary, input.report_improvements));
        lines.push(format!("  filter: {}", set_filter_flags(summary.set)));
        for lag in &summary.comparison_base_lags {
            lines.push(format!("  {}", comparison_base_lag_warning(lag)));
        }
        for finding in &summary.findings {
            push_finding_block(&mut lines, finding, scope);
        }
    }
    push_warning(&mut lines, input.warning);
    finish(&lines)
}

/// Appends one finding as a paragraph: the benchmark id on its own line as a
/// chapter title, then a direction-colored `percentage metric (confidence)`
/// headline, a dimmed detail line, an optional blessing/recovery note, and a chart
/// of the metric over commits, scoped per [`ChartScope`].
fn push_finding_block(lines: &mut Vec<String>, finding: &Finding, scope: ChartScope) {
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

    if let Some(chart) = scoped_chart(finding, scope) {
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
    // `flipped_at` is set only on an inactive (recovered) history-mode spike, naming
    // the commit where the level returned to baseline; branch mode never sets it.
    if let Some(recovered_at) = &finding.flipped_at {
        use std::fmt::Write as _;
        write!(detail, " · recovers at {recovered_at}").expect("writing to a String is infallible");
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

/// How a finding's metric chart is scoped for the analysis mode it was produced in.
///
/// The two modes ask different questions of the same stored history, so they chart
/// different slices of a finding's series.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChartScope {
    /// History mode: plot the whole series, so the long-range trend shows.
    FullHistory,
    /// Branch mode: plot the comparison baseline followed by the most recent points
    /// ending at the tip commit. Branch mode judges the branch by that one commit, so
    /// the bounded chart keeps it legible instead of aliasing it into a single edge
    /// column (see [`BRANCH_CHART_MAX_POINTS`]).
    BranchComparison,
}

/// Chooses the [`ChartScope`] for an analysis `mode` string (`history`/`branch`).
///
/// Only `branch` uses the bounded comparison; every other value (history, and any
/// future mode) charts the full series, the information-preserving default.
fn chart_scope(mode: &str) -> ChartScope {
    if mode == "branch" {
        ChartScope::BranchComparison
    } else {
        ChartScope::FullHistory
    }
}

/// Builds the bounded values for a branch finding's comparison chart.
///
/// The first value is the detector's actual comparison baseline. The remaining values
/// are the recent observed suffix ending at the tip, guaranteeing that both sides of
/// the reported change remain visible even when a long-lived branch contributes more
/// points than the chart cap. The tip is the one data point branch mode acts on, and it
/// must never be dropped or aliased away, however long the underlying history is.
fn branch_chart_values(finding: &Finding) -> Vec<f64> {
    let observed_limit = BRANCH_CHART_MAX_POINTS.saturating_sub(1);
    let start = finding.series.len().saturating_sub(observed_limit);
    let observed = finding
        .series
        .get(start..)
        .expect("saturating subtraction keeps the start within the series");
    let mut values = Vec::with_capacity(observed.len().saturating_add(1));
    values.push(finding.baseline);
    values.extend(observed.iter().map(|point| point.value));
    values
}

/// Renders a finding's metric chart for the given [`ChartScope`], or `None` when the
/// scoped series has too few points to plot.
///
/// [`ChartScope::FullHistory`] charts the whole series; [`ChartScope::BranchComparison`]
/// charts [`branch_chart_values`], so the comparison baseline and tip commit stay legible
/// rather than becoming aliased edge columns.
fn scoped_chart(finding: &Finding, scope: ChartScope) -> Option<String> {
    let values: Vec<f64> = match scope {
        ChartScope::FullHistory => finding.series.iter().map(|point| point.value).collect(),
        // Chart the comparison baseline and recent tail ending at the tip. This is
        // business-critical: the tip commit is the sole data point branch mode judges,
        // so it must remain visible and unaliased regardless of how much history
        // precedes it.
        ChartScope::BranchComparison => branch_chart_values(finding),
    };
    chart(&values)
}

/// Renders a compact line chart of `values` over commits at the report's fixed chart
/// dimensions. Returns `None` when there are fewer than two points (nothing to plot).
///
/// The line is always drawn uncolored: the report is most often read as Markdown, where
/// ANSI styling would only add noise, so the chart carries plain characters that render
/// anywhere.
#[must_use]
pub fn chart(values: &[f64]) -> Option<String> {
    if values.len() < 2 {
        return None;
    }
    let config = Config::default()
        .with_height(CHART_HEIGHT)
        .with_width(CHART_WIDTH);
    Some(
        plot(values.to_vec(), config)
            .trim_end_matches('\n')
            .to_owned(),
    )
}

fn render_markdown(input: &ReportInput<'_>) -> String {
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

    // Both modes draw a per-finding chart, matching the text report; the scope differs
    // (history walks the whole series, branch charts the baseline and recent tail).
    let scope = chart_scope(input.mode);
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
        for lag in &summary.comparison_base_lags {
            lines.push(String::new());
            lines.push(format!("> {}", comparison_base_lag_warning(lag)));
        }
        for finding in &summary.findings {
            push_finding_markdown(&mut lines, finding, "###", scope);
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

    // Both modes draw a per-finding chart, matching the full reports; the scope differs
    // (history walks the whole series, branch charts the baseline and recent tail).
    //
    // Comparison-base warnings are per-set metadata, but the summary flattens the set
    // grouping, so surface each affected set's warnings once — immediately before that
    // set's first retained finding.
    let scope = chart_scope(input.mode);
    let mut warned_sets: HashSet<&DiscriminantSet> = HashSet::new();
    for finding in input.findings.iter().take(limit.get()) {
        if warned_sets.insert(&finding.set)
            && let Some(summary) = input
                .sets
                .iter()
                .find(|summary| *summary.set == finding.set)
        {
            for lag in &summary.comparison_base_lags {
                lines.push(String::new());
                lines.push(format!("> {}", comparison_base_lag_warning(lag)));
            }
        }
        push_finding_markdown(&mut lines, finding, "##", scope);
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
/// line, the shared detail line, an optional blessing note, and the metric chart in a
/// fenced `text` block so it survives Markdown rendering, scoped per [`ChartScope`].
/// `heading` carries the ATX prefix (`##`/`###`) so the block nests correctly —
/// top-level in the summary, one level under the set heading in the full report.
fn push_finding_markdown(
    lines: &mut Vec<String>,
    finding: &Finding,
    heading: &str,
    scope: ChartScope,
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

    if let Some(chart) = scoped_chart(finding, scope) {
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
            engine: summary.set.engine.as_str(),
            target_triple: &summary.set.target_triple,
            machine_key: &summary.set.machine_key,
            runs: summary.runs,
            series: summary.series,
            regressions: count_direction(&summary.findings, Direction::Regression),
            improvements: count_direction(&summary.findings, Direction::Improvement),
            comparison_base_lags: &summary.comparison_base_lags,
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
        ghosts_excluded: input.ghosts_excluded,
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

    use std::sync::TryLockError;
    use std::thread;

    use cbh_detect::SeriesValue;
    use cbh_model::{Engine, MetricKind};
    use nonempty::nonempty;

    use super::*;

    fn discriminant_set() -> DiscriminantSet {
        DiscriminantSet {
            engine: Engine::Callgrind,
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine_key: "m1".to_owned(),
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
            blessed_at: None,
            blessed_commit_time: None,
            series: Vec::new(),
            comparison_base_index: None,
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
            comparison_base_lags: Vec::new(),
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
            ghosts_excluded: 0,
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
            ghosts_excluded: 0,
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

        let footer = "_Filter:_ `--engine callgrind --target-triple x86_64-unknown-linux-gnu --machine-key m1`";
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
                engine: Engine::Criterion,
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
                "--engine callgrind --target-triple x86_64-unknown-linux-gnu --machine-key m1"
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
            ghosts_excluded: 0,
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
            ghosts_excluded: 0,
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
            report.contains("callgrind/x86_64-unknown-linux-gnu/m1"),
            "the set heading drops the redundant `Set ` prefix: {report}"
        );
        // The set header names the facet flags that reproduce exactly this partition,
        // so a reader who spots a finding knows how to query it directly.
        assert!(
            report.contains(
                "  filter: --engine callgrind --target-triple x86_64-unknown-linux-gnu --machine-key m1"
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
            comparison_base_lags: Vec::new(),
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
            ghosts_excluded: 0,
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
            report.contains("## callgrind/x86_64-unknown-linux-gnu/m1"),
            "the set heading drops the redundant `Set ` prefix: {report}"
        );
        // The per-set tally mirrors the JSON metadata and the text header.
        assert!(report.contains("- Regressions: 1"), "{report}");
        // The set header names the facet flags that reproduce exactly this partition.
        assert!(
            report.contains(
                "- Filter: `--engine callgrind --target-triple x86_64-unknown-linux-gnu --machine-key m1`"
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
            ghosts_excluded: 0,
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
            ghosts_excluded: 0,
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
            ghosts_excluded: 0,
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
            ghosts_excluded: 0,
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
            ghosts_excluded: 0,
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
    /// chart has enough points to draw.
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

    /// A y-axis value that only ever appears on a chart's scale, never in a finding's
    /// prose (its baseline/latest are 100/130), so a report either charting or omitting
    /// it can be told apart by a plain substring search.
    const CHART_ONLY_MARKER: f64 = 1000.0;

    /// Builds a regression finding with a long series: a distinctive early spike, a long
    /// flat baseline, and an elevated tip.
    ///
    /// The early spike ([`CHART_ONLY_MARKER`]) sits far outside the branch chart's
    /// bounded comparison and dwarfs the tip, so it dominates a whole-series chart's
    /// y-axis but is absent from a branch chart's — the discriminator the scope tests
    /// key off. The tip (index `len - 1`) is the one point branch mode judges; it must
    /// always survive into the chart.
    fn regression_with_long_series() -> Finding {
        let len = 200;
        let tip = 130.0;
        let baseline = 100.0;
        let mut series: Vec<SeriesValue> = (0..len)
            .map(|index| SeriesValue {
                commit: Some(format!("c{index}")),
                value: baseline,
                dirty: false,
            })
            .collect();
        // A towering early point, older than the bounded branch chart would reach.
        series[0].value = CHART_ONLY_MARKER;
        // The tip: the last commit analyzed, the single point branch mode acts on.
        series[len - 1].value = tip;
        Finding {
            series,
            ..regression()
        }
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
    fn branch_mode_text_draws_a_chart() {
        let set = discriminant_set();
        let findings = vec![regression_with_series()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.mode = "branch";
        let text = render(&input, ReportFormat::Text, false);
        // Branch mode now charts too (it previously did not); the axis marker proves it.
        assert!(text.contains('┤') || text.contains('┼'), "{text}");
    }

    #[test]
    fn branch_mode_text_charts_the_bounded_comparison_including_the_tip() {
        // BUSINESS-CRITICAL INVARIANT. Branch mode judges a feature branch by its tip
        // commit alone, so the tip is the one data point the report exists to convey. It
        // must remain visible on the chart no matter how long the history is — never
        // aliased away or shrunk to an indistinct edge column by resampling a
        // months-long series down to the chart width. This test pins that: charting the
        // baseline and recent tail keeps the tip as the chart's maximum while dropping
        // ancient history. Do NOT weaken it to "a chart is drawn" — the point is *which*
        // values the chart shows.
        let set = discriminant_set();
        let findings = vec![regression_with_long_series()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.mode = "branch";
        let text = render(&input, ReportFormat::Text, false);

        // A chart is drawn.
        assert!(text.contains('┤') || text.contains('┼'), "{text}");
        // The tip is the tail's peak, so it labels the top of the y-axis: the last
        // commit analyzed is unmistakably plotted, not aliased into the baseline.
        assert!(
            text.contains("130 ┤") || text.contains("130 ┼"),
            "the tip value must head the chart's y-axis: {text}"
        );
        // The ancient spike lies outside the bounded comparison, so it must not appear
        // on the chart scale — proof the whole series was not charted, and that ancient
        // outliers cannot squash the tip out of view.
        assert!(
            !text.contains("1000"),
            "branch mode must exclude ancient history from the chart: {text}"
        );
    }

    #[test]
    fn history_mode_text_charts_the_whole_series() {
        // The companion to the branch comparison test: history mode charts the *entire*
        // series, so the same ancient spike that a branch chart drops here heads the
        // y-axis. This keeps the two scopes distinct and stops a refactor from silently
        // collapsing branch's bounded view into history's whole-series view (or vice
        // versa).
        let set = discriminant_set();
        let findings = vec![regression_with_long_series()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.mode = "history";
        let text = render(&input, ReportFormat::Text, false);

        assert!(text.contains('┤') || text.contains('┼'), "{text}");
        assert!(
            text.contains("1000"),
            "history mode charts the whole series, so the early spike heads the y-axis: {text}"
        );
    }

    #[test]
    fn branch_mode_markdown_draws_a_fenced_chart() {
        let set = discriminant_set();
        let findings = vec![regression_with_series()];
        let mut summaries = Vec::new();
        let mut input = single_set_input("folo", &set, &findings, &mut summaries);
        input.mode = "branch";
        let report = render(&input, ReportFormat::Markdown, false);
        // The branch chart sits inside the same fenced `text` block as a history chart.
        assert!(report.contains("```text"), "{report}");
        assert!(report.contains('┤') || report.contains('┼'), "{report}");
    }

    #[test]
    fn markdown_summary_charts_the_branch_comparison() {
        // The summary is the third renderer that must chart branch findings; and, like the
        // full reports, it must keep the tip (last commit) visible while windowing out
        // ancient history. See
        // `branch_mode_text_charts_the_bounded_comparison_including_the_tip`.
        let findings = vec![regression_with_long_series()];
        let mut input = flat_input(&findings);
        input.mode = "branch";
        let report = render_markdown_summary(&input, DEFAULT_SUMMARY_LIMIT);

        assert!(report.contains("```text"), "{report}");
        assert!(
            report.contains("130 ┤") || report.contains("130 ┼"),
            "the tip must head the summary chart's y-axis: {report}"
        );
        assert!(
            !report.contains("1000"),
            "the branch summary must exclude ancient history: {report}"
        );
    }

    #[test]
    fn branch_chart_values_keep_the_baseline_and_tip() {
        // BUSINESS-CRITICAL INVARIANT. A long-lived branch can itself contribute more
        // points than the chart cap. Even then, the bounded chart must retain both the
        // comparison baseline and the last point — the tip commit branch mode judges —
        // without resampling either one away.
        let mut finding = regression_with_long_series();
        for point in finding
            .series
            .iter_mut()
            .rev()
            .take(BRANCH_CHART_MAX_POINTS)
        {
            point.value = finding.latest;
        }
        let values = branch_chart_values(&finding);
        assert_eq!(values.len(), BRANCH_CHART_MAX_POINTS, "the chart is capped");
        assert_eq!(
            values
                .first()
                .expect("the baseline is always present")
                .to_bits(),
            finding.baseline.to_bits(),
            "the detector's comparison baseline must remain visible"
        );
        assert_eq!(
            values
                .last()
                .expect("the observed tail is non-empty")
                .to_bits(),
            finding
                .series
                .last()
                .expect("the test series is non-empty")
                .value
                .to_bits(),
            "the tip must be the last charted point"
        );
        assert!(
            values
                .iter()
                .all(|value| value.to_bits() != CHART_ONLY_MARKER.to_bits()),
            "ancient observations must fall outside the bounded chart"
        );
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
    fn chart_needs_at_least_two_points() {
        // A single point cannot be plotted; two is the minimum (a `< 2` -> `<= 2`
        // slip would reject the two-point case).
        assert!(chart(&[1.0]).is_none());
        assert!(chart(&[1.0, 2.0]).is_some());
    }

    #[test]
    fn color_override_holds_the_global_lock_across_threads() {
        let _color = ColorOverride::force(false);

        let lock_was_held = thread::spawn(|| {
            matches!(
                COLOR_OVERRIDE_LOCK.try_lock(),
                Err(TryLockError::WouldBlock)
            )
        })
        .join()
        .unwrap();

        assert!(lock_was_held);
    }

    #[test]
    fn chart_plots_uncolored_without_ansi() {
        // Charts are always uncolored, whatever `colored`'s process-global override
        // says. Force it on to prove a drawn chart never embeds an ANSI escape.
        let _color = ColorOverride::force(true);
        let chart = chart(&[1.0, 2.0, 3.0]).expect("two-plus points plot");
        // The rasciigraph axis marker proves a chart was drawn; no escape byte proves
        // the line stayed uncolored.
        assert!(chart.contains('┤') || chart.contains('┼'), "{chart}");
        assert!(!chart.contains('\u{1b}'), "no ANSI escape: {chart:?}");
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
            engine: Engine::Callgrind,
            target_triple: "aarch64-apple-darwin".to_owned(),
            machine_key: "m2".to_owned(),
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
                comparison_base_lags: Vec::new(),
            },
            SetSummary {
                set: &set_b,
                runs: 4,
                series: 1,
                findings: Vec::new(),
                comparison_base_lags: Vec::new(),
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
            ghosts_excluded: 0,
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
                comparison_base_lags: Vec::new(),
            },
            SetSummary {
                set: &set_b,
                runs: 4,
                series: 1,
                findings: Vec::new(),
                comparison_base_lags: Vec::new(),
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
            ghosts_excluded: 0,
        };
        let report = render(&input, ReportFormat::Markdown, false);
        assert!(report.contains("drift"), "{report}");
        assert!(report.contains("x86_64-unknown-linux-gnu"), "{report}");
        assert!(
            !report.contains("aarch64-apple-darwin"),
            "the empty set is skipped: {report}"
        );
    }

    /// Builds a comparison-base lag from a plain count and reason.
    fn lag(commits_behind: usize, reason: ComparisonBaseLagReason) -> ComparisonBaseLag {
        ComparisonBaseLag {
            commits_behind: NonZero::new(commits_behind).expect("test count is non-zero"),
            reason,
        }
    }

    /// A single-set branch-mode report whose set carries `lags`, for the
    /// comparison-base warning-surface tests.
    fn input_with_lags<'a>(
        set: &'a DiscriminantSet,
        findings: &'a [Finding],
        lags: Vec<ComparisonBaseLag>,
        summaries: &'a mut Vec<SetSummary<'a>>,
    ) -> ReportInput<'a> {
        summaries.push(SetSummary {
            set,
            runs: findings.len().saturating_add(3),
            series: findings.len().max(1),
            findings: findings.iter().collect(),
            comparison_base_lags: lags,
        });
        ReportInput {
            project: "folo",
            tip_commit: "1234567890abcdef1234",
            tip_dirty: false,
            mode: "branch",
            notable: !findings.is_empty(),
            runs: findings.len().saturating_add(3),
            series: findings.len().max(1),
            commit_span: None,
            report_improvements: false,
            findings,
            sets: summaries,
            hint: None,
            warning: None,
            ghosts_excluded: 0,
        }
    }

    #[test]
    fn every_format_reports_a_discriminant_set_mismatch_warning() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = input_with_lags(
            &set,
            &findings,
            vec![lag(5, ComparisonBaseLagReason::DiscriminantSetMismatch)],
            &mut summaries,
        );
        let expected =
            "Warning: comparison base is 5 commits behind base (discriminant set mismatch)";
        for report in [
            render(&input, ReportFormat::Text, false),
            render(&input, ReportFormat::Markdown, false),
            render_markdown_summary(&input, DEFAULT_SUMMARY_LIMIT),
        ] {
            assert!(report.contains(expected), "{report}");
        }
    }

    #[test]
    fn missing_base_data_warning_uses_a_singular_count() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = input_with_lags(
            &set,
            &findings,
            vec![lag(1, ComparisonBaseLagReason::NoRecentBaseData)],
            &mut summaries,
        );
        assert!(
            render(&input, ReportFormat::Text, false).contains(
                "Warning: comparison base is 1 commit behind base \
                 (no base data at more recent commits)"
            ),
            "singular count and generic reason"
        );
    }

    #[test]
    fn json_report_carries_comparison_base_lags_in_order() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = input_with_lags(
            &set,
            &findings,
            vec![
                lag(5, ComparisonBaseLagReason::DiscriminantSetMismatch),
                lag(2, ComparisonBaseLagReason::NoRecentBaseData),
            ],
            &mut summaries,
        );
        let json = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let lags = &parsed["sets"][0]["comparison_base_lags"];
        assert_eq!(lags[0]["commits_behind"], 5);
        assert_eq!(lags[0]["reason"], "discriminant_set_mismatch");
        assert_eq!(lags[1]["commits_behind"], 2);
        assert_eq!(lags[1]["reason"], "no_recent_base_data");
    }

    #[test]
    fn json_report_omits_comparison_base_lags_when_absent() {
        let set = discriminant_set();
        let findings = vec![regression()];
        let mut summaries = Vec::new();
        let input = single_set_input("folo", &set, &findings, &mut summaries);
        let json = render(&input, ReportFormat::Json, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            parsed["sets"][0].get("comparison_base_lags").is_none(),
            "unaffected sets carry no comparison_base_lags: {json}"
        );
    }

    #[test]
    fn summary_emits_a_set_warning_once_before_its_findings() {
        // Two findings from the same set: the set's warning must surface exactly once,
        // even though the summary flattens the per-set grouping.
        let set = discriminant_set();
        let findings = vec![
            named_regression("alpha", 0.50),
            named_regression("beta", 0.40),
        ];
        let mut summaries = Vec::new();
        let input = input_with_lags(
            &set,
            &findings,
            vec![lag(3, ComparisonBaseLagReason::DiscriminantSetMismatch)],
            &mut summaries,
        );
        let summary = render_markdown_summary(&input, DEFAULT_SUMMARY_LIMIT);
        let warning =
            "Warning: comparison base is 3 commits behind base (discriminant set mismatch)";
        assert_eq!(summary.matches(warning).count(), 1, "{summary}");
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
