//! The `analyze` command: resolve which stored runs make up each benchmark's
//! history from live git topology, reconstruct the series, and report the notable
//! changes.
//!
//! Unlike a snapshot tool, `analyze` orders a series by *git history* rather than
//! by ingest time (see the `analyze` command in `DESIGN.md`): it resolves the
//! target ref's first-parent
//! ancestry, splits it at the merge-base with a base branch, and admits dirty
//! (uncommitted-tree) snapshots only on the target side of that split. The pure
//! logic (selection, series reconstruction, finding detection, report rendering)
//! stays sync and Miri-safe; only the git queries and object loads touch async
//! ports. [`execute`] wires the real adapters; [`analyze_with`] is the
//! storage- and git-generic orchestrator the in-memory tests drive.

pub(crate) mod bless;
pub(crate) mod list;
pub(crate) mod prune;

pub(crate) use cargo_bench_history_core::analyze::{
    AnalysisConfig, AnalysisContext, AnalysisMode, BlessingPlacement, DiscriminantSetQuery,
    FacetFilter, ReportFormat, ReportInput, Series, SeriesBuilder, SeriesFilter, SetSummary,
    StorageKey, apply_blessings, find_changes_spawned, parse_key, render, select_commits,
};

use std::collections::{BTreeMap, HashMap};
use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyspawn::Spawner;
use futures::{StreamExt as _, TryStreamExt as _};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use nonempty::NonEmpty;

use crate::config::{Config, load_config};
use crate::git_history::{GitHistory, SystemGitHistory};
use crate::machine::resolve_machine_key;
use crate::model::BlessingRecord;
use crate::model::Run;
use crate::model::{DiscriminantSet, Engine, STORAGE_VERSION, sanitize_segment};
use crate::probe::{EnvironmentProbe, SystemProbe};
use crate::report::{Reporter, ReporterExt, StderrReporter};
use crate::storage::{Storage, build_storage};
use crate::text::count_noun;
use crate::wiring::{
    resolve_config_path, resolve_local_path, resolve_project_id, resolve_repo, storage_env,
};
use crate::{
    AnalyzeOptions, BlessOptions, ListOptions, PruneOptions, RunError, RunOutcome, UnblessOptions,
};

