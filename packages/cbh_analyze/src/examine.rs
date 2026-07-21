//! The `examine` command: pivot one `(benchmark, metric)` series into its raw
//! per-commit data points.
//!
//! `examine` is a drill-down sibling of `list runs`: both are read-only previews
//! over `analyze`'s exact data-set selection (see
//! `AnalyzeOptions` and `ExamineOptions`) resolved via
//! the shared [`select_dataset`](super::select_dataset) path, so a selection
//! parameter added to `analyze` applies here unchanged. Where `list` counts the
//! selected runs, `examine` names one `(benchmark, metric)` series and prints its
//! points — one row per recorded observation, in git first-parent order, pairing
//! the value with the short commit id and the start of the commit's title so a
//! maintainer can correlate a value's move with the commit that caused it.
//!
//! It runs no detection, re-baselining, or blessing: it shows every selected point
//! exactly as the chart would plot it (a commit's clean run and its dirty snapshots
//! each contribute a flagged row, clean before dirty). The text and Markdown
//! renderings lead each set with the same small line chart `analyze` draws (reusing
//! its renderer). Like `analyze` it needs a resolvable repository — first-parent topology
//! to order the points and each commit's title to label them — and repeats the pivot
//! once per matching discriminant set.

use std::collections::HashMap;
use std::path::Path;

use anyspawn::Spawner;
use cbh_command::ExamineOptions;
use cbh_config::{
    Config, cache_env, load_config, resolve_cache_path, resolve_config_path, resolve_local_path,
    resolve_project_id, resolve_repo, storage_env,
};
use cbh_diag::{Reporter, ReporterExt, StderrReporter, count_noun};
use cbh_git::{GitHistory, SystemGitHistory};
use cbh_model::{BenchmarkIdPrefix, DiscriminantSet, MetricKind};
use cbh_storage::{Storage, StorageFacade, resolve_storage};
use jiff::Timestamp;
use serde::Serialize;
use tick::Clock;

use super::{
    AutoFacets, ReportFormat, Selection, Series, SeriesFilter, chart, dirty_base_exception_warning,
    empty_history_hint, format_value, resolve_auto_facets, resolve_now, select_dataset,
};
use crate::{AnalyzeError, RenderedReports, ReportRequest};

/// How many leading characters of a commit title the text and Markdown tables
/// keep. The truncation is a readability convenience of those renderings; the JSON
/// form carries the full title.
const TITLE_LIMIT: usize = 50;

/// The real `examine`: load configuration, wire the configured storage and git
/// history, and orchestrate.
///
/// `clock_override` injects the [`tick::Clock`] the shared selection anchors its
/// "now" to (see [`analyze`](super::analyze)); production passes `None` for the
/// runtime wall clock.
// Thin real-adapter wiring: loads config from disk, builds the configured storage,
// and shells out via `SystemGitHistory`/`detect_auto_facets` before delegating every
// decision to the mutation-tested `examine_with`. In-crate tests cannot drive these
// real adapters deterministically; the binary's integration tests cover this edge.
#[cfg_attr(test, mutants::skip)]
pub async fn execute(
    options: &ExamineOptions,
    workspace_dir: &Path,
    clock_override: Option<Clock>,
    storage_override: Option<StorageFacade>,
    auto_override: Option<AutoFacets>,
) -> Result<RenderedReports, AnalyzeError> {
    let reporter = StderrReporter::new(options.verbose);

    let config_path = resolve_config_path(workspace_dir, options.config_path.as_deref());
    reporter.note_with(|| format!("loading configuration from {}", config_path.display()));
    let config = load_config(&config_path, options.config_path.is_some()).await?;

    let project_id = resolve_project_id(&config, workspace_dir);
    let local = resolve_local_path(options.local.as_ref(), storage_env().as_deref())?;
    let cache = resolve_cache_path(options.cache.as_ref(), cache_env().as_deref())?;
    let storage = resolve_storage(
        storage_override,
        local.as_deref(),
        &config,
        workspace_dir,
        cache.as_deref(),
        &reporter,
    )?;
    storage.synchronize_cache(&project_id, &reporter).await?;

    let git = SystemGitHistory::new(resolve_repo(workspace_dir, options.repo.as_deref()));
    let auto = resolve_auto_facets(auto_override).await?;

    let now = resolve_now(clock_override);
    // The object-load work shares the ambient Tokio worker threads (mirrors
    // `analyze::execute`).
    let spawner = Spawner::new_tokio();
    let outcome = examine_with(
        &git,
        &storage,
        &project_id,
        &config,
        options,
        &auto,
        now,
        &reporter,
        &spawner,
    )
    .await;
    storage.report_cache_tally(&reporter);
    outcome
}

