//! The `analyze` orchestration entry points: [`execute`] wires the real adapters
//! and [`analyze_with`] is the storage- and git-generic orchestrator that the sibling
//! modules (`selection`, `facets`, `load`, `dataset`, `history`, `window`) compose.
//! The parent module re-exports the surface the sibling query commands
//! (`list`, `prune`, `examine`, `bless`) share.

use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyspawn::Spawner;
use cbh_command::AnalyzeOptions;
use cbh_config::{
    Config, cache_env, load_config, resolve_cache_path, resolve_config_path, resolve_local_path,
    resolve_project_id, resolve_repo, storage_env,
};
use cbh_detect::{
    AnalysisConfig, AnalysisContext, Series, SeriesFilter, apply_blessings, find_changes_spawned,
    retain_present_at_context, short_commit,
};
use cbh_diag::{Reporter, ReporterExt, StderrReporter, count_noun};
use cbh_git::{GitHistory, SystemGitHistory};
use cbh_model::DiscriminantSet;
use cbh_probe::{EnvironmentProbe, SystemProbe, resolve_machine_key};
use cbh_render::{DEFAULT_SUMMARY_LIMIT, ReportInput, SetSummary, render, render_markdown_summary};
use cbh_storage::{Storage, StorageFacade, resolve_storage};
use jiff::Timestamp;
use tick::Clock;

use super::dataset::{empty_history_hint, select_dataset};
use super::facets::AutoFacets;
use super::history::dirty_base_exception_warning;
use super::selection::Selection;
use crate::{AnalyzeError, RenderedReports, ReportRequest};

/// The real `analyze`: load configuration, wire the configured storage and git
/// history, and orchestrate.
///
/// `clock_override` injects the [`tick::Clock`] the analysis anchors its "now" to:
/// `None` reads the runtime wall clock (`Clock::new_tokio`), while tests pass a
/// frozen clock (`Clock::new_frozen_at`) so the anchor is deterministic. That
/// single anchor drives both the history-mode default `--since` look-back and the
/// resolution of any relative `--since` duration, so the cutoff is
/// deterministic under a frozen clock.
///
/// Returns the rendered reports for the requested formats plus the regression
/// count; the shell writes the files and prints the text report.
// Thin real-adapter wiring: loads config from disk, builds the configured storage,
// and shells out via `SystemGitHistory`/`detect_auto_facets` before delegating every
// decision to the mutation-tested `analyze_with`. In-crate tests cannot drive these
// real adapters deterministically; the binary's integration tests cover this edge.
#[cfg_attr(test, mutants::skip)]
pub async fn execute(
    options: &AnalyzeOptions,
    workspace_dir: &Path,
    clock_override: Option<Clock>,
    storage_override: Option<StorageFacade>,
    auto_override: Option<AutoFacets>,
) -> Result<(RenderedReports, usize), AnalyzeError> {
    // Per-object notes follow `--verbose`; stage timings are emitted under either
    // `--verbose` or the programmatic `timing` flag (the stress harness sets the
    // latter alone to see the load breakdown without the per-object flood).
    let reporter = StderrReporter::with_timing(options.verbose, options.stage_timings_enabled());

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
    // Reconcile the read-through cache (if any) with the cloud before loading, so a
    // stale mirror is wiped rather than served.
    storage.synchronize_cache(&project_id, &reporter).await?;

    let git = SystemGitHistory::new(resolve_repo(workspace_dir, options.repo.as_deref()));
    let auto = resolve_auto_facets(auto_override).await?;

    let now = resolve_now(clock_override);
    let color = should_colorize(
        std::io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
    );
    // Distribute the compute-bound detection across the runtime's blocking pool, so
    // the analysis shares the ambient Tokio worker threads rather than spawning its
    // own short-lived ones.
    let spawner = Spawner::new_tokio();
    let outcome = analyze_with(
        &git,
        &storage,
        &project_id,
        &config,
        options,
        &auto,
        now,
        &reporter,
        color,
        &spawner,
    )
    .await;
    // Surface the cache hit/miss tally after the load, so a slow analyze can be
    // diagnosed as a cold or invalidated mirror regardless of the load's outcome.
    storage.report_cache_tally(&reporter);
    outcome
}

/// Reads the analysis anchor instant from a [`tick::Clock`], the single source of
/// wall-clock time for the whole analyze family (`analyze`/`list`/`examine`/`prune`/
/// `bless`).
///
/// Production passes `clock_override: None` and reads the runtime clock
/// (`Clock::new_tokio`); tests inject a frozen clock (`Clock::new_frozen_at`) so the
/// resolved window is deterministic. Sourcing the instant through the clock keeps
/// time injectable rather than minting it from a bare `Timestamp::now()`.
pub(crate) fn resolve_now(clock_override: Option<Clock>) -> Timestamp {
    clock_override
        .unwrap_or_else(Clock::new_tokio)
        .system_time_as::<Timestamp>()
}

/// Whether colored output should be emitted: only to an interactive terminal with
/// `NO_COLOR` unset.
fn should_colorize(is_terminal: bool, no_color: bool) -> bool {
    is_terminal && !no_color
}

/// Probes the current machine's auto-detect facet values for the query commands.
///
/// The host triple comes from `rustc -vV` (with a platform fallback) and the
/// machine key from the hardware fingerprint. There is no engine probe — a bare
/// query analyzes every engine. Tests drive the generic orchestrators directly
/// with deterministic [`AutoFacets`] instead of calling this.
#[cfg_attr(test, mutants::skip)] // Probes the host environment; the facet resolution it feeds is tested.
pub(crate) async fn detect_auto_facets() -> Result<AutoFacets, AnalyzeError> {
    let probe = SystemProbe::default();
    let toolchain = probe.toolchain().await.map_err(AnalyzeError::Io)?;
    let hardware = probe.hardware().await;
    Ok(AutoFacets {
        triple: toolchain.host.unwrap_or_default(),
        machine_key: resolve_machine_key(None, &hardware),
    })
}

/// Resolves the auto-detect facets for a query command, preferring an injected
/// override over probing the host.
///
/// Production passes `None` and probes via [`detect_auto_facets`]; the binary's
/// integration tests inject deterministic [`AutoFacets`] through the `Overrides`
/// test hook so the suite is independent of the host it runs on.
#[cfg_attr(test, mutants::skip)] // Trivial override-or-probe selection; the probe path is host-dependent.
pub(crate) async fn resolve_auto_facets(
    auto_override: Option<AutoFacets>,
) -> Result<AutoFacets, AnalyzeError> {
    match auto_override {
        Some(auto) => Ok(auto),
        None => detect_auto_facets().await,
    }
}