/// The real `analyze`: load configuration, wire the configured storage and git
/// history, and orchestrate.
///
/// `now_override` anchors the history-mode default `--since` lookback to a fixed
/// instant; production passes `None` so the anchor is the wall clock, while tests
/// pass a deterministic instant (the relative-`--since` parsing keeps using the
/// real clock regardless — only the *default* lookback uses this anchor).
pub(crate) async fn execute(
    options: &AnalyzeOptions,
    workspace_dir: &Path,
    now_override: Option<Timestamp>,
) -> Result<RunOutcome, RunError> {
    // Per-object notes follow `--verbose`; stage timings are emitted under either
    // `--verbose` or the programmatic `timing` flag (the stress harness sets the
    // latter alone to see the load breakdown without the per-object flood).
    let reporter = StderrReporter::with_timing(options.verbose, options.stage_timings_enabled());

    let config_path = resolve_config_path(workspace_dir, options.config_path.as_deref());
    reporter.note(&format!(
        "loading configuration from {}",
        config_path.display()
    ));
    let config = load_config(&config_path, options.config_path.is_some()).await?;

    let project_id = resolve_project_id(&config, workspace_dir);
    let local = resolve_local_path(options.local.as_ref(), storage_env().as_deref())?;
    let storage = build_storage(local.as_deref(), &config, workspace_dir)?;

    let git = SystemGitHistory::new(resolve_repo(workspace_dir, options.repo.as_deref()));
    let auto = detect_auto_facets().await?;

    let now = now_override.unwrap_or_else(Timestamp::now);
    let color = should_colorize(
        std::io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
    );
    // Distribute the compute-bound detection across the runtime's blocking pool, so
    // the analysis shares the ambient Tokio worker threads rather than spawning its
    // own short-lived ones.
    let spawner = Spawner::new_tokio();
    analyze_with(
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
    .await
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
async fn detect_auto_facets() -> Result<AutoFacets, RunError> {
    let probe = SystemProbe::default();
    let toolchain = probe.toolchain().await.map_err(RunError::Io)?;
    let hardware = probe.hardware().await;
    Ok(AutoFacets {
        triple: toolchain.host.unwrap_or_default(),
        machine_key: resolve_machine_key(None, &hardware),
    })
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
) -> Result<RunOutcome, RunError>
where
    G: GitHistory,
    S: Storage,
{
    let format = parse_format(options.format.as_deref())?;
    let selection = Selection::from_analyze(options)?;
    let filter = SeriesFilter {
        prefixes: &options.prefixes,
    };
    let load_started = Instant::now();
    let dataset = select_dataset(
        git, storage, project_id, config, &selection, filter, auto, now, reporter,
    )
    .await?;
    reporter.timing(
        "select_dataset (full load: list + filter + topology + fetch/parse/fold + build)",
        load_started.elapsed(),
    );

    let mut series = dataset.series;
    // Re-baseline blessed series before detection (history mode only; branch and
    // tip modes carry an empty blessing map).
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
    let hint = empty_history_hint(
        dataset.run_index.is_empty(),
        dataset.candidate_count,
        &dataset.target_ref,
        dataset.tally,
    );

    // Admitting a dirty snapshot on the base branch's tip is a courtesy for the
    // "evaluating the tool" / "accidentally working on the base branch" cases; warn
    // that such data is not persisted across commits.
    let warning = dataset
        .included_dirty_base_exception
        .then(dirty_base_exception_warning);

    let input = ReportInput {
        project: project_id,
        mode: dataset.mode.as_str(),
        notable,
        runs: dataset.run_index.total(),
        series: series.len(),
        findings: &findings,
        sets: &summaries,
        hint: hint.as_deref(),
        warning: warning.as_deref(),
    };
    let render_started = Instant::now();
    let report = render(&input, format, color);
    reporter.timing("report render", render_started.elapsed());

    Ok(RunOutcome::Analyzed {
        report,
        regressions,
    })
}

/// The data-set selection parameters shared by the query commands: which stored
/// objects to consider (facets + `--since` / `--until`) and how to resolve the git
/// timeline (`--repo` is resolved by the caller into the [`GitHistory`] adapter;
/// `--context` / `--base` / `--no-dirty` steer the topology query). Analyze's
/// benchmark-prefix scope is deliberately *not* here: it filters which series are
/// built, not which runs load.
///
/// Each facet (`engine` / `target_triple` / `machine_key`) carries the raw,
/// repeatable command-line values; [`resolve_facets`] turns them into
/// [`FacetFilter`]s, applying the current-machine auto-detect default and the
/// `all` keyword.
struct Selection<'a> {
    context: Option<&'a str>,
    base: Option<&'a str>,
    no_dirty: bool,
    since: Option<&'a str>,
    until: Option<&'a str>,
    engine: &'a [String],
    target_triple: &'a [String],
    machine_key: &'a [String],
    /// Explicit `--mode` override; `None` lets the mode auto-detect from topology.
    mode_override: Option<AnalysisMode>,
}

impl<'a> Selection<'a> {
    fn from_analyze(options: &'a AnalyzeOptions) -> Result<Self, RunError> {
        Ok(Self {
            context: options.context.as_deref(),
            base: options.base.as_deref(),
            no_dirty: options.no_dirty,
            since: options.since.as_deref(),
            until: options.until.as_deref(),
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
            mode_override: parse_mode(options.mode.as_deref())?,
        })
    }

    fn from_list(options: &'a ListOptions) -> Self {
        Self {
            context: options.context.as_deref(),
            base: options.base.as_deref(),
            no_dirty: options.no_dirty,
            since: options.since.as_deref(),
            until: options.until.as_deref(),
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
            mode_override: None,
        }
    }

    fn from_prune(options: &'a PruneOptions) -> Self {
        Self {
            context: options.context.as_deref(),
            base: options.base.as_deref(),
            // `prune` resolves the data set with dirty admission always on; the
            // base-tip exception is applied unconditionally (see
            // `DirtyTipPolicy::Always`), and the per-object scope (`--dirty` /
            // `--clean`) decides which runs are actually removed.
            no_dirty: false,
            since: options.since.as_deref(),
            until: options.until.as_deref(),
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
            mode_override: None,
        }
    }

    /// Selection facets for `bless`. Only the discriminant facets (and `base`)
    /// matter: a blessing always acts at the current commit, so it has no
    /// `context` / `since` / topology selectors.
    fn from_bless(options: &'a BlessOptions) -> Self {
        Self {
            context: None,
            base: options.base.as_deref(),
            no_dirty: false,
            since: None,
            until: None,
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
            mode_override: None,
        }
    }

    /// Selection facets for `unbless`. Mirrors [`from_bless`](Self::from_bless).
    fn from_unbless(options: &'a UnblessOptions) -> Self {
        Self {
            context: None,
            base: options.base.as_deref(),
            no_dirty: false,
            since: None,
            until: None,
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
            mode_override: None,
        }
    }
}

/// The current machine's auto-detected facet values, used as the default when a
/// query facet is omitted (see the *Discriminant set & query facets* section of
/// `DESIGN.md`).
///
/// Production probes these once (host triple from `rustc -vV`, machine key from
/// the hardware fingerprint); tests pass deterministic literals. There is no auto
/// engine — a bare query analyzes every engine — so only the triple and machine
/// key are detected.
#[derive(Clone, Debug)]
pub(crate) struct AutoFacets {
    /// The host target triple (`rustc -vV` host).
    pub(crate) triple: String,
    /// The host machine fingerprint.
    pub(crate) machine_key: String,
}

/// One commit's run tally within a discriminant set, the granularity the report
/// summaries and the `list runs` breakdown need.
#[derive(Clone, Debug)]
pub(crate) struct CommitCounts {
    /// The commit the runs were measured against (full SHA, or a label in tests).
    pub(crate) commit: String,
    /// Clean (committed-tree) runs recorded on the commit.
    pub(crate) clean: usize,
    /// Dirty (uncommitted-tree) snapshots recorded on the commit.
    pub(crate) dirty: usize,
}

/// Compact per-set, per-commit run tallies kept *in place of* a retained copy of
/// every loaded object.
///
/// A long history holds tens of thousands of run objects, each carrying every
/// benchmark; keeping them all resident alongside the reconstructed series is what
/// drove analysis into tens of gigabytes. The analysis only needs the series plus
/// these aggregate counts (total runs, per-set runs, and the per-commit breakdown
/// the listing renders), so each parsed run is folded into the series and dropped,
/// updating this index as it goes.
#[derive(Clone, Debug, Default)]
pub(crate) struct RunIndex {
    total: usize,
    sets: BTreeMap<DiscriminantSet, BTreeMap<usize, CommitCounts>>,
}

impl RunIndex {
    /// An empty index.
    fn new() -> Self {
        Self::default()
    }

    /// Records one run on `commit` (first-parent position `topo_index`) in `set`.
    fn record(&mut self, set: &DiscriminantSet, topo_index: usize, commit: &str, dirty: bool) {
        self.total = self.total.saturating_add(1);
        let entry = self
            .sets
            .entry(set.clone())
            .or_default()
            .entry(topo_index)
            .or_insert_with(|| CommitCounts {
                commit: commit.to_owned(),
                clean: 0,
                dirty: 0,
            });
        if dirty {
            entry.dirty = entry.dirty.saturating_add(1);
        } else {
            entry.clean = entry.clean.saturating_add(1);
        }
    }

    /// Total runs admitted across every set.
    pub(crate) fn total(&self) -> usize {
        self.total
    }

    /// Whether no run entered the selection.
    pub(crate) fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// Runs admitted in `set`.
    pub(crate) fn runs_in_set(&self, set: &DiscriminantSet) -> usize {
        self.sets.get(set).map_or(0, |by_commit| {
            by_commit
                .values()
                .map(|counts| counts.clean.saturating_add(counts.dirty))
                .sum()
        })
    }

    /// Each set with at least one run, paired with its per-commit tallies in
    /// first-parent topological order (oldest first).
    pub(crate) fn sets(
        &self,
    ) -> impl Iterator<Item = (&DiscriminantSet, &BTreeMap<usize, CommitCounts>)> {
        self.sets.iter()
    }
}

/// The data an analysis (or listing) draws on, plus the bookkeeping needed to
/// explain an empty outcome and warn about ephemeral data.
struct SelectedDataSet {
    /// The reconstructed series for the in-window runs, built with the caller's
    /// series filter and ordered by git topology. Pre-blessing: the caller applies
    /// blessings (history mode) or leaves them unapplied (branch/tip, listings).
    series: Vec<Series>,
    /// Compact per-set, per-commit run tallies, standing in for a retained copy of
    /// every loaded object (which a large history cannot afford to keep resident).
    run_index: RunIndex,
    /// How many facet-matching candidates existed before topology filtering.
    candidate_count: usize,
    /// Why candidates were excluded, for the empty-history hint.
    tally: ExclusionTally,
    /// Whether a dirty run was admitted solely by the base-branch dirty-tree
    /// exception (triggers the ephemeral-data warning).
    included_dirty_base_exception: bool,
    /// The target ref the timeline was resolved against (for diagnostics).
    target_ref: String,
    /// The resolved analysis mode (auto-detected from topology, or overridden).
    mode: AnalysisMode,
    /// First-parent topological index of the merge-base, used by branch mode to
    /// split base-side history from the branch's own commits.
    merge_base_index: Option<usize>,
    /// Blessings recorded on in-window commits, grouped by discriminant set. Each
    /// entry pairs the blessed commit's first-parent topological index and its
    /// committer date (from topology, for the report anchor) with the record;
    /// history-mode re-baselining picks, per series, the latest matching blessing.
    /// Empty in branch and tip modes (they ignore blessings).
    blessings: HashMap<DiscriminantSet, Vec<BlessingPlacement>>,
}

/// Resolves one facet's raw command-line values into a [`FacetFilter`].
///
/// The case-insensitive `all` keyword (anywhere in the list) is an explicit
/// synonym for no filter. An empty list auto-detects: the current-machine value
/// when one is supplied (`auto`), else no filter (engine has no host default).
fn resolve_facet(values: &[String], auto: Option<&str>) -> FacetFilter {
    if values.iter().any(|value| value.eq_ignore_ascii_case("all")) {
        return FacetFilter::All;
    }
    match NonEmpty::from_vec(values.to_vec()) {
        Some(values) => FacetFilter::Explicit(values),
        None => auto.map_or(FacetFilter::All, |value| {
            FacetFilter::Auto(value.to_owned())
        }),
    }
}

/// Resolves every command-line facet into a [`DiscriminantSetQuery`] filter, validating that any
/// explicit `--engine` values name a known engine.
///
/// `auto` supplies the current-machine defaults for the triple and machine-key
/// facets when those are omitted. Passing `None` resolves omitted facets to no
/// filter instead — used by the `discriminants` catalog listing, which is a
/// discovery view over all stored partitions rather than the current machine's.
fn resolve_facets(
    selection: &Selection<'_>,
    auto: Option<&AutoFacets>,
) -> Result<DiscriminantSetQuery, RunError> {
    let engine = resolve_facet(selection.engine, None);
    if let FacetFilter::Explicit(values) = &engine {
        for value in values.iter() {
            parse_engine(Some(value))?;
        }
    }
    Ok(DiscriminantSetQuery {
        engine,
        target_triple: resolve_facet(selection.target_triple, auto.map(|a| a.triple.as_str())),
        machine_key: resolve_facet(selection.machine_key, auto.map(|a| a.machine_key.as_str())),
    })
}

/// Lists the stored objects under the project's partition and keeps the ones whose
/// discriminant set matches the facet filters. Shared by the topology-aware
/// selection and the discriminant listing (which needs no repository).
async fn facet_filtered_candidates<S: Storage>(
    storage: &S,
    project_id: &str,
    facets: &DiscriminantSetQuery,
    reporter: &dyn Reporter,
) -> Result<Vec<(String, StorageKey)>, RunError> {
    // The listing prefix must use the same sanitized project segment that
    // `DiscriminantSet` writes its storage keys under. A project id containing a
    // character that sanitizes (a space, `/`, a non-ASCII letter, ...) is stored
    // mangled, so listing under the raw id would silently find an empty history.
    let project = sanitize_segment(project_id);
    let prefix = format!("{STORAGE_VERSION}/{project}/");

    reporter.note(&format!(
        "project id: {project_id} (storage segment: {project})"
    ));
    reporter.note(&format!("listing stored objects under prefix {prefix}"));
    reporter.note_with(|| format!("facet filters: {}", describe_facets(facets)));

    let list_started = Instant::now();
    let keys = storage.list(&prefix).await.map_err(RunError::Storage)?;
    reporter.timing("storage.list(prefix) round-trip", list_started.elapsed());
    reporter.note(&format!(
        "storage returned {}",
        count_noun(keys.len(), "object key")
    ));

    let mut candidates: Vec<(String, StorageKey)> = Vec::new();
    for key in keys {
        if !key.ends_with(".json") {
            reporter.note_with(|| format!("skipping {key}: not a .json object"));
            continue;
        }
        let Some(parsed) = parse_key(&key) else {
            reporter.note_with(|| {
                format!("skipping {key}: not a recognized {STORAGE_VERSION} storage key")
            });
            continue;
        };
        if !facets.matches(&parsed.set) {
            reporter.note_with(|| {
                format!(
                    "skipping {key}: discriminant {} does not match the facet filters",
                    parsed.set
                )
            });
            continue;
        }
        candidates.push((key, parsed));
    }
    reporter.note(&format!(
        "{} match the facet filters",
        count_noun(candidates.len(), "object")
    ));
    Ok(candidates)
}

/// How many stored objects to fetch concurrently while loading a data set.
///
/// `analyze`/`list` load every in-selection object before reconstructing the
/// series. Each [`Storage::get`] is a round-trip — a *network* round-trip against
/// the Azure Blob backend — so fetching them one at a time makes the load a sum of
/// latencies, which dominates wall time at scale. Overlapping a bounded number of
/// fetches turns that sum into roughly its maximum, cutting the per-mode load
/// floor (critical for the remote backend, where thousands of sequential
/// round-trips would otherwise stretch the local floor into minutes). The bound
/// sits near the knee of the throughput curve: enough fetches are in flight to
/// saturate the network path and hide per-object latency, while staying below the
/// point where extra concurrency merely subdivides the fixed path bandwidth among
/// more requests and lengthens each one's latency without lifting throughput.
const LOAD_CONCURRENCY: usize = 128;

/// Fetches and deserializes the given stored objects with bounded concurrency.
///
/// `parse` turns one object's raw bytes into the parsed value `T` (it owns the
/// UTF-8 decoding so the per-type error wording stays exact). The fetches overlap
/// up to [`LOAD_CONCURRENCY`] at a time and therefore complete out of order, so
/// the caller must re-sort the results (by storage key) to keep diagnostics and
/// the loaded order deterministic. The whole operation stays single-threaded and
/// `!Send`, so it runs unchanged under the Miri-driven `block_on` tests.
async fn load_objects_concurrently<S, T, F>(
    storage: &S,
    keys: Vec<(String, StorageKey)>,
    parse: F,
) -> Result<Vec<(String, StorageKey, T)>, RunError>
where
    S: Storage,
    F: Fn(&str, Vec<u8>) -> Result<T, RunError>,
{
    let parse = &parse;
    futures::stream::iter(keys)
        .map(move |(key, parsed)| fetch_one(storage, key, parsed, parse))
        .buffer_unordered(LOAD_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await
}

/// Fetches and deserializes a single stored object. Factored out of
/// [`load_objects_concurrently`] so the stream closure stays a plain `FnMut`
/// returning this future (rather than a closure wrapping an `async` block).
async fn fetch_one<S, T, F>(
    storage: &S,
    key: String,
    parsed: StorageKey,
    parse: &F,
) -> Result<(String, StorageKey, T), RunError>
where
    S: Storage,
    F: Fn(&str, Vec<u8>) -> Result<T, RunError>,
{
    let bytes = storage.get(&key).await.map_err(RunError::Storage)?;
    let value = parse(&key, bytes)?;
    Ok((key, parsed, value))
}

/// Like [`fetch_one`], but carries an arbitrary `rank` through the out-of-order
/// fetch so the caller can recover each object's position (its storage-key
/// ordinal). Kept as a named `async fn` for the same reason as [`fetch_one`]: the
/// stream closure then stays a plain `FnMut` returning this future rather than one
/// wrapping an `async` block.
async fn fetch_one_ranked<S, F>(
    storage: &S,
    rank: usize,
    key: String,
    parsed: StorageKey,
    parse: &F,
) -> Result<(usize, String, StorageKey, Run), RunError>
where
    S: Storage,
    F: Fn(&str, Vec<u8>) -> Result<Run, RunError>,
{
    let (key, parsed, run) = fetch_one(storage, key, parsed, parse).await?;
    Ok((rank, key, parsed, run))
}

/// Resolves the git topology, selects the comparable commits, and loads the
/// in-selection objects into a [`SelectedDataSet`]. Requires a repository: the
/// timeline is reconstructed from git history, not from stored timestamps.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the analyze selection pipeline, which threads the same injected ports"
)]
async fn select_dataset<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    selection: &Selection<'_>,
    filter: SeriesFilter<'_>,
    auto: &AutoFacets,
    now: Timestamp,
    reporter: &dyn Reporter,
) -> Result<SelectedDataSet, RunError>
where
    G: GitHistory,
    S: Storage,
{
    let facets = resolve_facets(selection, Some(auto))?;
    let listing_started = Instant::now();
    let candidates = facet_filtered_candidates(storage, project_id, &facets, reporter).await?;
    reporter.timing(
        "candidate listing + facet filter (includes storage.list)",
        listing_started.elapsed(),
    );

    // Separate blessing sidecars from run objects: they share the partition prefix
    // but carry a different payload and are loaded into their own map rather than
    // the series.
    let (candidates, bless_candidates): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|(_, parsed)| !parsed.is_bless());
    if !bless_candidates.is_empty() {
        reporter.note(&format!(
            "{} of those are blessing sidecars",
            count_noun(bless_candidates.len(), "object")
        ));
    }

    let topology_started = Instant::now();
    let ResolvedHistory {
        target_ref,
        order,
        commit_times,
        admit_dirty,
        dirty_base_exception,
        merge_base_index,
        tip_is_merge_base,
    } = resolve_history(
        git,
        config,
        selection,
        DirtyTipPolicy::WhenWorkingTreeDirty,
        reporter,
    )
    .await?;
    reporter.timing(
        "git topology resolution (resolve_history)",
        topology_started.elapsed(),
    );

    // Mode auto-detection keys off the *recorded data set*, not the on-disk
    // repository state. The branch view exists to compare a feature branch's runs
    // against its base, so it only applies when there is actually feature-branch
    // data: commits past the merge-base, or a dirty run recorded on top of the
    // base tip. A dirty working tree with no dirty run stored on the tip carries no
    // such data, so it stays the long-range history view.
    let dirty_tip_run_present = candidates.iter().any(|(_, parsed)| {
        parsed.is_dirty()
            && dirty_base_exception
                .get(parsed.commit.as_str())
                .copied()
                .unwrap_or(false)
    });

    // The mode steers the analysis and the default `--since`. An explicit `--mode`
    // overrides the auto-detection.
    let mode = match selection.mode_override {
        Some(mode) => {
            reporter.note(&format!(
                "analysis mode: {} (set explicitly via --mode, overriding auto-detection)",
                mode.as_str()
            ));
            mode
        }
        None => {
            let mode = auto_mode(tip_is_merge_base, dirty_tip_run_present);
            reporter.note(&format!(
                "analysis mode: {} (auto-detected because the target tip {} its own merge-base \
                 with the base branch and {} recorded on top of it; the on-disk working-tree \
                 state is deliberately not consulted here)",
                mode.as_str(),
                if tip_is_merge_base { "is" } else { "is not" },
                if dirty_tip_run_present {
                    "a dirty run is"
                } else {
                    "no dirty run is"
                },
            ));
            mode
        }
    };
    let since = resolve_since(selection.since, mode, now)?;
    reporter.note(&format!(
        "since cutoff: {} ({})",
        since.map_or_else(|| "none".to_owned(), |since| since.to_string()),
        since_cutoff_reason(selection.since.is_some(), mode)
    ));
    let until = parse_until(selection.until)?;
    reporter.note(&format!(
        "until cutoff: {}",
        until.map_or_else(|| "none".to_owned(), |until| until.to_string())
    ));

    // Tally why candidates do not enter the analysis, so a `0 runs` outcome can
    // explain itself (via `--verbose` per object, and via a summary hint when
    // candidates existed but none were admitted).
    let candidate_count = candidates.len();
    let mut excluded_outside_history = 0_usize;
    let mut excluded_dirty_base = 0_usize;
    let mut excluded_since = 0_usize;
    let mut excluded_until = 0_usize;
    // Whether at least one dirty run was admitted solely by the base-branch
    // dirty-tree exception, so the report can warn that it is ephemeral.
    let mut included_dirty_base_exception = false;

    // Phase 1 — key-only filtering, in candidate order. Every exclusion that does
    // not need the object's payload runs here, before anything is fetched, so an
    // excluded candidate never costs a round-trip: history membership, base-side
    // dirty admission, and the `--since`/`--until` window (decided from each
    // commit's committer time, which git reports with the topology).
    let phase1_started = Instant::now();
    let mut to_fetch: Vec<(String, StorageKey)> = Vec::new();
    for (key, parsed) in candidates {
        if !order.contains_key(&parsed.commit) {
            excluded_outside_history = excluded_outside_history.saturating_add(1);
            reporter.note_with(|| {
                format!(
                    "excluding {key}: commit {} is not on {target_ref}'s analyzed history",
                    parsed.commit
                )
            });
            continue;
        }
        if parsed.is_dirty()
            && !admit_dirty
                .get(parsed.commit.as_str())
                .copied()
                .unwrap_or(false)
        {
            excluded_dirty_base = excluded_dirty_base.saturating_add(1);
            reporter.note_with(|| {
                format!(
                    "excluding {key}: dirty snapshot on a base-side commit ({} \
                     only admits clean runs); dirty runs count only on the target side",
                    parsed.commit
                )
            });
            continue;
        }
        match window_excludes(commit_times.get(&parsed.commit).copied(), since, until) {
            Some(WindowEdge::Since) => {
                excluded_since = excluded_since.saturating_add(1);
                reporter.note_with(|| {
                    format!(
                        "excluding {key}: commit {} is before the --since cutoff",
                        parsed.commit
                    )
                });
                continue;
            }
            Some(WindowEdge::Until) => {
                excluded_until = excluded_until.saturating_add(1);
                reporter.note_with(|| {
                    format!(
                        "excluding {key}: commit {} is after the --until cutoff",
                        parsed.commit
                    )
                });
                continue;
            }
            None => {}
        }
        to_fetch.push((key, parsed));
    }
    reporter.timing(
        "phase 1 — key-only candidate filtering (no fetches)",
        phase1_started.elapsed(),
    );

    // Phase 2/3 — fetch the survivors concurrently and fold each into the series as
    // it arrives, dropping the parsed run immediately so the whole parsed data set
    // is never resident at once (the peak that drove analysis into tens of
    // gigabytes). Each object's ordinal — the final point tie-break — is its rank
    // in storage-key order, assigned up front because `buffer_unordered` completes
    // out of order. The per-object verbose notes and the run tally are collected
    // during the fold and emitted in storage-key order afterwards, so the
    // diagnostics stay byte-identical to a deterministic in-order pass.
    to_fetch.sort_by(|left, right| left.0.cmp(&right.0));

    let fetch_fold_started = Instant::now();
    let mut builder = SeriesBuilder::new(filter);
    let mut run_index = RunIndex::new();
    // (storage key, admitted-by-dirty-base-exception) per folded object, for the
    // key-ordered verbose notes emitted once the fold completes.
    let mut admitted: Vec<(String, bool)> = Vec::new();
    {
        let parse = |key: &str, bytes: Vec<u8>| -> Result<Run, RunError> {
            let text = String::from_utf8(bytes).map_err(|error| RunError::Analyze {
                message: format!("stored object {key} is not valid UTF-8: {error}"),
            })?;
            Run::from_json(&text).map_err(|error| RunError::Analyze {
                message: format!("stored object {key} is not a valid result set: {error}"),
            })
        };
        let parse = &parse;
        let mut fetches = futures::stream::iter(to_fetch.into_iter().enumerate())
            .map(move |(rank, (key, parsed))| fetch_one_ranked(storage, rank, key, parsed, parse))
            .buffer_unordered(LOAD_CONCURRENCY);

        while let Some(fetched) = fetches.next().await {
            let (rank, key, parsed, run) = fetched?;
            let topo_index = order
                .get(&parsed.commit)
                .copied()
                .expect("phase 1 admitted only commits on the analyzed history");
            let dirty = parsed.is_dirty();
            let is_exception = dirty
                && dirty_base_exception
                    .get(parsed.commit.as_str())
                    .copied()
                    .unwrap_or(false);
            if is_exception {
                included_dirty_base_exception = true;
            }
            run_index.record(&parsed.set, topo_index, &parsed.commit, dirty);
            builder.push(&parsed.set, topo_index, dirty, ordinal_of(rank), &run);
            admitted.push((key, is_exception));
            // `run` is dropped here; only the extracted (compact) points are kept.
        }
    }
    reporter.timing(
        "phase 2/3 — concurrent fetch + serial parse + fold into series",
        fetch_fold_started.elapsed(),
    );

    // Emit the per-object verbose notes in storage-key order — the deterministic
    // order objects were previously admitted in — then the summary.
    admitted.sort_by(|left, right| left.0.cmp(&right.0));
    for (key, is_exception) in &admitted {
        if *is_exception {
            reporter.note_with(|| {
                format!(
                    "including {key}: dirty snapshot on the base-branch tip, admitted \
                     because the working tree is dirty (ephemeral — see the warning)"
                )
            });
        } else {
            reporter.note_with(|| format!("including {key}"));
        }
    }
    let finish_started = Instant::now();
    let series = builder.finish();
    reporter.timing(
        "series build finalization (builder.finish: assemble + serial point sort)",
        finish_started.elapsed(),
    );
    reporter.note(&format!(
        "{} entered the analysis ({excluded_outside_history} outside history, \
         {excluded_dirty_base} dirty-on-base, {excluded_since} before --since, \
         {excluded_until} after --until)",
        count_noun(run_index.total(), "object")
    ));

    // Load the blessing sidecars on in-window commits into a per-set map. A
    // blessing on a commit outside the analyzed history (or that fails to parse) is
    // irrelevant and skipped. Branch and tip modes ignore blessings entirely, so
    // only history mode pays the load.
    let mut blessings: HashMap<DiscriminantSet, Vec<BlessingPlacement>> = HashMap::new();
    if mode == AnalysisMode::History {
        let blessing_started = Instant::now();
        // Phase 1 — key-only filtering: drop blessings whose commit is not on the
        // analyzed history before fetching, in candidate order.
        let mut to_fetch: Vec<(String, StorageKey)> = Vec::new();
        for (key, parsed) in bless_candidates {
            if order.contains_key(&parsed.commit) {
                to_fetch.push((key, parsed));
            } else {
                reporter.note_with(|| {
                    format!(
                        "skipping blessing {key}: commit {} is not on {target_ref}'s analyzed \
                         history",
                        parsed.commit
                    )
                });
            }
        }
        // Phase 2 — fetch and deserialize concurrently, then restore storage-key
        // order (`buffer_unordered` completes out of order).
        let mut fetched = load_objects_concurrently(storage, to_fetch, |key, bytes| {
            let text = String::from_utf8(bytes).map_err(|error| RunError::Analyze {
                message: format!("stored blessing {key} is not valid UTF-8: {error}"),
            })?;
            BlessingRecord::from_json(&text).map_err(|error| RunError::Analyze {
                message: format!("stored blessing {key} is not a valid blessing record: {error}"),
            })
        })
        .await?;
        fetched.sort_by(|left, right| left.0.cmp(&right.0));
        // Phase 3 — record each blessing against its commit's topological index
        // and committer date (resolved from topology, for the report anchor).
        for (key, parsed, record) in fetched {
            let topo_index = order
                .get(&parsed.commit)
                .copied()
                .expect("phase 1 admitted only blessings whose commit is on the analyzed history");
            let commit_time = commit_times.get(&parsed.commit).copied();
            reporter.note_with(|| {
                format!(
                    "loaded blessing {key} ({} accepted at {})",
                    count_noun(record.prefixes.len(), "prefix filter"),
                    parsed.commit
                )
            });
            blessings.entry(parsed.set.clone()).or_default().push((
                topo_index,
                commit_time,
                record,
            ));
        }
        reporter.timing(
            "blessing sidecar load (history mode: filter + fetch + parse)",
            blessing_started.elapsed(),
        );
    }

    Ok(SelectedDataSet {
        series,
        run_index,
        candidate_count,
        tally: ExclusionTally {
            outside_history: excluded_outside_history,
            dirty_base: excluded_dirty_base,
            since: excluded_since,
            until: excluded_until,
        },
        included_dirty_base_exception,
        target_ref,
        mode,
        merge_base_index,
        blessings,
    })
}