/// Storage- and git-generic `examine`: resolve the same data set `analyze` would,
/// narrow it to one `(benchmark, metric)` series per discriminant set, and render
/// the ordered data points.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the analyze selection pipeline, which threads the same injected ports"
)]
pub(crate) async fn examine_with<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    options: &ExamineOptions,
    auto: &AutoFacets,
    now: Timestamp,
    reporter: &dyn Reporter,
    spawner: &Spawner,
) -> Result<RenderedReports, AnalyzeError>
where
    G: GitHistory,
    S: Storage + Clone + 'static,
{
    let request = ReportRequest::resolve(
        options.no_text,
        options.markdown.as_deref(),
        options.json.as_deref(),
    )?;

    // Reject an unknown metric name up front, before any load — the one command
    // that names a metric validates it against the known set.
    let metric_kind = parse_metric(&options.metric)?;

    // The benchmark identity scopes the series load coarsely (a prefix of the
    // qualified id); the exact `id == benchmark` narrowing happens after series
    // reconstruction. An unmatched id is not an error — it yields an empty pivot.
    let prefix = BenchmarkIdPrefix::new(options.benchmark.clone()).map_err(|_empty| {
        AnalyzeError::Analyze {
            message: "--benchmark must not be empty".to_owned(),
        }
    })?;
    let prefixes = [prefix];
    let filter = SeriesFilter {
        prefixes: &prefixes,
    };

    let selection = Selection::from_examine(options);
    let dataset = select_dataset(
        git, storage, project_id, config, &selection, filter, auto, now, reporter, spawner,
    )
    .await?;

    let pivot = build_pivot(
        project_id,
        &options.benchmark,
        metric_kind,
        &dataset.series,
        &dataset.commit_subjects,
    );

    // When the pivot is empty, explain why: either no run entered the selection at
    // all (the same self-explaining hint `analyze` gives), or runs entered but none
    // carried this `(benchmark, metric)` pair (a data-dependent id/name mismatch).
    let hint = if !pivot.sets.is_empty() {
        None
    } else if dataset.run_index.is_empty() {
        empty_history_hint(
            true,
            dataset.candidate_count,
            &dataset.target_ref,
            dataset.tally,
            &dataset.facets,
        )
    } else {
        Some(unmatched_series_hint(
            &options.benchmark,
            metric_kind,
            dataset.run_index.total(),
        ))
    };

    // The same ephemeral-data warning `analyze`/`list` show when a dirty
    // base-branch-tip run was admitted because the working tree is dirty.
    let warning = dataset
        .included_dirty_base_exception
        .then(dirty_base_exception_warning);

    Ok(request.render(|format| render_pivot(&pivot, format, hint.as_deref(), warning.as_deref())))
}

/// Parses the `--metric` value into a [`MetricKind`], rejecting an unknown name
/// with the list of valid names.
fn parse_metric(name: &str) -> Result<MetricKind, AnalyzeError> {
    MetricKind::from_name(name).ok_or_else(|| {
        let valid = MetricKind::ALL
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        AnalyzeError::Analyze {
            message: format!("unknown metric {name:?}; expected one of: {valid}"),
        }
    })
}

/// One recorded observation of the examined series.
#[derive(Clone, Debug)]
struct DataPoint {
    /// The commit the run was measured against (full commit ID, or a label in tests).
    commit: String,
    /// The short commit id shown in the text and Markdown tables.
    short_commit: String,
    /// The measured value.
    value: f64,
    /// Whether the observation came from a dirty (uncommitted-tree) snapshot.
    dirty: bool,
    /// The commit's full title (subject), empty when topology reported none.
    title: String,
}