/// Storage- and git-generic `analyze`: facet-filter the stored objects, resolve
/// the git topology, select the comparable commits, build the series, detect
/// changes, and render a report for the requested format.
///
/// `color` enables ANSI styling and colored charts in the text report; callers
/// pass the terminal-detection result so piped output and tests stay plain.
#[expect(
    clippy::too_many_arguments,
    reason = "analyze orchestration wires several injected ports plus the rendering color flag"
)]
pub(crate) async fn analyze_with<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    options: &AnalyzeOptions,
    auto: &AutoFacets,
    now: Timestamp,
    reporter: &dyn Reporter,
    color: bool,
    spawner: &Spawner,
) -> Result<(RenderedReports, usize), AnalyzeError>
where
    G: GitHistory,
    S: Storage + Clone + 'static,
{
    let request = ReportRequest::resolve_analyze(
        options.no_text,
        options.markdown.as_deref(),
        options.json.as_deref(),
        options.markdown_summary.as_deref(),
    )?;
    let selection = Selection::from_analyze(options);
    let filter = SeriesFilter {
        prefixes: &options.prefixes,
    };
    let load_started = Instant::now();
    let dataset = select_dataset(
        git, storage, project_id, config, &selection, filter, auto, now, reporter, spawner,
    )
    .await?;
    reporter.timing(
        "select_dataset (full load: list + filter + topology + fetch/parse/fold + build)",
        load_started.elapsed(),
    );

    let mut series = dataset.series;

    // Ghost filtering: analyze only benchmarks that still exist at the context
    // commit. A benchmark that appears only for past commits — renamed, removed,
    // or replaced — is a "ghost"; re-flagging it is noise. Dropping ghosts
    // *before* detection also keeps them out of the false-discovery-rate
    // correction, so a removed benchmark cannot dilute the correction for real
    // ones. Presence is read from the raw points, independent of re-baselining.
    let ghosts_excluded = {
        let ghost_started = Instant::now();
        let ghosts = retain_present_at_context(&mut series, &dataset.tip_commit);
        for (set, id) in &ghosts {
            reporter.note_with(|| {
                format!(
                    "excluding benchmark {} from {set}: not present at the context commit {}",
                    id.qualified(),
                    short_commit(&dataset.tip_commit),
                )
            });
        }
        reporter.note_with(|| {
            format!(
                "ghost filter: excluded {} not present at the context commit {}; {} remain",
                count_noun(ghosts.len(), "ghost benchmark"),
                short_commit(&dataset.tip_commit),
                count_noun(series.len(), "series"),
            )
        });
        reporter.timing(
            "ghost filter (retain_present_at_context)",
            ghost_started.elapsed(),
        );
        ghosts.len()
    };

    // Re-baseline blessed series before detection (history mode only; branch
    // mode carries an empty blessing map).
    let rebaseline_started = Instant::now();
    apply_blessings(&mut series, &dataset.blessings);
    reporter.timing(
        "re-baseline blessed series (apply_blessings)",
        rebaseline_started.elapsed(),
    );
    let context = AnalysisContext {
        mode: dataset.mode,
        config: AnalysisConfig::default(),
        merge_base_index: dataset.merge_base_index,
        include_improvements: options.include_improvements,
        include_inactive: options.include_inactive,
    };
    // Share the series across the detection's blocking tasks without copying; the
    // remaining per-set reporting reads them back through this same handle.
    let series: Arc<[Series]> = Arc::from(series);
    let detect_started = Instant::now();
    let findings = find_changes_spawned(Arc::clone(&series), context, spawner).await;
    reporter.timing(
        "change detection (find_changes: per-series detectors + FDR filter)",
        detect_started.elapsed(),
    );
    let regressions = findings
        .iter()
        .filter(|finding| finding.is_regression())
        .count();
    let notable = !findings.is_empty();

    // Break the report down by comparable set so each partition reads on its own.
    let mut sets: Vec<DiscriminantSet> = series.iter().map(|one| one.set.clone()).collect();
    sets.sort();
    sets.dedup();
    let summaries: Vec<SetSummary<'_>> = sets
        .iter()
        .map(|set| SetSummary {
            set,
            runs: dataset.run_index.runs_in_set(set),
            series: series.iter().filter(|one| &one.set == set).count(),
            findings: findings
                .iter()
                .filter(|finding| &finding.set == set)
                .collect(),
        })
        .collect();

    // When stored runs existed but none entered the analysis, the empty outcome is
    // otherwise indistinguishable from "no data". Explain the dominant reasons so
    // the user can act without resorting to `--verbose`.
    //
    // The ghost filter is a distinct empty case: runs *did* load and analyze, but
    // every benchmark was dropped as a ghost. `empty_history_hint` keys off an
    // empty load and stays silent here, so name the ghost case on its own.
    let hint = if ghosts_excluded > 0 && series.is_empty() {
        Some(all_ghosts_hint(&dataset.tip_commit))
    } else {
        empty_history_hint(
            dataset.run_index.is_empty(),
            dataset.candidate_count,
            &dataset.target_ref,
            dataset.tally,
            &dataset.facets,
        )
    };

    // Admitting a dirty snapshot on the base branch's tip is a courtesy for the
    // "evaluating the tool" / "accidentally working on the base branch" cases; warn
    // that such data is not persisted across commits.
    let warning = dataset
        .included_dirty_base_exception
        .then(dirty_base_exception_warning);

    let input = ReportInput {
        project: project_id,
        tip_commit: &dataset.tip_commit,
        tip_dirty: dataset.tip_dirty,
        mode: dataset.mode.as_str(),
        notable,
        runs: dataset.run_index.total(),
        series: series.len(),
        commit_span: dataset.run_index.commit_span(),
        report_improvements: context.reports_improvements(),
        findings: &findings,
        sets: &summaries,
        hint: hint.as_deref(),
        warning: warning.as_deref(),
        ghosts_excluded,
    };
    let render_started = Instant::now();
    let rendered = request.render_analyze(
        |format| render(&input, format, color),
        || render_markdown_summary(&input, DEFAULT_SUMMARY_LIMIT),
    );
    reporter.timing("report render", render_started.elapsed());

    Ok((rendered, regressions))
}