/// Narrows a storage-key rank to the series point ordinal width.
///
/// The ordinal is a pure tie-break, so the (practically impossible) overflow past
/// `u32::MAX` distinct in-window objects merely lets the last ordinals collide —
/// the affected points then keep their stable fold order rather than panicking.
#[expect(
    clippy::cast_possible_truncation,
    reason = "saturating: ordinals only tie-break, and >4 billion in-window objects never occur"
)]
fn ordinal_of(rank: usize) -> u32 {
    rank.min(u32::MAX as usize) as u32
}

/// Auto-detects the analysis mode from the resolved topology and recorded data.
///
/// An official base-branch view — the target's tip *is* its own merge-base (or no
/// base is known) and no dirty run is recorded on that tip — is
/// [`AnalysisMode::History`]. Anything else (commits past the merge-base, or a
/// dirty run actually recorded on top of the base tip) is treated as an unnamed
/// feature branch: [`AnalysisMode::Branch`]. The detection looks only at the data
/// set, never at the on-disk working-tree state, so a dirty checkout with no dirty
/// run stored on the tip still analyzes as history. [`AnalysisMode::Tip`] is never
/// auto-selected.
fn auto_mode(tip_is_merge_base: bool, dirty_tip_run_present: bool) -> AnalysisMode {
    if tip_is_merge_base && !dirty_tip_run_present {
        AnalysisMode::History
    } else {
        AnalysisMode::Branch
    }
}