/// One discriminant set's slice of the pivot.
#[derive(Clone, Debug)]
struct SetPivot {
    /// The comparable partition these points share.
    set: DiscriminantSet,
    /// The observations, oldest first by git topology (clean before dirty).
    points: Vec<DataPoint>,
}

/// The fully resolved pivot of one `(benchmark, metric)` series, ready to render.
#[derive(Clone, Debug)]
struct Pivot {
    /// The project the data belongs to.
    project: String,
    /// The qualified benchmark identity that was examined.
    benchmark: String,
    /// The metric's stable name.
    metric: &'static str,
    /// The metric's unit, for display.
    unit: &'static str,
    /// The per-set slices, one entry per discriminant set carrying the series.
    sets: Vec<SetPivot>,
}

/// Narrows the reconstructed series to the one `(benchmark, metric)` pair, once per
/// discriminant set, and turns each into its ordered data points.
fn build_pivot(
    project_id: &str,
    benchmark: &str,
    metric_kind: MetricKind,
    series: &[Series],
    commit_subjects: &HashMap<String, String>,
) -> Pivot {
    let mut sets: Vec<SetPivot> = series
        .iter()
        .filter(|one| one.kind == metric_kind && one.id.qualified() == benchmark)
        .map(|one| {
            let points = one
                .points
                .iter()
                .map(|point| {
                    let commit = point.commit.as_deref().unwrap_or("unknown");
                    let title = commit_subjects.get(commit).cloned().unwrap_or_default();
                    DataPoint {
                        short_commit: short_commit_id(commit).to_owned(),
                        commit: commit.to_owned(),
                        value: point.value,
                        dirty: point.dirty,
                        title,
                    }
                })
                .collect();
            SetPivot {
                set: one.set.clone(),
                points,
            }
        })
        .collect();
    // Sort the sets deterministically so the same data set always renders in the
    // same order regardless of series-build order.
    sets.sort_by(|left, right| left.set.cmp(&right.set));

    Pivot {
        project: project_id.to_owned(),
        benchmark: benchmark.to_owned(),
        metric: metric_kind.as_str(),
        unit: metric_kind.as_unit(),
        sets,
    }
}

/// The hint shown when runs entered the selection but none carried the examined
/// `(benchmark, metric)` pair — a data-dependent id or name mismatch.
fn unmatched_series_hint(benchmark: &str, metric: MetricKind, entered: usize) -> String {
    format!(
        "{} entered the analysis, but none recorded benchmark {benchmark:?} with metric {:?}. \
         Check the benchmark id and metric name — copy them verbatim from an `analyze` finding \
         (`list runs` shows the benchmark ids present in the data set).",
        count_noun(entered, "run"),
        metric.as_str(),
    )
}

/// The first [`TITLE_LIMIT`] characters of a commit title, for the text and
/// Markdown tables.
fn truncate_title(title: &str) -> String {
    title.chars().take(TITLE_LIMIT).collect()
}

/// The short commit id: the first 12 characters of the commit ID (mirrors the
/// abbreviation `list`, `bless`, and `backfill` use).
fn short_commit_id(commit_id: &str) -> &str {
    commit_id.get(..12).unwrap_or(commit_id)
}

/// The line chart of a set's ordered values, drawn before its data points (the same
/// chart `analyze` renders for a finding). `None` when there are too few points to plot.
fn chart_of_points(points: &[DataPoint]) -> Option<String> {
    let values: Vec<f64> = points.iter().map(|point| point.value).collect();
    chart(&values)
}

/// Renders the pivot in the requested format, appending the diagnostic hint and
/// ephemeral-data warning (if any).
fn render_pivot(
    pivot: &Pivot,
    format: ReportFormat,
    hint: Option<&str>,
    warning: Option<&str>,
) -> String {
    match format {
        ReportFormat::Text => render_pivot_text(pivot, hint, warning),
        ReportFormat::Markdown => render_pivot_markdown(pivot, hint, warning),
        ReportFormat::Json => render_pivot_json(pivot, hint, warning),
    }
}