/// The empty-outcome hint for the all-ghosts case: runs loaded and analyzed, but
/// the ghost filter dropped every benchmark because none is present at the context
/// commit. Distinct from [`empty_history_hint`], which speaks only to an empty load.
fn all_ghosts_hint(tip_commit: &str) -> String {
    format!(
        "Runs were analyzed, but every benchmark was filtered as a ghost — none is \
         present at the context commit {}. This usually means the context commit has \
        no stored runs (collect at the context commit), or its benchmark set differs \
        from history.",
        short_commit(tip_commit)
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::path::PathBuf;

    use cbh_config::{Config, parse_config};
    use cbh_diag::RecordingReporter;
    use cbh_git::FakeGitHistory;
    use cbh_model::{
        BenchmarkId, BenchmarkIdPrefix, BenchmarkResult, BlessingRecord, EnvironmentInfo, GitInfo,
        Metric, MetricKind, Run, RunContext, ToolchainInfo, sanitize_segment,
    };
    use cbh_storage::{MemoryStorage, Storage};
    use futures::executor::block_on;
    use jiff::Timestamp;
    use nonempty::nonempty;

    use super::*;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    /// A minimal configuration; `analyze_with` only reads `project.default_branch`.
    fn config() -> Config {
        Config::default()
    }

    /// Builds a stored result set carrying one record with one `Ir` metric.
    fn ir_set(effective: i64, commit: &str, value: f64) -> Run {
        let time = ts(effective);
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

    /// The clean object key for `commit` in the callgrind/linux partition.
    fn clean_key(commit: &str) -> String {
        format!("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json")
    }

    /// The clean object key for `commit` in an arbitrary engine/triple/machine-key partition.
    fn clean_key_in(engine: &str, triple: &str, machine: &str, commit: &str) -> String {
        format!("v1/folo/objects/{engine}/{triple}/{machine}/{commit}/clean.json")
    }

    /// A stored result set whose single record carries two metrics (`Ir` and
    /// `ConditionalBranches`), so its partition reconstructs two distinct series.
    fn two_metric_set(effective: i64, commit: &str, ir: f64, branches: f64) -> Run {
        let time = ts(effective);
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

    /// A dirty snapshot key for `commit` taken at `unix`.
    fn dirty_key(commit: &str, unix: i64) -> String {
        format!(
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/dirty-{unix}.json"
        )
    }

    /// Stores a value at `key` in `storage`, panicking on failure (test helper).
    fn store(storage: &MemoryStorage, key: &str, set: &Run) {
        let json = set.to_json().unwrap();
        block_on(storage.put(key, json.as_bytes())).unwrap();
    }

    /// A linear master history `c0 - c1 - c2 - c3`, HEAD at the tip. Each commit
    /// carries committer time `ts(N)` for `cN`, matching the `effective`-second
    /// convention the seeders use, so the topology-decided `--since`
    /// cutoff can be exercised.
    fn linear_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit_at("c0", None, ts(0))
            .commit_at("c1", Some("c0"), ts(1))
            .commit_at("c2", Some("c1"), ts(2))
            .commit_at("c3", Some("c2"), ts(3))
            .branch("master", "c3")
            .head("master")
            .mark_default("master");
        git
    }

    /// A master history with a feature branch off `c1`:
    ///
    /// ```text
    /// master:  c0 - c1 - c2 - c3
    ///                \
    /// feature:        f1 - f2   (HEAD)
    /// ```
    fn feature_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("f1", Some("c1"))
            .commit("f2", Some("f1"))
            .branch("master", "c3")
            .branch("feature", "f2")
            .head("feature")
            .mark_default("master");
        git
    }

    /// A master history `c0 - c1 - c2 - c3` with a feature branch off the tip `c3`:
    ///
    /// ```text
    /// master:  c0 - c1 - c2 - c3
    ///                          \
    /// feature:                  f1 - f2   (HEAD)
    /// ```
    ///
    /// The four-commit base line gives a branch comparison a baseline large enough
    /// to reach rank-test significance against a raised feature regime; branching at
    /// the tip keeps every base commit an ancestor of the feature tip.
    fn feature_tip_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("f1", Some("c3"))
            .commit("f2", Some("f1"))
            .branch("master", "c3")
            .branch("feature", "f2")
            .head("feature")
            .mark_default("master");
        git
    }

    /// A linear master history `c0 - c1 - c2 - c3 - c4 - c5`, HEAD at the tip.
    ///
    /// Long enough to host a sustained level shift with at least two points on
    /// each side, which the change-point detector requires before it flags.
    fn linear6_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("c4", Some("c3"))
            .commit("c5", Some("c4"))
            .branch("master", "c5")
            .head("master")
            .mark_default("master");
        git
    }

    /// A six-commit master history `c0..c5` with a feature branch off `c1`,
    /// HEAD on the feature branch. The longer master line lets `--context master`
    /// reconstruct a sustained step.
    fn feature6_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("c4", Some("c3"))
            .commit("c5", Some("c4"))
            .commit("f1", Some("c1"))
            .commit("f2", Some("f1"))
            .branch("master", "c5")
            .branch("feature", "f2")
            .head("feature")
            .mark_default("master");
        git
    }

    /// Seeds a clean linear sustained-step history (`c0..c5` =
    /// 100,100,100,130,130,130) under the default partition, so the change-point
    /// detector flags a single major regression at `c3`.
    fn seed_linear_step(storage: &MemoryStorage) {
        for (index, value) in [100.0, 100.0, 100.0, 130.0, 130.0, 130.0]
            .into_iter()
            .enumerate()
        {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
    }

    fn options() -> AnalyzeOptions {
        AnalyzeOptions::default()
    }

    /// A fixed clock anchor for the history-mode default `--since` window in unit
    /// tests. The seeded data sits at the Unix epoch (`ts(0..)`); anchoring here
    /// keeps the default six-month look-back well before it, so the default window
    /// never drops a seeded point.
    fn now_anchor() -> Timestamp {
        Timestamp::from_second(0).unwrap()
    }

    /// The auto-detected facets for the default synthetic partition the unit-test
    /// data is seeded under (`x86_64-unknown-linux-gnu`, `synthetic` machine).
    fn auto() -> AutoFacets {
        AutoFacets {
            triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine_key: "synthetic".to_owned(),
        }
    }

    /// An inline spawner that runs the detection's blocking tasks on the calling
    /// thread, so `analyze_with` needs no Tokio runtime under `block_on` or Miri.
    fn spawner() -> Spawner {
        cbh_detect::testing::synchronous_spawner()
    }

    /// Runs `analyze_with` requesting the JSON report, returning the JSON text, the
    /// regression count, and the recording reporter so a test can assert on the
    /// machine-readable report and the verbose trail together. The text report is
    /// suppressed, so the JSON is the only rendered output.
    fn analyze_json(
        git: &FakeGitHistory,
        storage: &MemoryStorage,
        project: &str,
        options: &AnalyzeOptions,
    ) -> (String, usize, RecordingReporter) {
        let mut options = options.clone();
        options.no_text = true;
        options.markdown = None;
        options.json = Some(PathBuf::from("report.json"));
        let reporter = RecordingReporter::new();
        let (rendered, regressions) = block_on(analyze_with(
            git,
            storage,
            project,
            &config(),
            &options,
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();
        let report = rendered
            .json
            .expect("the JSON report was rendered for the requested path");
        (report, regressions, reporter)
    }

    #[test]
    fn should_colorize_only_in_an_interactive_terminal_without_no_color() {
        assert!(should_colorize(true, false), "terminal, NO_COLOR unset");
        assert!(!should_colorize(false, false), "not a terminal");
        assert!(!should_colorize(true, true), "NO_COLOR set");
        assert!(!should_colorize(false, true), "neither");
    }

    #[test]
    fn facet_filter_skips_an_unrecognized_storage_key() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        // A `.json` object under the project's objects prefix whose key is not a
        // valid eight-segment storage key is noted and skipped, not parsed as data.
        block_on(storage.put("v1/folo/objects/bogus.json", b"{}")).unwrap();
        let reporter = RecordingReporter::new();
        block_on(analyze_with(
            &linear6_git(),
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();
        assert!(
            reporter.contains("not a recognized"),
            "{:?}",
            reporter.notes()
        );
    }

    /// Runs `analyze_with` and unwraps the rendered text report and regression count.
    fn analyze(
        git: &FakeGitHistory,
        storage: &MemoryStorage,
        project: &str,
        options: &AnalyzeOptions,
    ) -> (String, usize) {
        let reporter = RecordingReporter::new();
        let (rendered, regressions) = block_on(analyze_with(
            git,
            storage,
            project,
            &config(),
            options,
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();
        (rendered.text.unwrap_or_default(), regressions)
    }

    #[test]
    fn analyze_without_a_repository_is_an_error() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let git = FakeGitHistory::new(); // No commits: HEAD does not resolve.
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, AnalyzeError::Analyze { .. }), "{error:?}");
        assert!(
            error.to_string().contains("requires a git repository"),
            "{error}"
        );
    }

    #[test]
    fn empty_history_reports_no_changes() {
        let storage = MemoryStorage::new();
        let git = linear_git();
        let (report, regressions) = analyze(&git, &storage, "folo", &options());
        assert_eq!(regressions, 0);
        assert!(report.contains("No notable changes detected."), "{report}");
    }

    #[test]
    fn official_view_detects_a_clean_regression_in_topology_order() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let git = linear6_git();
        let (report, regressions) = analyze(&git, &storage, "folo", &options());
        assert_eq!(regressions, 1);
        assert!(report.contains("regression"), "{report}");
        assert!(report.contains("nm/nm::observe/pull"), "{report}");
        assert!(report.contains("instruction_count"), "{report}");
    }

    #[test]
    fn json_notable_flag_reflects_whether_findings_survived() {
        // The `notable` signal appears only in the JSON report (the text report
        // keys off the finding list directly), so assert it there.
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let (report, regressions, _) = analyze_json(&linear6_git(), &storage, "folo", &options());
        assert_eq!(regressions, 1);
        assert!(report.contains("\"notable\": true"), "{report}");

        let empty = MemoryStorage::new();
        let (report, _, _) = analyze_json(&linear_git(), &empty, "folo", &options());
        assert!(report.contains("\"notable\": false"), "{report}");
    }

    #[test]
    fn select_dataset_notes_blessing_sidecars_in_the_partition() {
        // A blessing sidecar shares the run partition prefix; the verbose trail
        // calls it out only when at least one is present.
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let record = BlessingRecord::new(
            "c3".to_owned(),
            ts(3),
            vec![BenchmarkIdPrefix::new("nm").unwrap()],
            "0.0.1".to_owned(),
        );
        let bless_key =
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/c3/bless-3.json"
                .to_owned();
        block_on(storage.put(&bless_key, record.to_json().unwrap().as_bytes())).unwrap();

        let reporter = RecordingReporter::new();
        block_on(analyze_with(
            &linear6_git(),
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();
        assert!(
            reporter.contains("are blessing sidecars"),
            "{:?}",
            reporter.notes()
        );

        // No sidecar → the note is absent.
        let clean = MemoryStorage::new();
        seed_linear_step(&clean);
        let reporter = RecordingReporter::new();
        block_on(analyze_with(
            &linear6_git(),
            &clean,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();
        assert!(
            !reporter.contains("are blessing sidecars"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    fn analyze_records_a_timing_for_each_pipeline_stage() {
        // Every stage drawn in docs/analyze.md emits a timing on the dedicated
        // timing channel, so a `--verbose` run can localize a mystery slowdown.
        // History mode is used because it also exercises the blessing-load stage.
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let record = BlessingRecord::new(
            "c3".to_owned(),
            ts(3),
            vec![BenchmarkIdPrefix::new("nm").unwrap()],
            "0.0.1".to_owned(),
        );
        let bless_key =
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/c3/bless-3.json"
                .to_owned();
        block_on(storage.put(&bless_key, record.to_json().unwrap().as_bytes())).unwrap();

        let reporter = RecordingReporter::new();
        block_on(analyze_with(
            &linear6_git(),
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();

        for stage in [
            // analyze_with stages.
            "select_dataset",
            "re-baseline",
            "change detection",
            "report render",
            // select_dataset sub-stages.
            "candidate listing",
            "storage.list",
            "git topology",
            "git.first_parent",
            "phase 1",
            "phase 2/3",
            "series build finalization",
            // History-mode-only blessing load.
            "blessing sidecar load",
        ] {
            assert!(reporter.timed(stage), "missing timing for {stage:?}");
        }

        // Timings are a distinct channel: they never leak into the per-object note
        // stream a `--verbose` run also prints.
        assert!(!reporter.contains("timing:"), "{:?}", reporter.notes());
    }

    /// Drives history-mode analyze expecting the blessing load to fail.
    fn analyze_blessing_error(storage: &MemoryStorage) -> AnalyzeError {
        block_on(analyze_with(
            &linear6_git(),
            storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap_err()
    }

    #[test]
    fn history_mode_rejects_a_non_utf8_blessing_on_the_analyzed_history() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        // c3 is on the linear6 history, so history mode loads its sidecar.
        let bless_key =
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/c3/bless-3.json"
                .to_owned();
        block_on(storage.put(&bless_key, &[0xff, 0xfe, 0x00])).unwrap();
        let error = analyze_blessing_error(&storage);
        match error {
            AnalyzeError::Analyze { message } => {
                assert!(message.contains("is not valid UTF-8"), "{message}");
            }
            other => panic!("expected an analyze error, got {other:?}"),
        }
    }

    #[test]
    fn history_mode_rejects_an_invalid_blessing_on_the_analyzed_history() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let bless_key =
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/c3/bless-3.json"
                .to_owned();
        block_on(storage.put(&bless_key, b"{ not a blessing record")).unwrap();
        let error = analyze_blessing_error(&storage);
        match error {
            AnalyzeError::Analyze { message } => {
                assert!(
                    message.contains("is not a valid blessing record"),
                    "{message}"
                );
            }
            other => panic!("expected an analyze error, got {other:?}"),
        }
    }

    #[test]
    fn history_mode_skips_a_blessing_off_the_analyzed_history() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        // A blessing on a commit that is not on the analyzed history is noted and
        // skipped rather than applied.
        let record = BlessingRecord::new(
            "z9".to_owned(),
            ts(3),
            vec![BenchmarkIdPrefix::new("nm").unwrap()],
            "0.0.1".to_owned(),
        );
        let bless_key =
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/z9/bless-3.json"
                .to_owned();
        block_on(storage.put(&bless_key, record.to_json().unwrap().as_bytes())).unwrap();

        let reporter = RecordingReporter::new();
        block_on(analyze_with(
            &linear6_git(),
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();
        assert!(
            reporter.contains("is not on HEAD's analyzed history"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    fn per_set_report_counts_runs_and_series_independently() {
        let storage = MemoryStorage::new();
        // Both sets run through the `linear_git` tip (`c3`) so neither benchmark is a
        // ghost there and the always-on tip filter keeps every series.
        //
        // Set A — callgrind/linux/synthetic: three runs (c1..c3), each carrying two
        // metrics so the set reconstructs two distinct series.
        for index in 1..4 {
            let commit = format!("c{index}");
            let second = i64::from(index);
            store(
                &storage,
                &clean_key(&commit),
                &two_metric_set(second, &commit, 100.0, 200.0),
            );
        }
        // Set B — callgrind/darwin/synthetic: two runs (c2..c3), each carrying one
        // metric so the set reconstructs a single series. Distinct run AND series
        // counts from set A make an `==`/`!=` swap in either per-set tally observable.
        for index in 2..4 {
            let commit = format!("c{index}");
            let second = i64::from(index);
            store(
                &storage,
                &clean_key_in("callgrind", "aarch64-apple-darwin", "synthetic", &commit),
                &ir_set(second, &commit, 100.0),
            );
        }

        let git = linear_git();
        // The two sets live under different triples, and synthetic sets obey the
        // target-triple facet, so an auto-detected triple would report only its own.
        // Widen to `all` to exercise the per-set tallies across both partitions.
        let opts = AnalyzeOptions {
            target_triple: vec!["all".to_owned()],
            ..options()
        };
        let (report, _, _) = analyze_json(&git, &storage, "folo", &opts);

        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let sets = parsed["sets"].as_array().unwrap();

        let set_a = sets
            .iter()
            .find(|set| set["target_triple"] == "x86_64-unknown-linux-gnu")
            .unwrap();
        assert_eq!(set_a["runs"], 3, "{report}");
        assert_eq!(set_a["series"], 2, "{report}");

        let set_b = sets
            .iter()
            .find(|set| set["target_triple"] == "aarch64-apple-darwin")
            .unwrap();
        assert_eq!(set_b["runs"], 2, "{report}");
        assert_eq!(set_b["series"], 1, "{report}");
    }

    /// A stored result set naming several benchmarks, each carrying one `Ir` metric,
    /// so one commit's object can present or omit specific benchmarks — the shape a
    /// ghost (a benchmark that disappears before the tip) needs.
    fn multi_bench(effective: i64, commit: &str, benches: &[(&str, f64)]) -> Run {
        let time = ts(effective);
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
        let records = benches
            .iter()
            .map(|(name, value)| {
                BenchmarkResult::new(
                    BenchmarkId::new(nonempty![(*name).to_owned()]),
                    vec![Metric::new(MetricKind::InstructionCount, *value)],
                )
            })
            .collect::<Vec<_>>();
        Run::new(context, records)
    }

    #[test]
    fn a_ghost_benchmark_is_excluded() {
        // `kept` is measured through the tip (c0..c3); `ghost` disappears after c2.
        // The tip is c3, so `ghost` is no longer part of the current suite there.
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("c0"),
            &multi_bench(0, "c0", &[("kept", 100.0), ("ghost", 100.0)]),
        );
        store(
            &storage,
            &clean_key("c1"),
            &multi_bench(1, "c1", &[("kept", 100.0), ("ghost", 100.0)]),
        );
        store(
            &storage,
            &clean_key("c2"),
            &multi_bench(2, "c2", &[("kept", 100.0), ("ghost", 100.0)]),
        );
        store(
            &storage,
            &clean_key("c3"),
            &multi_bench(3, "c3", &[("kept", 100.0)]),
        );
        let git = linear_git();

        // The ghost is filtered out before detection, and the verbose trail names it
        // and the context commit it is absent from.
        let (report, _, reporter) = analyze_json(&git, &storage, "folo", &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["ghosts_excluded"], 1, "{report}");
        assert_eq!(parsed["series"], 1, "only `kept` survives, {report}");
        assert!(
            reporter.contains("excluding benchmark ghost"),
            "{:?}",
            reporter.notes()
        );
        assert!(
            reporter.contains("not present at the context commit c3"),
            "{:?}",
            reporter.notes()
        );
    }

    #[test]
    fn an_all_ghosts_analysis_emits_the_dedicated_hint() {
        // Every benchmark disappears before the tip (data stops at c2, tip is c3), so
        // the filter empties the analysis. The empty outcome must read as an
        // all-ghosts case, distinct from a bare "no data".
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("c2"), &ir_set(2, "c2", 100.0));
        let git = linear_git();

        let (report, _, _) = analyze_json(&git, &storage, "folo", &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["ghosts_excluded"], 1, "{report}");
        assert_eq!(parsed["series"], 0, "{report}");
        assert_eq!(parsed["runs"], 3, "the runs still loaded, {report}");
        let hint = parsed["hint"].as_str().unwrap_or_default();
        assert!(hint.contains("filtered as a ghost"), "{report}");
        assert!(
            hint.contains("context commit"),
            "the hint names the context commit: {report}"
        );
    }

    #[test]
    fn series_order_follows_topology_not_observation_time() {
        // Topology is c0..c5 with a sustained step at c3 (100,100,100,130,130,130),
        // but the objects' observation clock is reversed (c0 newest, c5 oldest).
        // Ordering by topology reconstructs the rising step and flags a regression;
        // were the provenance-only observation time ever allowed to order the series
        // it would reverse into a falling step (an improvement, no regression). So a
        // single detected regression proves topology won.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0, 130.0, 130.0, 130.0]
            .into_iter()
            .enumerate()
        {
            let commit = format!("c{index}");
            // Reverse the clock: c0 has the newest observation time, c5 the oldest.
            let second = 100 - i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        let git = linear6_git();
        let (_, regressions) = analyze(&git, &storage, "folo", &options());
        assert_eq!(regressions, 1, "the step must be read in topology order");
    }

    #[test]
    fn official_view_excludes_dirty_runs() {
        // A dirty snapshot on the master tip must not enter the official timeline.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        // A wildly different dirty value on the tip: if admitted it would flag.
        store(&storage, &dirty_key("c3", 500), &ir_set(500, "c3", 999.0));
        let git = linear_git();

        let (report, regressions, _) = analyze_json(&git, &storage, "folo", &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "the dirty tip run is excluded");
        assert_eq!(regressions, 0);
    }

    #[test]
    fn feature_view_admits_dirty_after_the_merge_base() {
        // feature branched at the master tip c3; the target side rises at f1 and a
        // dirty f2 snapshot sustains the new level. The dirty run is admitted
        // (runs == 7) and is essential to the flag: no engine is exact, so the raised
        // regime must clear a rank test against the baseline. With only the two clean
        // feature points the 4-vs-2 split is underpowered; the dirty f2 snapshot tips
        // it to a significant 4-vs-3 difference.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("c2"), &ir_set(2, "c2", 100.0));
        store(&storage, &clean_key("c3"), &ir_set(3, "c3", 100.0));
        store(&storage, &clean_key("f1"), &ir_set(4, "f1", 130.0));
        store(&storage, &clean_key("f2"), &ir_set(5, "f2", 130.0));
        store(&storage, &dirty_key("f2", 6), &ir_set(6, "f2", 130.0));
        let git = feature_tip_git();

        let (report, regressions, _) = analyze_json(&git, &storage, "folo", &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 7, "the dirty f2 snapshot is admitted");
        assert_eq!(regressions, 1, "the admitted dirty f2 completes the step");
    }

    #[test]
    fn no_dirty_suppresses_the_target_side_dirty_run() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 100.0));
        store(&storage, &dirty_key("f2", 3), &ir_set(3, "f2", 130.0));
        let git = feature_git();

        let opts = AnalyzeOptions {
            no_dirty: true,
            ..options()
        };
        let (report, _, _) = analyze_json(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "--no-dirty drops the dirty snapshot");
    }

    #[test]
    fn dirty_run_on_a_base_side_commit_is_excluded() {
        // A dirty snapshot on c1 (at/before the merge-base) is base-side, so even
        // on the feature view it is clean-only and the dirty file is excluded.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &dirty_key("c1", 9), &ir_set(9, "c1", 999.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 100.0));
        let git = feature_git();

        let (report, _, _) = analyze_json(&git, &storage, "folo", &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "the base-side dirty c1 run is excluded");
    }

    #[test]
    fn all_dirty_on_base_yields_zero_runs_with_a_hint() {
        // The user-reported trap: on the default branch's tip every run is a
        // dirty snapshot (e.g. because the config file was never committed), so
        // all are excluded and the empty outcome must explain itself with a hint
        // and per-object verbose notes rather than looking like "no data".
        let storage = MemoryStorage::new();
        store(&storage, &dirty_key("c3", 100), &ir_set(100, "c3", 100.0));
        store(&storage, &dirty_key("c3", 200), &ir_set(200, "c3", 130.0));
        let git = linear_git();

        let (report, _, reporter) = analyze_json(&git, &storage, "folo", &options());

        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            parsed["runs"], 0,
            "every dirty-on-base snapshot is excluded"
        );
        let hint = parsed["hint"].as_str().unwrap();
        assert!(
            hint.contains("Found 2 stored runs"),
            "the hint should count the stored runs: {hint}"
        );
        assert!(
            hint.contains("dirty"),
            "the hint should explain the dirty-on-base exclusion: {hint}"
        );

        assert!(
            reporter.contains("dirty snapshot on a base-side commit"),
            "verbose notes should explain each exclusion: {:?}",
            reporter.notes()
        );
    }

    #[test]
    fn dirty_tree_on_base_branch_admits_tip_dirty_runs_with_a_warning() {
        // On the base branch (official view) with a currently-dirty working tree,
        // the dirty snapshots on the tip are the user's in-flight work and ARE
        // admitted, with a warning that they are ephemeral. Three snapshots at the
        // raised level form a regime large enough to clear the rank test against the
        // clean baseline.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        store(&storage, &dirty_key("c3", 300), &ir_set(300, "c3", 130.0));
        store(&storage, &dirty_key("c3", 400), &ir_set(400, "c3", 130.0));
        store(&storage, &dirty_key("c3", 500), &ir_set(500, "c3", 130.0));
        let mut git = linear_git();
        git.mark_dirty();

        let (report, regressions, reporter) = analyze_json(&git, &storage, "folo", &options());

        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            parsed["runs"], 6,
            "all three dirty tip snapshots are admitted"
        );
        assert_eq!(regressions, 1, "the dirty tip snapshots complete the step");
        assert_eq!(
            parsed["tip_commit"], "c3",
            "the report names the analyzed tip"
        );
        assert_eq!(
            parsed["tip_dirty"], true,
            "the currently-dirty working tree annotates the tip"
        );
        let warning = parsed["warning"].as_str().unwrap();
        assert!(
            warning.contains("dirty runs") && warning.contains("Switch to a new branch"),
            "{warning}"
        );
        assert!(
            reporter.contains("ephemeral"),
            "a verbose note should flag the ephemeral inclusion: {:?}",
            reporter.notes()
        );
    }

    #[test]
    fn clean_tree_on_base_branch_excludes_dirty_and_warns_nothing() {
        // The exception is gated on the working tree being dirty: with a clean
        // tree the base-tip dirty snapshot stays excluded and no warning fires.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        store(&storage, &dirty_key("c3", 300), &ir_set(300, "c3", 999.0));
        let git = linear_git(); // Clean working tree (the default).

        let (report, _, _) = analyze_json(&git, &storage, "folo", &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "the dirty tip run stays excluded");
        assert_eq!(
            parsed["tip_commit"], "c3",
            "the report names the analyzed tip"
        );
        assert_eq!(
            parsed["tip_dirty"], false,
            "a clean working tree leaves the tip unannotated"
        );
        assert!(
            parsed["warning"].is_null(),
            "no warning when the tree is clean"
        );
    }

    #[test]
    fn dirty_working_tree_without_recorded_dirty_runs_stays_history_mode() {
        // The reported corner case: on the base branch with a currently-dirty
        // working tree but ONLY clean runs recorded (no dirty run on the tip), mode
        // auto-detection keys off the *admitted* runs — a dirty tree with no admitted
        // dirty run carries no branch evidence — and picks history mode, so the
        // long-range change-point detector still flags the sustained step. The old
        // behaviour keyed off `git.is_dirty()` alone and wrongly fell into branch
        // mode here.
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let mut git = linear6_git();
        git.mark_dirty();

        let (report, regressions, reporter) = analyze_json(&git, &storage, "folo", &options());

        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            parsed["mode"], "history",
            "a dirty tree with only clean runs is still the official history view"
        );
        assert_eq!(
            regressions, 1,
            "history mode flags the sustained clean step at c3"
        );
        assert!(
            parsed["warning"].is_null(),
            "no dirty runs are admitted, so nothing is ephemeral"
        );
        assert!(
            reporter.contains("no dirty run is")
                && reporter.contains("admitted only while the working tree is currently dirty"),
            "the verbose note should explain why history mode was chosen: {:?}",
            reporter.notes()
        );
    }

    #[test]
    fn no_dirty_overrides_the_dirty_tree_exception() {
        // `--no-dirty` skips the dirtiness probe and the exception, so even with a
        // dirty tree the base-tip dirty snapshot is excluded and no warning fires.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        store(&storage, &dirty_key("c3", 300), &ir_set(300, "c3", 999.0));
        let mut git = linear_git();
        git.mark_dirty();

        let opts = AnalyzeOptions {
            no_dirty: true,
            ..options()
        };
        let (report, _, reporter) = analyze_json(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "--no-dirty drops the dirty tip snapshot");
        assert_eq!(
            parsed["tip_dirty"], false,
            "--no-dirty skips the dirtiness probe, so the tip is never annotated dirty"
        );
        assert!(parsed["warning"].is_null(), "no warning under --no-dirty");
        assert!(
            !reporter.contains("dirty snapshots on a base-side tip will be admitted"),
            "--no-dirty skips the dirtiness probe, so the dirty-tree exception never fires: {:?}",
            reporter.notes()
        );
    }

    #[test]
    fn only_the_tip_admits_dirty_under_the_exception() {
        // With a dirty tree the exception applies ONLY to the base-branch tip: a
        // dirty snapshot on an earlier base-side commit stays excluded while the
        // tip's dirty snapshot is admitted (and warned).
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0, 100.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        store(&storage, &dirty_key("c1", 150), &ir_set(150, "c1", 999.0));
        store(&storage, &dirty_key("c3", 300), &ir_set(300, "c3", 130.0));
        let mut git = linear_git();
        git.mark_dirty();

        let (report, _, reporter) = analyze_json(&git, &storage, "folo", &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            parsed["runs"], 5,
            "only the tip's dirty run joins the four clean runs"
        );
        assert!(
            !parsed["warning"].is_null(),
            "the tip's admitted dirty run warns: {report}"
        );
        assert!(
            reporter.contains("dirty snapshot on a base-side commit"),
            "the earlier base-side dirty run is still excluded: {:?}",
            reporter.notes()
        );
    }

    #[test]
    fn commits_off_the_first_parent_chain_are_excluded() {
        // c2 and c3 are on master but not on feature's first-parent ancestry, so
        // their runs never enter a feature-view analysis.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("c2"), &ir_set(2, "c2", 999.0));
        store(&storage, &clean_key("c3"), &ir_set(3, "c3", 999.0));
        store(&storage, &clean_key("f1"), &ir_set(4, "f1", 100.0));
        let git = feature_git();

        let (report, _, _) = analyze_json(&git, &storage, "folo", &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "c2 and c3 are off the feature mainline");
    }

    #[test]
    fn explicit_branch_selects_the_official_master_view() {
        // From a feature checkout, `--context master` analyzes master's own history:
        // six clean commits with a sustained step at c3.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0, 130.0, 130.0, 130.0]
            .into_iter()
            .enumerate()
        {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        let git = feature6_git();

        let opts = AnalyzeOptions {
            context: Some("master".to_owned()),
            ..options()
        };
        let (report, regressions, _) = analyze_json(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 6, "master's six commits");
        assert_eq!(regressions, 1);
    }

    #[test]
    fn within_a_commit_clean_precedes_dirty() {
        // On a target-side commit, a clean run and dirty snapshots both load; the
        // clean run is the baseline and the later dirty values are the latest
        // points. Three dirty snapshots at the raised level form a regime large
        // enough to clear the rank test against the four-commit clean baseline.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("c2"), &ir_set(2, "c2", 100.0));
        store(&storage, &clean_key("c3"), &ir_set(3, "c3", 100.0));
        store(&storage, &clean_key("f1"), &ir_set(4, "f1", 100.0));
        store(&storage, &clean_key("f2"), &ir_set(5, "f2", 100.0));
        store(&storage, &dirty_key("f2", 6), &ir_set(6, "f2", 130.0));
        store(&storage, &dirty_key("f2", 7), &ir_set(7, "f2", 130.0));
        store(&storage, &dirty_key("f2", 8), &ir_set(8, "f2", 130.0));
        let git = feature_tip_git();

        let (_, regressions, _) = analyze_json(&git, &storage, "folo", &options());
        assert_eq!(regressions, 1, "the dirty f2 values are the latest points");
    }

    #[test]
    fn target_triple_facet_selects_the_windows_set() {
        // Two sets differing only by triple; an explicit `--target-triple` reports
        // just the matching one, even though the auto-detected default is Linux.
        // Both are seeded at the `linear_git` tip (`c3`) so the tip filter keeps them.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c3"), &ir_set(3, "c3", 100.0));
        store(
            &storage,
            "v1/folo/objects/callgrind/x86_64-pc-windows-msvc/synthetic/c3/clean.json",
            &ir_set(3, "c3", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            target_triple: vec!["x86_64-pc-windows-msvc".to_owned()],
            ..options()
        };
        let (report, _, _) = analyze_json(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 1, "only the windows set is loaded");
        assert_eq!(parsed["sets"].as_array().unwrap().len(), 1, "{report}");
        assert_eq!(
            parsed["sets"][0]["target_triple"], "x86_64-pc-windows-msvc",
            "{report}"
        );
    }

    #[test]
    fn target_triple_facet_selects_one_set() {
        // Two sets differing only by triple; `--target-triple` reports just the one.
        // Both are seeded at the `linear_git` tip (`c3`) so the tip filter keeps them.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c3"), &ir_set(3, "c3", 100.0));
        store(
            &storage,
            "v1/folo/objects/callgrind/x86_64-pc-windows-msvc/synthetic/c3/clean.json",
            &ir_set(3, "c3", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            target_triple: vec!["x86_64-unknown-linux-gnu".to_owned()],
            ..options()
        };
        let (report, _, _) = analyze_json(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 1, "only the linux-gnu triple is loaded");
        assert_eq!(parsed["sets"].as_array().unwrap().len(), 1, "{report}");
        assert_eq!(
            parsed["sets"][0]["target_triple"], "x86_64-unknown-linux-gnu",
            "{report}"
        );
    }

    #[test]
    fn two_sets_produce_two_report_sections() {
        // Both sets are seeded at the `linear_git` tip (`c3`) so the tip filter keeps
        // each and every partition is reported. They differ only by triple, and
        // synthetic sets obey the target-triple facet, so the query widens to
        // `--target-triple all` to search both partitions rather than just the host's.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c3"), &ir_set(3, "c3", 100.0));
        store(
            &storage,
            "v1/folo/objects/callgrind/x86_64-pc-windows-msvc/synthetic/c3/clean.json",
            &ir_set(3, "c3", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            target_triple: vec!["all".to_owned()],
            ..options()
        };
        let (report, _, _) = analyze_json(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["sets"].as_array().unwrap().len(), 2, "{report}");
    }

    #[test]
    fn engine_facet_narrows_the_listing() {
        // Two sets in the same triple/machine-key partition differing only by engine,
        // so the engine facet alone selects one.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v1/folo/objects/criterion/x86_64-unknown-linux-gnu/synthetic/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            engine: vec!["callgrind".to_owned()],
            ..options()
        };
        let (report, _, _) = analyze_json(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 1, "only the callgrind object is loaded");
    }

    #[test]
    fn since_window_excludes_earlier_runs() {
        let storage = MemoryStorage::new();
        // c0,c1 at epoch seconds 0,1; c2,c3 at 2,3. `--since` epoch 2 keeps c2,c3.
        for (index, value) in [100.0, 100.0, 100.0, 130.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        let git = linear_git();

        let opts = AnalyzeOptions {
            since: Some("1970-01-01T00:00:02Z".to_owned()),
            ..options()
        };
        let (report, _, _) = analyze_json(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 2, "only c2 and c3 are within the window");
    }

    #[test]
    fn analyze_without_a_resolvable_base_branch_is_an_error() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        // HEAD resolves, but there is no advertised default branch and no --base /
        // config default, so the base branch cannot be determined and there is no
        // merge-base to split the timeline on. Rather than silently analyze the
        // incomplete topology as a base-branch (history) view, this is an error
        // that tells the user how to supply the missing history.
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("c4", Some("c3"))
            .commit("c5", Some("c4"))
            .branch("master", "c5")
            .head("master"); // No `.mark_default(...)`.
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, AnalyzeError::Analyze { .. }), "{error:?}");
        let message = error.to_string();
        assert!(
            message.contains("could not determine the base branch"),
            "{message}"
        );
        assert!(message.contains("--base"), "{message}");
    }

    #[test]
    fn analyze_without_a_common_ancestor_is_an_error() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        // The base branch resolves, but it shares no history with the target — the
        // shallow-clone case, where the fetched depth stops short of the branch
        // point. `git merge-base` finds no common ancestor, so the timeline cannot
        // be split; this errors and leads with the deepen-the-clone fix rather than
        // guessing a base-branch view. The base was auto-detected here, so the
        // message also offers --base as the way to name a different one.
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("c4", Some("c3"))
            .commit("c5", Some("c4"))
            // A disjoint base history with no common ancestor with the target.
            .commit("m0", None)
            .branch("master", "m0")
            .branch("feature", "c5")
            .head("feature")
            .mark_default("master");
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, AnalyzeError::Analyze { .. }), "{error:?}");
        let message = error.to_string();
        assert!(message.contains("no common ancestor"), "{message}");
        assert!(message.contains("--unshallow"), "{message}");
        // Auto-detected base: offer --base as the way to name the intended one.
        assert!(message.contains("--base"), "{message}");
    }

    #[test]
    fn analyze_with_an_explicit_disjoint_base_does_not_suggest_a_different_base() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        // The user deliberately chose `--base master`, which resolves but shares no
        // history with the target. The remedy is still to deepen the clone; we must
        // not glibly tell them to pass some other --base, since they picked this one
        // on purpose. Only if the history is complete is the chosen base called out
        // as genuinely unrelated.
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("c4", Some("c3"))
            .commit("c5", Some("c4"))
            .commit("m0", None)
            .branch("master", "m0")
            .branch("feature", "c5")
            .head("feature")
            .mark_default("master");
        let mut options = options();
        options.base = Some("master".to_owned());
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options,
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, AnalyzeError::Analyze { .. }), "{error:?}");
        let message = error.to_string();
        assert!(message.contains("--unshallow"), "{message}");
        // The deliberately chosen base is named and reported as unrelated, without
        // suggesting the user pick a different --base value.
        assert!(
            message.contains("master is genuinely unrelated"),
            "{message}"
        );
        assert!(!message.contains("name the intended base"), "{message}");
    }

    #[test]
    fn history_is_found_for_a_project_id_that_requires_sanitizing() {
        // `collect` stores under the sanitized project segment, so `analyze` must list
        // under that same segment; listing under the raw id would miss the history.
        let storage = MemoryStorage::new();
        let raw_project = "my project/v2";
        let sanitized = sanitize_segment(raw_project);
        for (index, value) in [100.0, 100.0, 100.0, 130.0, 130.0, 130.0]
            .into_iter()
            .enumerate()
        {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            let key = format!(
                "v1/{sanitized}/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json"
            );
            store(&storage, &key, &ir_set(second, &commit, value));
        }
        let git = linear6_git();

        let (report, regressions) = analyze(&git, &storage, raw_project, &options());
        assert_eq!(
            regressions, 1,
            "history stored under the sanitized key must be found"
        );
        assert!(report.contains("nm/nm::observe/pull"), "{report}");
    }

    #[test]
    fn a_flagged_regression_still_yields_a_successful_analysis() {
        // The exit code no longer depends on findings: even a flagged regression
        // yields a successful (Ok) analysis (the signal lives in the report JSON).
        // The shell maps this into an always-successful `RunOutcome`.
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let git = linear6_git();

        let (_, regressions) = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .expect("a flagged regression must not fail the analysis");
        assert_eq!(regressions, 1, "the seeded step is a flagged regression");
    }

    #[test]
    fn json_format_is_rendered() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 10.0));
        let git = linear_git();

        let (report, _, _) = analyze_json(&git, &storage, "folo", &options());
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["project"], "folo");
        assert_eq!(parsed["runs"], 1);
    }

    #[test]
    fn non_json_objects_are_skipped() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 10.0));
        // A stray non-result object under the prefix must be ignored, not parsed.
        block_on(storage.put("v1/folo/objects/callgrind/README.txt", b"not json")).unwrap();
        let git = linear_git();

        let (_, regressions) = analyze(&git, &storage, "folo", &options());
        assert_eq!(regressions, 0);
    }

    #[test]
    fn malformed_stored_object_is_an_analyze_error() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c0"), b"{ not valid")).unwrap();
        let git = linear_git();

        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, AnalyzeError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn invalid_utf8_object_is_an_analyze_error() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c0"), &[0xff, 0xfe])).unwrap();
        let git = linear_git();

        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, AnalyzeError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn no_output_selected_is_rejected() {
        // Suppressing the text report without requesting any file output leaves
        // nothing to produce, which is a usage error rather than a silent no-op.
        let storage = MemoryStorage::new();
        let git = linear_git();
        let opts = AnalyzeOptions {
            no_text: true,
            ..options()
        };
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, AnalyzeError::Analyze { .. }), "{error:?}");
        assert!(error.to_string().contains("no output selected"), "{error}");
    }

    #[test]
    fn unknown_engine_is_rejected() {
        let storage = MemoryStorage::new();
        let git = linear_git();
        let opts = AnalyzeOptions {
            engine: vec!["dhat".to_owned()],
            ..options()
        };
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, AnalyzeError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn unresolvable_base_is_rejected() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let git = linear_git();
        let opts = AnalyzeOptions {
            base: Some("does-not-exist".to_owned()),
            ..options()
        };
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, AnalyzeError::Analyze { .. }), "{error:?}");
        assert!(error.to_string().contains("--base"), "{error}");
    }

    #[test]
    fn configured_default_branch_is_used_as_the_base() {
        // The config names `master` as the default branch; analyzing the feature
        // branch must split at the master merge-base even without `--base`.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &dirty_key("c1", 9), &ir_set(9, "c1", 999.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 100.0));
        // A git history that does NOT advertise a default branch, so resolution
        // must fall through to the configured `project.default_branch`.
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("f1", Some("c1"))
            .branch("master", "c1")
            .branch("feature", "f1")
            .head("feature");
        let config = parse_config("[project]\ndefault_branch = \"master\"\n").unwrap();

        let opts = AnalyzeOptions {
            no_text: true,
            json: Some(PathBuf::from("report.json")),
            ..options()
        };
        let (rendered, _) = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config,
            &opts,
            &auto(),
            now_anchor(),
            &RecordingReporter::new(),
            false,
            &spawner(),
        ))
        .unwrap();
        let report = rendered.json.expect("the JSON report was rendered");
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        // c1's dirty run is base-side (excluded); c0, c1 clean and f1 clean load.
        assert_eq!(
            parsed["runs"], 3,
            "base-side dirty c1 excluded via config base"
        );
    }

    #[test]
    fn resolve_now_reads_the_injected_clock() {
        // The analyze family sources its wall-clock anchor through an injectable
        // `tick::Clock`; a frozen clock must surface its own instant verbatim rather than
        // any default minted independently of the clock.
        let anchor = ts(1_700_000_000);
        assert_eq!(resolve_now(Some(Clock::new_frozen_at(anchor))), anchor);
    }
}