/// Explains, for a verbose note, why the resolved `--since` cutoff is what it is.
fn since_cutoff_reason(explicit_since: bool, mode: AnalysisMode) -> &'static str {
    if explicit_since {
        "from the --since option"
    } else if mode == AnalysisMode::History {
        "history-mode default six-month look-back"
    } else {
        "no default look-back window outside history mode"
    }
}

/// Resolves the effective `--since` cutoff: an explicit value always wins;
/// otherwise history mode applies a default look-back so a scheduled trend watch
/// does not silently widen as history accumulates, while branch and tip modes have
/// no default (a feature branch's whole history is in scope).
fn resolve_since(
    value: Option<&str>,
    mode: AnalysisMode,
    now: Timestamp,
) -> Result<Option<Timestamp>, RunError> {
    if value.is_some() {
        return parse_since(value);
    }
    if mode == AnalysisMode::History {
        return default_history_since(now).map(Some);
    }
    Ok(None)
}

/// Default history-mode look-back window: six months before `now`.
const HISTORY_DEFAULT_LOOKBACK_MONTHS: i32 = 6;

/// The instant [`HISTORY_DEFAULT_LOOKBACK_MONTHS`] before `now`, anchored with
/// calendar-correct zoned arithmetic (months have no fixed length).
fn default_history_since(now: Timestamp) -> Result<Timestamp, RunError> {
    now.to_zoned(TimeZone::UTC)
        .checked_sub(Span::new().months(HISTORY_DEFAULT_LOOKBACK_MONTHS))
        .map(|zoned| zoned.timestamp())
        .map_err(|error| RunError::Analyze {
            message: format!("default --since window is out of the representable range: {error}"),
        })
}

/// Parses the `--mode` option into an explicit [`AnalysisMode`] override.
///
/// `auto` (the default when omitted) resolves to `None` so the mode is detected
/// from topology; `history`, `branch`, and `tip` force that mode.
fn parse_mode(value: Option<&str>) -> Result<Option<AnalysisMode>, RunError> {
    match value {
        None | Some("auto") => Ok(None),
        Some(name) => AnalysisMode::from_name(name)
            .map(Some)
            .ok_or_else(|| RunError::Analyze {
                message: format!(
                    "unknown analysis mode {name:?}; expected auto, history, branch, or tip"
                ),
            }),
    }
}

/// How the base-branch dirty-tip exception is gated.
///
/// On a feature branch the target-side commits admit dirty runs unconditionally;
/// this policy only governs the *base* branch's tip. `analyze`/`list` admit a
/// base-tip dirty run only when the working tree is currently dirty (the
/// "evaluating the tool / accidentally on the base branch" case); `prune` admits
/// base-tip dirty runs regardless of the current working-tree state.
#[derive(Clone, Copy)]
enum DirtyTipPolicy {
    /// Admit a base-side tip's dirty runs only when the working tree is dirty now.
    WhenWorkingTreeDirty,
    /// Always treat a base-side tip as admitting dirty runs.
    Always,
}

/// The git topology a selection resolves to: the target ref it was resolved
/// against, the first-parent position of each selected commit, and the per-commit
/// dirty-admission flags. All maps use owned commit SHAs so the borrowed
/// `selected` set can drop before the caller's load loop.
struct ResolvedHistory {
    /// The target ref the timeline was resolved against (for diagnostics).
    target_ref: String,
    /// First-parent position of each selected commit, for series ordering. An
    /// object whose commit is absent is outside the analyzed history.
    order: HashMap<String, usize>,
    /// Committer timestamp of each first-parent commit, for deciding the
    /// `--since`/`--until` window from topology before any object is fetched. A
    /// commit absent here has an unknown time and is treated as in-window.
    commit_times: HashMap<String, Timestamp>,
    /// Whether each selected commit admits dirty (uncommitted-tree) snapshots.
    admit_dirty: HashMap<String, bool>,
    /// Whether a commit's dirty runs are admitted *only* by the base-branch
    /// dirty-tree exception, which triggers the ephemeral-data warning.
    dirty_base_exception: HashMap<String, bool>,
    /// First-parent topological index of the merge-base, used by branch mode to
    /// split base-side history from the branch's own commits. `None` when no base
    /// is known or the merge-base is off the analyzed chain.
    merge_base_index: Option<usize>,
    /// Whether the target's tip *is* its own merge-base with the base (or no base
    /// is known): the signal that this is an official base-branch view.
    tip_is_merge_base: bool,
}