fn render_pivot_text(pivot: &Pivot, hint: Option<&str>, warning: Option<&str>) -> String {
    let mut lines = vec![format!(
        "Data points for {} metric {} ({}) in project {}",
        pivot.benchmark, pivot.metric, pivot.unit, pivot.project
    )];
    if pivot.sets.is_empty() {
        lines.push(String::new());
        lines.push("No data point matches the selection.".to_owned());
    } else {
        for set in &pivot.sets {
            lines.push(String::new());
            lines.push(set.set.to_string());
            // Lead the set with the same small line chart `analyze` draws, so a
            // maintainer sees the shape of the series before reading the points it
            // pivots.
            if let Some(chart) = chart_of_points(&set.points) {
                lines.push(chart);
                lines.push(String::new());
            }
            // Align the commit and value columns so a maintainer can read the values
            // straight down and spot where one jumps.
            let commit_width = set
                .points
                .iter()
                .map(|point| point.short_commit.len())
                .max()
                .unwrap_or(0);
            let values: Vec<String> = set
                .points
                .iter()
                .map(|point| format_value(point.value))
                .collect();
            let value_width = values.iter().map(String::len).max().unwrap_or(0);
            for (point, value) in set.points.iter().zip(&values) {
                let title = truncate_title(&point.title);
                let marker = if point.dirty { "  (dirty)" } else { "" };
                lines.push(format!(
                    "  {commit:<commit_width$}  {value:>value_width$}  {title}{marker}",
                    commit = point.short_commit,
                ));
            }
        }
    }
    append_hint_and_warning(&mut lines, hint, warning);
    format!("{}\n", lines.join("\n"))
}

fn render_pivot_markdown(pivot: &Pivot, hint: Option<&str>, warning: Option<&str>) -> String {
    let mut lines = vec![
        format!("# Data points for {} in {}", pivot.benchmark, pivot.project),
        String::new(),
        format!("**Metric:** {} ({})", pivot.metric, pivot.unit),
    ];
    if pivot.sets.is_empty() {
        lines.push(String::new());
        lines.push("No data point matches the selection.".to_owned());
    } else {
        for set in &pivot.sets {
            lines.push(String::new());
            lines.push(format!("## {}", set.set));
            // The same series chart the text pivot draws, fenced as a `text`
            // block so it survives Markdown rendering (mirrors `analyze`).
            if let Some(chart) = chart_of_points(&set.points) {
                lines.push(String::new());
                lines.push("```text".to_owned());
                lines.push(chart);
                lines.push("```".to_owned());
            }
            lines.push(String::new());
            lines.push("| Commit | Value | Kind | Title |".to_owned());
            lines.push("| --- | --- | --- | --- |".to_owned());
            for point in &set.points {
                let kind = if point.dirty { "dirty" } else { "clean" };
                lines.push(format!(
                    "| {} | {} | {} | {} |",
                    point.short_commit,
                    format_value(point.value),
                    kind,
                    escape_cell(&truncate_title(&point.title)),
                ));
            }
        }
    }
    append_hint_and_warning(&mut lines, hint, warning);
    format!("{}\n", lines.join("\n"))
}

fn render_pivot_json(pivot: &Pivot, hint: Option<&str>, warning: Option<&str>) -> String {
    #[derive(Serialize)]
    struct JsonPoint<'a> {
        commit: &'a str,
        value: f64,
        dirty: bool,
        title: &'a str,
    }
    #[derive(Serialize)]
    struct JsonSet<'a> {
        engine: &'a str,
        target_triple: &'a str,
        machine_key: &'a str,
        points: Vec<JsonPoint<'a>>,
    }
    #[derive(Serialize)]
    struct JsonPivot<'a> {
        project: &'a str,
        benchmark: &'a str,
        metric: &'a str,
        unit: &'a str,
        sets: Vec<JsonSet<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        warning: Option<&'a str>,
    }

    let sets: Vec<JsonSet<'_>> = pivot
        .sets
        .iter()
        .map(|set| JsonSet {
            engine: &set.set.engine,
            target_triple: &set.set.target_triple,
            machine_key: &set.set.machine_key,
            points: set
                .points
                .iter()
                .map(|point| JsonPoint {
                    commit: &point.commit,
                    value: point.value,
                    dirty: point.dirty,
                    title: &point.title,
                })
                .collect(),
        })
        .collect();

    let document = JsonPivot {
        project: &pivot.project,
        benchmark: &pivot.benchmark,
        metric: pivot.metric,
        unit: pivot.unit,
        sets,
        hint,
        warning,
    };
    serde_json::to_string_pretty(&document).expect("pivot structures always serialize to JSON")
}

/// Escapes a Markdown table cell so a commit title's pipe characters do not break
/// the table's column layout.
fn escape_cell(value: &str) -> String {
    value.replace('|', "\\|")
}

/// Appends the hint and warning (if any) as trailing, blank-line-separated blocks
/// so they read at the very end of a text or Markdown pivot.
fn append_hint_and_warning(lines: &mut Vec<String>, hint: Option<&str>, warning: Option<&str>) {
    if let Some(hint) = hint {
        lines.push(String::new());
        lines.push(hint.to_owned());
    }
    if let Some(warning) = warning {
        lines.push(String::new());
        lines.push(warning.to_owned());
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
    use std::path::PathBuf;

    use cbh_config::Config;
    use cbh_diag::RecordingReporter;
    use cbh_git::FakeGitHistory;
    use cbh_model::{
        BenchmarkId, BenchmarkResult, EnvironmentInfo, GitInfo, Metric, MetricKind, Run,
        RunContext, ToolchainInfo,
    };
    use cbh_storage::{MemoryStorage, Storage};
    use futures::executor::block_on;
    use jiff::Timestamp;
    use nonempty::nonempty;

    use super::*;

    fn config() -> Config {
        Config::default()
    }

    /// The auto-detected facets the tests seed their default partition under.
    fn auto() -> AutoFacets {
        AutoFacets {
            triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine_key: "m1".to_owned(),
        }
    }

    fn options() -> ExamineOptions {
        ExamineOptions {
            benchmark: "nm/nm::observe/pull".to_owned(),
            metric: "instruction_count".to_owned(),
            ..ExamineOptions::default()
        }
    }

    /// An inline spawner that runs the load tasks on the calling thread, so the
    /// tests need no Tokio runtime under `block_on` or Miri.
    fn spawner() -> Spawner {
        cbh_detect::testing::synchronous_spawner()
    }

    /// A run with one benchmark carrying a single instruction-count metric of the
    /// given value on the given commit.
    fn single_metric_run(effective: i64, commit: &str, value: f64) -> Run {
        let time = Timestamp::from_second(effective).unwrap();
        let context = RunContext::new(
            time,
            GitInfo {
                commit: Some(commit.to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            EnvironmentInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        let record = BenchmarkResult::new(
            BenchmarkId::new(nonempty![
                "nm".to_owned(),
                "nm::observe".to_owned(),
                "pull".to_owned(),
            ]),
            vec![Metric::new(MetricKind::InstructionCount, value)],
        );
        Run::new(context, vec![record])
    }

    /// A run whose one benchmark carries two metrics, so the partition reconstructs
    /// two distinct series (only one of which any single `--metric` selects).
    fn two_metric_run(effective: i64, commit: &str, ir: f64, branches: f64) -> Run {
        let time = Timestamp::from_second(effective).unwrap();
        let context = RunContext::new(
            time,
            GitInfo {
                commit: Some(commit.to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            EnvironmentInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        let record = BenchmarkResult::new(
            BenchmarkId::new(nonempty![
                "nm".to_owned(),
                "nm::observe".to_owned(),
                "pull".to_owned(),
            ]),
            vec![
                Metric::new(MetricKind::InstructionCount, ir),
                Metric::new(MetricKind::ConditionalBranches, branches),
            ],
        );
        Run::new(context, vec![record])
    }

    fn clean_key(commit: &str) -> String {
        format!("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/{commit}/clean.json")
    }

    fn dirty_key(commit: &str, unix: i64) -> String {
        format!("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/{commit}/dirty-{unix}.json")
    }

    fn store(storage: &MemoryStorage, key: &str, run: &Run) {
        let json = run.to_json().unwrap();
        block_on(storage.put(key, json.as_bytes())).unwrap();
    }

    /// A linear history `c0 <- c1 <- c2 <- c3` with commit titles, on the default
    /// branch `master`.
    fn linear_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .subject("c0", "Add the pull benchmark")
            .subject("c1", "Optimize the hot loop")
            .subject(
                "c2",
                "Refactor the observer to shave allocations off the record path",
            )
            .branch("master", "c3")
            .head("master")
            .mark_default("master");
        git
    }

    /// Drives `examine_with` and unwraps the rendered text message.
    fn examine(storage: &MemoryStorage, git: &FakeGitHistory, options: &ExamineOptions) -> String {
        let rendered = block_on(examine_with(
            git,
            storage,
            "folo",
            &config(),
            options,
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &spawner(),
        ))
        .unwrap();
        rendered
            .text
            .expect("examine renders the text report by default")
    }

    /// Drives `examine_with` requesting the JSON report and returns the JSON text
    /// (the text report is suppressed).
    fn examine_json(
        storage: &MemoryStorage,
        git: &FakeGitHistory,
        options: &ExamineOptions,
    ) -> String {
        let mut options = options.clone();
        options.no_text = true;
        options.markdown = None;
        options.json = Some(PathBuf::from("report.json"));
        let rendered = block_on(examine_with(
            git,
            storage,
            "folo",
            &config(),
            &options,
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &spawner(),
        ))
        .unwrap();
        rendered
            .json
            .expect("the JSON report was rendered for the requested path")
    }

    /// Drives `examine_with` requesting the Markdown report and returns the Markdown
    /// text.
    fn examine_markdown(
        storage: &MemoryStorage,
        git: &FakeGitHistory,
        options: &ExamineOptions,
    ) -> String {
        let mut options = options.clone();
        options.no_text = true;
        options.json = None;
        options.markdown = Some(PathBuf::from("report.md"));
        let rendered = block_on(examine_with(
            git,
            storage,
            "folo",
            &config(),
            &options,
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &spawner(),
        ))
        .unwrap();
        rendered
            .markdown
            .expect("the Markdown report was rendered for the requested path")
    }

    #[test]
    fn pivots_one_series_into_ordered_points() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &single_metric_run(0, "c0", 100.0),
        );
        store(
            &storage,
            &clean_key("c1"),
            &single_metric_run(1, "c1", 130.0),
        );
        store(
            &storage,
            &clean_key("c2"),
            &single_metric_run(2, "c2", 128.0),
        );
        let git = linear_git();

        let report = examine_json(&storage, &git, &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();

        assert_eq!(parsed["benchmark"], "nm/nm::observe/pull");
        assert_eq!(parsed["metric"], "instruction_count");
        assert_eq!(parsed["unit"], "count");
        let sets = parsed["sets"].as_array().unwrap();
        assert_eq!(sets.len(), 1);
        let points = sets[0]["points"].as_array().unwrap();
        assert_eq!(points.len(), 3);
        // Oldest first by topology.
        assert_eq!(points[0]["commit"], "c0");
        assert_eq!(points[0]["value"], 100.0);
        assert_eq!(points[0]["dirty"], false);
        assert_eq!(points[0]["title"], "Add the pull benchmark");
        assert_eq!(points[2]["commit"], "c2");
        assert_eq!(points[2]["value"], 128.0);
    }

    #[test]
    fn json_keeps_full_precision_and_full_title() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c2"),
            &single_metric_run(2, "c2", 128.499_999),
        );
        let git = linear_git();

        let report = examine_json(&storage, &git, &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let point = &parsed["sets"][0]["points"][0];
        assert_eq!(point["value"], 128.499_999);
        // The full, untruncated title (54 chars) survives in JSON.
        assert_eq!(
            point["title"],
            "Refactor the observer to shave allocations off the record path"
        );
    }

    #[test]
    fn text_truncates_the_title_to_fifty_characters() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c2"),
            &single_metric_run(2, "c2", 128.0),
        );
        let git = linear_git();

        let report = examine(&storage, &git, &options());
        assert!(
            report.contains("Data points for nm/nm::observe/pull"),
            "{report}"
        );
        // The 62-char subject is cut to its first 50 characters ("...off the"),
        // dropping the trailing " record path".
        assert!(
            report.contains("Refactor the observer to shave allocations off the"),
            "{report}"
        );
        assert!(
            !report.contains("record path"),
            "the title is truncated to 50 characters: {report}"
        );
    }

    #[test]
    fn flags_dirty_snapshots_after_the_clean_run() {
        // On the base branch (linear history, tip == merge-base) with a dirty
        // working tree, the tip's clean run and its dirty snapshot both show — the
        // dirty snapshot admitted via the base-tip dirty exception, ordered after
        // the clean run and flagged.
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c3"),
            &single_metric_run(3, "c3", 100.0),
        );
        store(
            &storage,
            &dirty_key("c3", 400),
            &single_metric_run(4, "c3", 118.0),
        );
        let mut git = linear_git();
        git.mark_dirty();

        let report = examine_json(&storage, &git, &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let points = parsed["sets"][0]["points"].as_array().unwrap();
        assert_eq!(points.len(), 2, "clean and dirty both shown: {report}");
        assert_eq!(points[0]["dirty"], false, "clean first");
        assert_eq!(points[0]["value"], 100.0);
        assert_eq!(points[1]["dirty"], true, "dirty second");
        assert_eq!(points[1]["value"], 118.0);
        // The dirty tip snapshot is ephemeral, so the pivot carries the warning.
        assert!(
            parsed["warning"].as_str().is_some(),
            "ephemeral-data warning present: {report}"
        );

        let text = examine(&storage, &git, &options());
        assert!(text.contains("(dirty)"), "the dirty row is flagged: {text}");
    }

    #[test]
    fn selects_only_the_named_metric_of_a_multi_metric_benchmark() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &two_metric_run(0, "c0", 100.0, 250.0),
        );
        let git = linear_git();

        let report = examine_json(&storage, &git, &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let points = parsed["sets"][0]["points"].as_array().unwrap();
        assert_eq!(points.len(), 1);
        // The instruction-count value, not the conditional-branches one.
        assert_eq!(points[0]["value"], 100.0);

        let branches = ExamineOptions {
            metric: "conditional_branches".to_owned(),
            ..options()
        };
        let report = examine_json(&storage, &git, &branches);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["sets"][0]["points"][0]["value"], 250.0);
    }

    #[test]
    fn markdown_renders_a_per_set_table() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &single_metric_run(0, "c0", 100.0),
        );
        let git = linear_git();

        let report = examine_markdown(&storage, &git, &options());
        assert!(
            report.contains("# Data points for nm/nm::observe/pull in folo"),
            "{report}"
        );
        assert!(
            report.contains("**Metric:** instruction_count (count)"),
            "{report}"
        );
        assert!(
            report.contains("| Commit | Value | Kind | Title |"),
            "{report}"
        );
        assert!(report.contains("| c0 | 100 | clean |"), "{report}");
    }

    #[test]
    fn text_draws_a_chart_before_the_points() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &single_metric_run(0, "c0", 100.0),
        );
        store(
            &storage,
            &clean_key("c1"),
            &single_metric_run(1, "c1", 130.0),
        );
        store(
            &storage,
            &clean_key("c2"),
            &single_metric_run(2, "c2", 128.0),
        );
        let git = linear_git();

        let report = examine(&storage, &git, &options());
        // The rasciigraph axis marker proves the chart was drawn.
        let axis = report
            .find('┤')
            .or_else(|| report.find('┼'))
            .expect("a chart is drawn");
        // It leads the set: the chart precedes the first point row (the c0 title).
        let first_point = report
            .find("Add the pull benchmark")
            .expect("the first point row is present");
        assert!(
            axis < first_point,
            "the chart precedes the points: {report}"
        );
        // The chart carries no color (no ANSI escapes).
        assert!(!report.contains('\u{1b}'), "no ANSI escape: {report:?}");
    }

    #[test]
    fn markdown_fences_the_chart_before_the_table() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &single_metric_run(0, "c0", 100.0),
        );
        store(
            &storage,
            &clean_key("c1"),
            &single_metric_run(1, "c1", 130.0),
        );
        let git = linear_git();

        let report = examine_markdown(&storage, &git, &options());
        let fence = report.find("```text").expect("the chart is fenced");
        assert!(
            report.contains('┤') || report.contains('┼'),
            "a chart is drawn: {report}"
        );
        // The fenced chart precedes the per-set table.
        let table = report
            .find("| Commit | Value | Kind | Title |")
            .expect("the table header is present");
        assert!(fence < table, "the chart precedes the table: {report}");
    }

    #[test]
    fn a_single_point_set_draws_no_chart() {
        // A lone observation has too few points to plot, so no chart is drawn.
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &single_metric_run(0, "c0", 100.0),
        );
        let git = linear_git();

        let report = examine(&storage, &git, &options());
        assert!(
            !report.contains('┤') && !report.contains('┼'),
            "no chart for a single point: {report}"
        );
    }

    #[test]
    fn unknown_metric_is_rejected_up_front() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &single_metric_run(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = ExamineOptions {
            metric: "not_a_metric".to_owned(),
            ..options()
        };
        let error = block_on(examine_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &spawner(),
        ))
        .unwrap_err();
        match error {
            AnalyzeError::Analyze { message } => {
                assert!(message.contains("unknown metric"), "{message}");
                assert!(message.contains("instruction_count"), "{message}");
            }
            other => panic!("expected an Analyze error, got {other:?}"),
        }
    }

    #[test]
    fn unmatched_benchmark_yields_empty_pivot_with_hint() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &single_metric_run(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = ExamineOptions {
            benchmark: "nm/nm::observe/nonexistent".to_owned(),
            ..options()
        };
        let report = examine(&storage, &git, &opts);
        assert!(
            report.contains("No data point matches the selection."),
            "{report}"
        );
        // Runs entered, but none carried the pair: the id/name-mismatch hint.
        assert!(report.contains("entered the analysis"), "{report}");
        assert!(report.contains("nonexistent"), "{report}");
    }

    #[test]
    fn requires_a_repository() {
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &single_metric_run(0, "c0", 100.0),
        );
        let git = FakeGitHistory::new(); // No commits: HEAD does not resolve.
        let error = block_on(examine_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, AnalyzeError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn pivots_each_matching_discriminant_set() {
        // The same benchmark and metric recorded under two engines.
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &single_metric_run(0, "c0", 100.0),
        );
        store(
            &storage,
            "v1/folo/objects/criterion/x86_64-unknown-linux-gnu/m1/c0/clean.json",
            &single_metric_run(0, "c0", 200.0),
        );
        let git = linear_git();

        let opts = ExamineOptions {
            engine: vec!["all".to_owned()],
            ..options()
        };
        let report = examine_json(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let sets = parsed["sets"].as_array().unwrap();
        assert_eq!(sets.len(), 2, "one pivot per discriminant set: {report}");
    }

    #[test]
    fn parse_metric_lists_every_valid_name_on_error() {
        let error = parse_metric("bogus").unwrap_err();
        let AnalyzeError::Analyze { message } = error else {
            panic!("expected an Analyze error");
        };
        for kind in MetricKind::ALL {
            assert!(
                message.contains(kind.as_str()),
                "the error should list {}: {message}",
                kind.as_str()
            );
        }
    }

    #[test]
    fn escape_cell_backslash_escapes_pipes() {
        // A commit title containing a pipe would otherwise open extra Markdown
        // table columns, so each pipe must be backslash-escaped.
        assert_eq!(escape_cell("feat: parse a|b"), "feat: parse a\\|b");
        // Text with no pipe passes through byte-for-byte.
        assert_eq!(escape_cell("plain title"), "plain title");
    }
}