/// Resolves the git topology for a selection: the target ref's first-parent
/// ancestry, the merge-base with the base ref, and the per-commit dirty-admission
/// flags. Requires a repository — an unresolvable target ref is an error rather
/// than an empty success.
async fn resolve_history<G>(
    git: &G,
    config: &Config,
    selection: &Selection<'_>,
    policy: DirtyTipPolicy,
    reporter: &dyn Reporter,
) -> Result<ResolvedHistory, RunError>
where
    G: GitHistory,
{
    // Resolving the timeline requires a repository: the topology comes from git
    // history, not from stored timestamps. An unresolvable target ref means there
    // is no repository here (or the branch does not exist), which is an error.
    let target_ref = selection.context.unwrap_or("HEAD");
    let Some(target_sha) = git.resolve(target_ref).await.map_err(RunError::Io)? else {
        return Err(RunError::Analyze {
            message: format!(
                "this command requires a git repository: could not resolve {target_ref:?}. \
                 Run inside a repository (or pass --repo / --context)."
            ),
        });
    };

    let base_sha = resolve_base_ref(git, config, selection.base).await?;
    let first_parent_started = Instant::now();
    let first_parent = git.first_parent(&target_sha).await.map_err(RunError::Io)?;
    reporter.timing(
        "git.first_parent ancestry walk (target's first-parent line)",
        first_parent_started.elapsed(),
    );
    // Split the first-parent ancestry into the SHA timeline (for commit selection
    // and the merge-base lookup) and a SHA -> committer-time map (for the window).
    let commit_count = first_parent.len();
    let mut ancestry: Vec<String> = Vec::with_capacity(commit_count);
    let mut commit_times: HashMap<String, Timestamp> = HashMap::new();
    for commit in first_parent {
        if let Some(time) = commit.committer_time {
            commit_times.insert(commit.sha.clone(), time);
        }
        ancestry.push(commit.sha);
    }
    let merge_base = match &base_sha {
        Some(base) => git
            .merge_base(&target_sha, base)
            .await
            .map_err(RunError::Io)?,
        None => None,
    };

    reporter.note(&format!(
        "target ref {target_ref} resolves to {target_sha}; {} on its first-parent line",
        count_noun(commit_count, "commit")
    ));
    reporter.note(&format!(
        "base ref resolves to {}; merge-base with target is {}",
        base_sha.as_deref().unwrap_or("<none>"),
        merge_base.as_deref().unwrap_or("<none>")
    ));

    // The base-branch dirty-tip exception: `analyze`/`list` admit a base-side tip's
    // dirty runs only when the working tree is currently dirty (`--no-dirty` skips
    // both the probe and the exception); `prune` admits them unconditionally so it
    // can remove them regardless of the present working-tree state.
    let dirty_tip_exception = match policy {
        DirtyTipPolicy::Always => !selection.no_dirty,
        DirtyTipPolicy::WhenWorkingTreeDirty => {
            let working_tree_dirty = if selection.no_dirty {
                false
            } else {
                git.is_dirty().await.map_err(RunError::Io)?
            };
            if working_tree_dirty {
                reporter.note(
                    "working tree is dirty: dirty snapshots on a base-side tip will be admitted",
                );
            }
            working_tree_dirty
        }
    };

    let selected = select_commits(
        &ancestry,
        merge_base.as_deref(),
        !selection.no_dirty,
        dirty_tip_exception,
    );
    let order: HashMap<String, usize> = selected
        .iter()
        .enumerate()
        .map(|(index, one)| (one.commit.clone(), index))
        .collect();
    let admit_dirty: HashMap<String, bool> = selected
        .iter()
        .map(|one| (one.commit.clone(), one.admit_dirty))
        .collect();
    let dirty_base_exception: HashMap<String, bool> = selected
        .iter()
        .map(|one| (one.commit.clone(), one.dirty_base_exception))
        .collect();

    // The merge-base's topological position (when it is on the analyzed chain)
    // splits base-side history from the branch's own commits in branch mode.
    let merge_base_index = merge_base
        .as_deref()
        .and_then(|base| order.get(base).copied());
    // The target's tip is its own merge-base (or no base is known) exactly when
    // this is an official base-branch view rather than a feature branch.
    let tip_is_merge_base = merge_base.as_deref().is_none_or(|base| base == target_sha);

    Ok(ResolvedHistory {
        target_ref: target_ref.to_owned(),
        order,
        commit_times,
        admit_dirty,
        dirty_base_exception,
        merge_base_index,
        tip_is_merge_base,
    })
}

/// The ephemeral-data warning appended when a dirty base-branch-tip run is admitted.
fn dirty_base_exception_warning() -> String {
    "Warning: analysis included dirty runs (with uncommitted changes) on top of the \
     base branch. These may be excluded from future analysis. Switch to a new branch \
     to persist benchmark history of your changes."
        .to_owned()
}

/// Resolves the base ref the target's history is split against, returning its
/// commit SHA or `None` when no base can be determined.
///
/// Precedence: an explicit `--base` (an error if it does not resolve), then the
/// configured `project.default_branch`, then the repository's detected default
/// branch (`origin/HEAD`, else `main`/`master`).
async fn resolve_base_ref<G: GitHistory>(
    git: &G,
    config: &Config,
    base: Option<&str>,
) -> Result<Option<String>, RunError> {
    if let Some(base) = base {
        return git
            .resolve(base)
            .await
            .map_err(RunError::Io)?
            .map(Some)
            .ok_or_else(|| RunError::Analyze {
                message: format!("could not resolve --base {base:?}"),
            });
    }
    if let Some(default) = config.project.default_branch.as_deref()
        && let Some(resolved) = git.resolve(default).await.map_err(RunError::Io)?
    {
        return Ok(Some(resolved));
    }
    if let Some(name) = git.default_branch().await.map_err(RunError::Io)?
        && let Some(resolved) = git.resolve(&name).await.map_err(RunError::Io)?
    {
        return Ok(Some(resolved));
    }
    Ok(None)
}

/// Resolves the *display name* of the base ref (without resolving it to a SHA),
/// for diagnostics such as the `prune --prune-base` guard.
///
/// Mirrors [`resolve_base_ref`]'s precedence: an explicit `--base`, then the
/// configured `project.default_branch` (only when it resolves), then the
/// repository's detected default branch. Returns `None` when no base can be named.
async fn resolve_base_name<G: GitHistory>(
    git: &G,
    config: &Config,
    base: Option<&str>,
) -> Result<Option<String>, RunError> {
    if let Some(base) = base {
        return Ok(Some(base.to_owned()));
    }
    if let Some(default) = config.project.default_branch.as_deref()
        && git.resolve(default).await.map_err(RunError::Io)?.is_some()
    {
        return Ok(Some(default.to_owned()));
    }
    git.default_branch().await.map_err(RunError::Io)
}

/// Parses the `--format` option, defaulting to text.
fn parse_format(name: Option<&str>) -> Result<ReportFormat, RunError> {
    match name {
        None => Ok(ReportFormat::Text),
        Some(name) => ReportFormat::from_name(name).ok_or_else(|| RunError::Analyze {
            message: format!("unknown report format {name:?}; expected text, json, or markdown"),
        }),
    }
}

/// A human-readable summary of the active facet filters, for `--verbose` notes.
fn describe_facets(facets: &DiscriminantSetQuery) -> String {
    let parts = [
        ("engine", &facets.engine),
        ("target_triple", &facets.target_triple),
        ("machine_key", &facets.machine_key),
    ]
    .into_iter()
    .filter_map(|(label, filter)| describe_filter(filter).map(|value| format!("{label}={value}")))
    .collect::<Vec<_>>();
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

/// Renders one facet filter, or `None` when it imposes no constraint.
fn describe_filter(filter: &FacetFilter) -> Option<String> {
    match filter {
        FacetFilter::All => None,
        FacetFilter::Auto(value) => Some(format!("{value} (auto-detected)")),
        FacetFilter::Explicit(values) => Some(values.iter().cloned().collect::<Vec<_>>().join("|")),
    }
}

/// How many facet-matching candidates were excluded, by reason.
#[derive(Clone, Copy, Debug)]
struct ExclusionTally {
    /// Commit is not on the analyzed first-parent history.
    outside_history: usize,
    /// Dirty snapshot on a base-side commit (clean-only).
    dirty_base: usize,
    /// Effective time is before the `--since` cutoff.
    since: usize,
    /// Effective time is after the `--until` cutoff.
    until: usize,
}

/// Builds a diagnostic hint for the case where stored runs matched the facet
/// filters but none entered the analysis, so the empty outcome explains itself.
///
/// Returns `None` when at least one run was loaded, or when there were no
/// candidates at all (a genuinely empty history needs no special explanation).
fn empty_history_hint(
    loaded_is_empty: bool,
    candidate_count: usize,
    target_ref: &str,
    tally: ExclusionTally,
) -> Option<String> {
    if !loaded_is_empty || candidate_count == 0 {
        return None;
    }

    let mut lines = vec![format!(
        "Found {} for this project, but none entered the analysis:",
        count_noun(candidate_count, "stored run")
    )];
    if tally.dirty_base > 0 {
        lines.push(format!(
            "  - {} on base-branch commits — only clean runs count on the base \
             branch. Commit your working tree (including the configuration file) and re-run, \
             or analyze a feature branch with --context.",
            count_noun(tally.dirty_base, "dirty (uncommitted-tree) snapshot")
        ));
    }
    if tally.outside_history > 0 {
        lines.push(format!(
            "  - {} on commits outside {target_ref}'s analyzed history — check out the \
             branch they were recorded on, or pass --context.",
            count_noun(tally.outside_history, "run")
        ));
    }
    if tally.since > 0 {
        lines.push(format!(
            "  - {} older than the --since cutoff.",
            count_noun(tally.since, "run")
        ));
    }
    if tally.until > 0 {
        lines.push(format!(
            "  - {} newer than the --until cutoff.",
            count_noun(tally.until, "run")
        ));
    }
    lines.push("Re-run with --verbose for a per-object explanation.".to_owned());
    Some(lines.join("\n"))
}

/// Parses an `--engine` facet value into an [`Engine`], if set.
fn parse_engine(name: Option<&str>) -> Result<Option<Engine>, RunError> {
    match name {
        None => Ok(None),
        Some(name) => Engine::from_name(name)
            .map(Some)
            .ok_or_else(|| RunError::Analyze {
                message: format!(
                    "unknown engine {name:?}; expected one of: criterion, callgrind, \
                     alloc_tracker, all_the_time"
                ),
            }),
    }
}

/// Parses the `--since` option into an absolute lower-bound instant, if set.
///
/// See [`parse_instant`] for the accepted input forms.
fn parse_since(value: Option<&str>) -> Result<Option<Timestamp>, RunError> {
    parse_instant(value, "--since")
}

/// Parses the `--until` option into an absolute upper-bound instant, if set.
///
/// See [`parse_instant`] for the accepted input forms.
fn parse_until(value: Option<&str>) -> Result<Option<Timestamp>, RunError> {
    parse_instant(value, "--until")
}

/// Which edge of the `--since`/`--until` window a commit falls outside of.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowEdge {
    /// The commit predates the `--since` lower bound.
    Since,
    /// The commit postdates the `--until` upper bound.
    Until,
}

/// Decides whether a commit's `committer_time` places it outside the
/// `--since`/`--until` window, reporting which edge it fell off.
///
/// `--since` is an inclusive-on-or-after lower bound and `--until` an
/// inclusive-on-or-before upper bound, matching the object-timestamp comparison
/// the analysis used before topology carried the time. A commit with an unknown
/// time (`None`) is never excluded — git always reports a committer date for a
/// real commit, so this only guards a degenerate input conservatively (by
/// keeping the object and letting the rest of the pipeline judge it).
fn window_excludes(
    committer_time: Option<Timestamp>,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
) -> Option<WindowEdge> {
    let time = committer_time?;
    if since.is_some_and(|since| time < since) {
        return Some(WindowEdge::Since);
    }
    if until.is_some_and(|until| time > until) {
        return Some(WindowEdge::Until);
    }
    None
}

/// Parses a time-cutoff option into an absolute instant, if set.
///
/// Three input forms are accepted, tried in order:
///
/// * an RFC 3339 timestamp (`2024-01-01T00:00:00Z`),
/// * a bare `YYYY-MM-DD` date, interpreted at UTC midnight, and
/// * a relative duration in jiff's friendly or ISO 8601 form (`5 months`,
///   `5 months ago`, `P6M`, `2w`), interpreted as *that far in the past* —
///   resolved against the current instant via calendar-correct zoned arithmetic.
///
/// The relative form is normalized through [`Span::abs`] before subtracting, so a
/// duration written with the friendly `ago` suffix (which jiff parses as a
/// *negative* span) still means "this far back" rather than flipping into the
/// future. A cutoff in the future is never a sensible bound, so both `5 months`
/// and `5 months ago` resolve to the same past instant.
fn parse_instant(value: Option<&str>, flag: &str) -> Result<Option<Timestamp>, RunError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if let Ok(timestamp) = value.parse::<Timestamp>() {
        return Ok(Some(timestamp));
    }
    if let Ok(date) = value.parse::<Date>() {
        // UTC has no DST transitions, so civil midnight always maps to an instant.
        let zoned = date
            .to_zoned(TimeZone::UTC)
            .expect("UTC midnight is always a valid instant");
        return Ok(Some(zoned.timestamp()));
    }
    if let Ok(span) = value.parse::<Span>() {
        return Ok(Some(instant_before_now(span, flag)?));
    }
    Err(RunError::Analyze {
        message: format!(
            "invalid {flag} value {value:?}; expected an RFC 3339 timestamp, a YYYY-MM-DD \
             date, or a relative duration such as \"6 months\" or \"30 days ago\""
        ),
    })
}

/// Resolves a relative [`Span`] to the instant that far before now, treating the
/// span's magnitude as a look-back regardless of its sign (see [`parse_instant`]).
///
/// Calendar units (months, years) have no fixed length, so the subtraction is
/// anchored to the current UTC zoned datetime rather than to a bare duration.
fn instant_before_now(span: Span, flag: &str) -> Result<Timestamp, RunError> {
    let now = Timestamp::now().to_zoned(TimeZone::UTC);
    now.checked_sub(span.abs())
        .map(|zoned| zoned.timestamp())
        .map_err(|error| RunError::Analyze {
            message: format!("{flag} duration is out of the representable range: {error}"),
        })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use futures::executor::block_on;
    use jiff::Timestamp;

    use crate::config::{Config, parse_config};
    use crate::git_history::FakeGitHistory;
    use crate::model::{BenchmarkId, BenchmarkIdPrefix, BenchmarkResult, Metric, MetricKind};
    use crate::model::{EnvironmentInfo, GitInfo, RunContext, ToolchainInfo};
    use crate::report::RecordingReporter;
    use crate::storage::{MemoryStorage, Storage};

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
                short_commit: Some(commit.to_owned()),
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
        format!("v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json")
    }

    /// The clean object key for `commit` in an arbitrary engine/triple/machine-key partition.
    fn clean_key_in(engine: &str, triple: &str, machine: &str, commit: &str) -> String {
        format!("v1/folo/{engine}/{triple}/{machine}/{commit}/clean.json")
    }

    /// A stored result set whose single record carries two metrics (`Ir` and
    /// `EstimatedCycles`), so its partition reconstructs two distinct series.
    fn two_metric_set(effective: i64, commit: &str, ir: f64, cycles: f64) -> Run {
        let time = ts(effective);
        let context = RunContext::new(
            time,
            GitInfo {
                commit: Some(commit.to_owned()),
                short_commit: Some(commit.to_owned()),
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
                Metric::new(MetricKind::EstimatedCycles, cycles),
            ],
        );
        Run::new(context, vec![record])
    }

    /// A dirty snapshot key for `commit` taken at `unix`.
    fn dirty_key(commit: &str, unix: i64) -> String {
        format!("v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/dirty-{unix}.json")
    }

    /// Stores a value at `key` in `storage`, panicking on failure (test helper).
    fn store(storage: &MemoryStorage, key: &str, set: &Run) {
        let json = set.to_json().unwrap();
        block_on(storage.put(key, json.as_bytes())).unwrap();
    }

    /// A linear master history `c0 - c1 - c2 - c3`, HEAD at the tip. Each commit
    /// carries committer time `ts(N)` for `cN`, matching the `effective`-second
    /// convention the seeders use, so the topology-decided `--since`/`--until`
    /// window can be exercised.
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
        cargo_bench_history_core::testing::synchronous_spawner()
    }

    #[test]
    fn auto_mode_uses_tip_topology_and_recorded_dirty_runs_not_repo_state() {
        // A base branch whose tip is its own merge-base with no dirty run recorded
        // on the tip is history mode.
        assert_eq!(auto_mode(true, false), AnalysisMode::History);
        // Commits past the merge-base, or a dirty run actually recorded on top of
        // the tip, make it a (possibly unnamed) feature branch.
        assert_eq!(auto_mode(false, false), AnalysisMode::Branch);
        assert_eq!(auto_mode(true, true), AnalysisMode::Branch);
        assert_eq!(auto_mode(false, true), AnalysisMode::Branch);
    }

    #[test]
    fn since_cutoff_reason_explains_each_source() {
        assert_eq!(
            since_cutoff_reason(true, AnalysisMode::History),
            "from the --since option"
        );
        assert_eq!(
            since_cutoff_reason(true, AnalysisMode::Branch),
            "from the --since option"
        );
        assert_eq!(
            since_cutoff_reason(false, AnalysisMode::History),
            "history-mode default six-month look-back"
        );
        assert_eq!(
            since_cutoff_reason(false, AnalysisMode::Branch),
            "no default look-back window outside history mode"
        );
    }

    #[test]
    fn should_colorize_only_in_an_interactive_terminal_without_no_color() {
        assert!(should_colorize(true, false), "terminal, NO_COLOR unset");
        assert!(!should_colorize(false, false), "not a terminal");
        assert!(!should_colorize(true, true), "NO_COLOR set");
        assert!(!should_colorize(false, true), "neither");
    }

    #[test]
    fn describe_facets_joins_set_facets_and_reports_none_when_empty() {
        let empty = DiscriminantSetQuery {
            engine: FacetFilter::All,
            target_triple: FacetFilter::All,
            machine_key: FacetFilter::All,
        };
        assert_eq!(describe_facets(&empty), "none");

        let full = DiscriminantSetQuery {
            engine: FacetFilter::Explicit(nonempty!["criterion".to_owned()]),
            target_triple: FacetFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: FacetFilter::Explicit(nonempty!["abcd".to_owned()]),
        };
        assert_eq!(
            describe_facets(&full),
            "engine=criterion, target_triple=x86_64-pc-windows-msvc (auto-detected), \
             machine_key=abcd"
        );
    }

    #[test]
    fn empty_history_hint_explains_only_when_runs_were_excluded() {
        let no_exclusions = ExclusionTally {
            outside_history: 0,
            dirty_base: 0,
            since: 0,
            until: 0,
        };
        // No candidates at all → a genuinely empty history needs no hint.
        assert_eq!(empty_history_hint(true, 0, "master", no_exclusions), None);
        // Runs were actually loaded → no hint.
        assert_eq!(empty_history_hint(false, 3, "master", no_exclusions), None);

        let tally = ExclusionTally {
            outside_history: 2,
            dirty_base: 1,
            since: 4,
            until: 0,
        };
        let hint = empty_history_hint(true, 7, "master", tally).unwrap();
        assert!(hint.contains("7 stored runs"), "{hint}");
        assert!(
            hint.contains("1 dirty (uncommitted-tree) snapshot"),
            "{hint}"
        );
        assert!(hint.contains("2 runs on commits outside master"), "{hint}");
        assert!(
            hint.contains("4 runs older than the --since cutoff"),
            "{hint}"
        );
        assert!(hint.contains("--verbose"), "{hint}");

        // A zero reason omits its line entirely (each `> 0` guard is exercised in
        // both directions): only the dirty reason is present here.
        let dirty_only = ExclusionTally {
            outside_history: 0,
            dirty_base: 3,
            since: 0,
            until: 0,
        };
        let hint = empty_history_hint(true, 3, "master", dirty_only).unwrap();
        assert!(hint.contains("dirty (uncommitted-tree)"), "{hint}");
        assert!(!hint.contains("outside"), "{hint}");
        assert!(!hint.contains("--since cutoff"), "{hint}");
        assert!(!hint.contains("--until cutoff"), "{hint}");

        // Only the outside-history reason is present here (dirty omitted).
        let outside_only = ExclusionTally {
            outside_history: 2,
            dirty_base: 0,
            since: 0,
            until: 0,
        };
        let hint = empty_history_hint(true, 2, "master", outside_only).unwrap();
        assert!(hint.contains("outside master"), "{hint}");
        assert!(!hint.contains("dirty (uncommitted-tree)"), "{hint}");
        assert!(!hint.contains("--since cutoff"), "{hint}");

        // Only the until reason is present here.
        let until_only = ExclusionTally {
            outside_history: 0,
            dirty_base: 0,
            since: 0,
            until: 5,
        };
        let hint = empty_history_hint(true, 5, "master", until_only).unwrap();
        assert!(
            hint.contains("5 runs newer than the --until cutoff"),
            "{hint}"
        );
        assert!(!hint.contains("dirty (uncommitted-tree)"), "{hint}");
        assert!(!hint.contains("--since cutoff"), "{hint}");
    }

    #[test]
    fn parse_mode_resolves_auto_to_none_and_rejects_unknown() {
        assert_eq!(parse_mode(None).unwrap(), None);
        assert_eq!(parse_mode(Some("auto")).unwrap(), None);
        assert_eq!(
            parse_mode(Some("history")).unwrap(),
            Some(AnalysisMode::History)
        );
        assert_eq!(
            parse_mode(Some("branch")).unwrap(),
            Some(AnalysisMode::Branch)
        );
        assert_eq!(parse_mode(Some("tip")).unwrap(), Some(AnalysisMode::Tip));
        let error = parse_mode(Some("weekly")).unwrap_err();
        assert!(
            error.to_string().contains("unknown analysis mode"),
            "{error}"
        );
    }

    #[test]
    fn parse_engine_resolves_none_known_and_rejects_unknown() {
        assert!(parse_engine(None).unwrap().is_none());
        assert!(parse_engine(Some("callgrind")).unwrap().is_some());
        let error = parse_engine(Some("nonsuch")).unwrap_err();
        assert!(error.to_string().contains("unknown"), "{error}");
    }

    #[test]
    fn facet_filter_skips_an_unrecognized_storage_key() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        // A `.json` object under the project prefix whose key is not a valid
        // seven-segment storage key is noted and skipped, not parsed as data.
        block_on(storage.put("v1/folo/bogus.json", b"{}")).unwrap();
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

    #[test]
    fn resolve_since_applies_a_default_only_in_history_mode() {
        let now = "2024-06-01T00:00:00Z".parse::<Timestamp>().unwrap();
        // History mode without an explicit value falls back to the six-month window.
        let history = resolve_since(None, AnalysisMode::History, now)
            .unwrap()
            .unwrap();
        assert_eq!(
            history,
            "2023-12-01T00:00:00Z".parse::<Timestamp>().unwrap()
        );
        // Branch and tip modes have no default cutoff.
        assert_eq!(
            resolve_since(None, AnalysisMode::Branch, now).unwrap(),
            None
        );
        assert_eq!(resolve_since(None, AnalysisMode::Tip, now).unwrap(), None);
        // An explicit value always wins, even in branch mode.
        let explicit = resolve_since(Some("2024-01-01"), AnalysisMode::Branch, now)
            .unwrap()
            .unwrap();
        assert_eq!(
            explicit,
            "2024-01-01T00:00:00Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn default_history_since_subtracts_six_calendar_months() {
        let now = "2024-03-31T00:00:00Z".parse::<Timestamp>().unwrap();
        // Calendar arithmetic, not a fixed number of days: six months before
        // 2024-03-31 is 2023-09-30 (September has 30 days).
        let cutoff = default_history_since(now).unwrap();
        assert_eq!(cutoff, "2023-09-30T00:00:00Z".parse::<Timestamp>().unwrap());
    }

    #[test]
    fn window_excludes_decides_each_edge_from_committer_time() {
        let since = Some(ts(10));
        let until = Some(ts(20));
        // Before the lower bound, after the upper bound, and inside the window.
        assert_eq!(
            window_excludes(Some(ts(5)), since, until),
            Some(WindowEdge::Since)
        );
        assert_eq!(
            window_excludes(Some(ts(25)), since, until),
            Some(WindowEdge::Until)
        );
        assert_eq!(window_excludes(Some(ts(15)), since, until), None);
        // Both bounds are inclusive: a commit exactly on an edge stays in-window.
        assert_eq!(window_excludes(Some(ts(10)), since, until), None);
        assert_eq!(window_excludes(Some(ts(20)), since, until), None);
        // An open bound never excludes on that side.
        assert_eq!(window_excludes(Some(ts(0)), None, until), None);
        assert_eq!(window_excludes(Some(ts(99)), since, None), None);
        // An unknown committer time is never excluded, even with both bounds set.
        assert_eq!(window_excludes(None, since, until), None);
    }

    /// Runs `analyze_with` and unwraps the rendered report and regression count.
    fn analyze(
        git: &FakeGitHistory,
        storage: &MemoryStorage,
        project: &str,
        options: &AnalyzeOptions,
    ) -> (String, usize) {
        let reporter = RecordingReporter::new();
        let outcome = block_on(analyze_with(
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
        match outcome {
            RunOutcome::Analyzed {
                report,
                regressions,
                ..
            } => (report, regressions),
            RunOutcome::Completed { message } => (message, 0),
        }
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
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
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
        let mut json = options();
        json.format = Some("json".to_owned());

        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let (report, regressions) = analyze(&linear6_git(), &storage, "folo", &json);
        assert_eq!(regressions, 1);
        assert!(report.contains("\"notable\": true"), "{report}");

        let empty = MemoryStorage::new();
        let (report, _) = analyze(&linear_git(), &empty, "folo", &json);
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
            "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/c3/bless-3.json".to_owned();
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
            "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/c3/bless-3.json".to_owned();
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
    fn analyze_blessing_error(storage: &MemoryStorage) -> RunError {
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
            "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/c3/bless-3.json".to_owned();
        block_on(storage.put(&bless_key, &[0xff, 0xfe, 0x00])).unwrap();
        let error = analyze_blessing_error(&storage);
        match error {
            RunError::Analyze { message } => {
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
            "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/c3/bless-3.json".to_owned();
        block_on(storage.put(&bless_key, b"{ not a blessing record")).unwrap();
        let error = analyze_blessing_error(&storage);
        match error {
            RunError::Analyze { message } => {
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
            "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/z9/bless-3.json".to_owned();
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
        // Set A — callgrind/linux/synthetic: three runs (c0..c2), each carrying two
        // metrics so the set reconstructs two distinct series.
        for index in 0..3 {
            let commit = format!("c{index}");
            let second = i64::from(index);
            store(
                &storage,
                &clean_key(&commit),
                &two_metric_set(second, &commit, 100.0, 200.0),
            );
        }
        // Set B — callgrind/darwin/synthetic: two runs (c0..c1), each carrying one
        // metric so the set reconstructs a single series. Distinct run AND series
        // counts from set A make an `==`/`!=` swap in either per-set tally observable.
        for index in 0..2 {
            let commit = format!("c{index}");
            let second = i64::from(index);
            store(
                &storage,
                &clean_key_in("callgrind", "aarch64-apple-darwin", "synthetic", &commit),
                &ir_set(second, &commit, 100.0),
            );
        }

        let git = linear_git();
        let mut options = options();
        options.format = Some("json".to_owned());
        let (report, _) = analyze(&git, &storage, "folo", &options);

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

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, regressions) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "the dirty tip run is excluded");
        assert_eq!(regressions, 0);
    }

    #[test]
    fn feature_view_admits_dirty_after_the_merge_base() {
        // feature branched at c1; the target side rises at f1 and the dirty f2
        // snapshot sustains the new level. The dirty run is admitted (runs == 4)
        // and is essential to the flag: without it the lone f1 point cannot satisfy
        // the change-point detector's persistence requirement.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 130.0));
        store(&storage, &dirty_key("f2", 3), &ir_set(3, "f2", 130.0));
        let git = feature_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, regressions) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 4, "the dirty f2 snapshot is admitted");
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
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
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

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
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

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let reporter = RecordingReporter::new();
        let outcome = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();
        let RunOutcome::Analyzed { report, .. } = outcome else {
            panic!("expected an analysis outcome");
        };

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
        // admitted, with a warning that they are ephemeral. Two snapshots at the
        // raised level complete a sustained step over the clean baseline.
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
        let mut git = linear_git();
        git.mark_dirty();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let reporter = RecordingReporter::new();
        let outcome = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();
        let RunOutcome::Analyzed {
            report,
            regressions,
            ..
        } = outcome
        else {
            panic!("expected an analysis outcome");
        };

        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 5, "both dirty tip snapshots are admitted");
        assert_eq!(regressions, 1, "the dirty tip snapshots complete the step");
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

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "the dirty tip run stays excluded");
        assert!(
            parsed["warning"].is_null(),
            "no warning when the tree is clean"
        );
    }

    #[test]
    fn dirty_working_tree_without_recorded_dirty_runs_stays_history_mode() {
        // The reported corner case: on the base branch with a currently-dirty
        // working tree but ONLY clean runs recorded (no dirty run on the tip), mode
        // auto-detection must look at the *data set*, not the on-disk tree, and pick
        // history mode — so the long-range change-point detector still flags the
        // sustained step. The old behaviour keyed off `git.is_dirty()` and wrongly
        // fell into branch mode here.
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let mut git = linear6_git();
        git.mark_dirty();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let reporter = RecordingReporter::new();
        let outcome = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();
        let RunOutcome::Analyzed {
            report,
            regressions,
            ..
        } = outcome
        else {
            panic!("expected an analysis outcome");
        };

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
                && reporter.contains("working-tree state is deliberately not consulted"),
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
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 3, "--no-dirty drops the dirty tip snapshot");
        assert!(parsed["warning"].is_null(), "no warning under --no-dirty");
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

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let reporter = RecordingReporter::new();
        let outcome = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            now_anchor(),
            &reporter,
            false,
            &spawner(),
        ))
        .unwrap();
        let RunOutcome::Analyzed { report, .. } = outcome else {
            panic!("expected an analysis outcome");
        };
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

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
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
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, regressions) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 6, "master's six commits");
        assert_eq!(regressions, 1);
    }

    #[test]
    fn within_a_commit_clean_precedes_dirty() {
        // On a target-side commit, a clean run and dirty snapshots both load; the
        // clean run is the baseline and the later dirty values are the latest
        // points. Two dirty snapshots at the raised level complete a sustained step.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 100.0));
        store(&storage, &clean_key("f2"), &ir_set(3, "f2", 100.0));
        store(&storage, &dirty_key("f2", 4), &ir_set(4, "f2", 130.0));
        store(&storage, &dirty_key("f2", 5), &ir_set(5, "f2", 130.0));
        let git = feature_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (_, regressions) = analyze(&git, &storage, "folo", &opts);
        assert_eq!(regressions, 1, "the dirty f2 values are the latest points");
    }

    #[test]
    fn target_triple_facet_selects_the_windows_set() {
        // Two sets differing only by triple; an explicit `--target-triple` reports
        // just the matching one, even though the auto-detected default is Linux.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v1/folo/callgrind/x86_64-pc-windows-msvc/synthetic/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            target_triple: vec!["x86_64-pc-windows-msvc".to_owned()],
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
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
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v1/folo/callgrind/x86_64-pc-windows-msvc/synthetic/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            target_triple: vec!["x86_64-unknown-linux-gnu".to_owned()],
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
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
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v1/folo/callgrind/x86_64-pc-windows-msvc/synthetic/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
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
            "v1/folo/criterion/x86_64-unknown-linux-gnu/synthetic/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            engine: vec!["callgrind".to_owned()],
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
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
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 2, "only c2 and c3 are within the window");
    }

    #[test]
    fn until_window_excludes_later_runs() {
        let storage = MemoryStorage::new();
        // c0..c3 at epoch seconds 0..3. `--until` epoch 1 keeps only c0 and c1.
        for (index, value) in [100.0, 100.0, 130.0, 130.0].into_iter().enumerate() {
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
            until: Some("1970-01-01T00:00:01Z".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 2, "only c0 and c1 are within the window");
    }

    #[test]
    fn analyze_without_a_resolvable_base_branch_loads_the_whole_history() {
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        // HEAD resolves, but there is no advertised default branch and no --base /
        // config default, so resolve_base_ref yields None and there is no
        // merge-base to split the timeline on. The analysis still loads the
        // history rather than erroring.
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("c4", Some("c3"))
            .commit("c5", Some("c4"))
            .branch("master", "c5")
            .head("master"); // No `.mark_default(...)`.
        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            parsed["runs"], 6,
            "the full history loads without a merge-base: {report}"
        );
    }

    #[test]
    fn history_is_found_for_a_project_id_that_requires_sanitizing() {
        // `run` stores under the sanitized project segment, so `analyze` must list
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
                "v1/{sanitized}/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json"
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
    fn analyzed_outcome_is_always_successful() {
        // The exit code no longer depends on findings: even a flagged regression
        // yields a successful outcome (the signal lives in the report JSON).
        let storage = MemoryStorage::new();
        seed_linear_step(&storage);
        let git = linear6_git();

        let outcome = block_on(analyze_with(
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
        .unwrap();
        assert!(outcome.is_success(), "findings must never fail the build");
    }

    #[test]
    fn json_format_is_rendered() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 10.0));
        let git = linear_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["project"], "folo");
        assert_eq!(parsed["runs"], 1);
    }

    #[test]
    fn non_json_objects_are_skipped() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 10.0));
        // A stray non-result object under the prefix must be ignored, not parsed.
        block_on(storage.put("v1/folo/callgrind/README.txt", b"not json")).unwrap();
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
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
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
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn unknown_format_is_rejected() {
        let storage = MemoryStorage::new();
        let git = linear_git();
        let opts = AnalyzeOptions {
            format: Some("yaml".to_owned()),
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
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
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
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
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
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
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
            format: Some("json".to_owned()),
            ..options()
        };
        let outcome = block_on(analyze_with(
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
        let RunOutcome::Analyzed { report, .. } = outcome else {
            panic!("expected an analyzed outcome");
        };
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        // c1's dirty run is base-side (excluded); c0, c1 clean and f1 clean load.
        assert_eq!(
            parsed["runs"], 3,
            "base-side dirty c1 excluded via config base"
        );
    }

    #[test]
    fn since_accepts_timestamp_and_date() {
        assert_eq!(
            parse_since(Some("2024-01-01T00:00:00Z")).unwrap(),
            Some("2024-01-01T00:00:00Z".parse().unwrap())
        );
        assert_eq!(
            parse_since(Some("2024-01-01")).unwrap(),
            Some("2024-01-01T00:00:00Z".parse().unwrap())
        );
        assert_eq!(parse_since(None).unwrap(), None);
    }

    #[test]
    #[cfg_attr(miri, ignore = "relative `--since` parsing reads the wall clock")]
    fn since_accepts_relative_durations_as_look_back() {
        // A friendly duration resolves to an instant in the past, and the `ago`
        // suffix (which jiff parses as a negative span) means the same look-back
        // rather than flipping into the future.
        let now = Timestamp::now();
        let plain = parse_since(Some("5 months")).unwrap().unwrap();
        let with_ago = parse_since(Some("5 months ago")).unwrap().unwrap();
        assert!(plain < now, "a look-back must be in the past");
        assert!(with_ago < now, "the `ago` suffix must still look back");
        // The cutoff is `now - 5 months`, NOT the 1970 epoch: a constant default
        // would also satisfy `< now`, so bound it from below at roughly a year back.
        let one_year_back = now.as_second() - 400 * 24 * 60 * 60;
        assert!(
            plain.as_second() > one_year_back,
            "a 5-month look-back must land near now, not the epoch"
        );
        // Both spellings denote the same magnitude, so they land within a tiny
        // wall-clock window of each other (the two `now` reads differ by µs).
        let gap = (plain.as_second() - with_ago.as_second()).abs();
        assert!(gap <= 1, "`5 months` and `5 months ago` agree (gap {gap}s)");
    }

    #[test]
    #[cfg_attr(miri, ignore = "relative `--since` parsing reads the wall clock")]
    fn since_accepts_iso_and_week_durations() {
        let now = Timestamp::now();
        for input in ["P6M", "2w", "30 days", "-P1Y"] {
            let cutoff = parse_since(Some(input)).unwrap().unwrap();
            assert!(cutoff < now, "{input:?} must resolve to the past");
        }
    }

    #[test]
    fn since_rejects_garbage() {
        let error = parse_since(Some("not-a-date")).unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn run_index_counts_runs_and_reports_emptiness() {
        let set = DiscriminantSet {
            engine: "criterion".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine_key: "synthetic".to_owned(),
        };
        let mut index = RunIndex::new();
        assert!(index.is_empty(), "a fresh index admits no runs");
        assert_eq!(index.total(), 0);
        assert_eq!(index.runs_in_set(&set), 0);

        index.record(&set, 0, "c0", false);
        index.record(&set, 0, "c0", true);
        index.record(&set, 1, "c1", false);

        assert!(
            !index.is_empty(),
            "recording even one run makes the index non-empty"
        );
        assert_eq!(index.total(), 3, "every recorded run is counted once");
        assert_eq!(
            index.runs_in_set(&set),
            3,
            "clean and dirty runs both count toward the set tally"
        );
    }

    #[test]
    fn ordinal_of_passes_small_ranks_through_unchanged() {
        // The ordinal is the storage-key rank narrowed to the point's `u32` width;
        // realistic ranks pass through verbatim so the series tie-break stays in
        // key order.
        assert_eq!(ordinal_of(0), 0);
        assert_eq!(ordinal_of(7), 7);
        assert_eq!(
            ordinal_of(usize::try_from(u32::MAX).expect("u32 fits in usize")),
            u32::MAX
        );
    }
}
