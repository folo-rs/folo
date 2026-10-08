//! Shared in-process fixtures and execution helpers for pipeline policy tests.

use std::num::NonZero;
use std::path::PathBuf;

use anyspawn::Spawner;
use cbh_command::AnalyzeOptions;
use cbh_config::Config;
use cbh_detect::{MAX_BRANCH_BASE_COMMITS, MIN_REGIME, MIN_SERIES_POINTS};
use cbh_diag::{RecordingReporter, Reporter};
use cbh_git::FakeGitHistory;
use cbh_model::{
    BenchmarkId, BenchmarkResult, EnvironmentInfo, GitInfo, Metric, MetricKind, Run, RunContext,
    ToolchainInfo,
};
use cbh_render::AnalysisOutcome;
use cbh_storage::MemoryStorage;
use futures::executor::block_on;
use jiff::Timestamp;
use nonempty::nonempty;
use serde_json::Value;

use crate::pipeline::analyze_with;
use crate::testing::store_run as store;
use crate::{AutoDiscriminants, RenderedReports};

pub(crate) fn ts(seconds: i64) -> Timestamp {
    Timestamp::from_second(seconds).unwrap()
}

/// A minimal configuration without project overrides or analysis exclusions.
pub(crate) fn config() -> Config {
    Config::default()
}

/// Builds a stored result set carrying one record with one `Ir` metric.
pub(crate) fn ir_set(effective: i64, commit: &str, value: f64) -> Run {
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
pub(crate) fn clean_key(commit: &str) -> String {
    format!("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/{commit}/clean.json")
}

/// The clean object key for `commit` in an arbitrary engine/triple/machine-key partition.
pub(crate) fn clean_key_in(engine: &str, triple: &str, machine: &str, commit: &str) -> String {
    format!("v1/folo/objects/{engine}/{triple}/{machine}/{commit}/clean.json")
}

/// A stored result set whose single record carries two metrics (`Ir` and
/// `ConditionalBranches`), so its partition reconstructs two distinct series.
pub(crate) fn two_metric_set(effective: i64, commit: &str, ir: f64, branches: f64) -> Run {
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
pub(crate) fn dirty_key(commit: &str, unix: i64) -> String {
    format!("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/{commit}/dirty-{unix}.json")
}

/// Commits each regime of a seeded step holds: the production `min_regime`
/// gate, the fewest points the change-point detector trusts on either side of
/// the split it locates.
pub(crate) const REGIME_COMMITS: usize = 5;

/// Commits a history-mode fixture holds: two full regimes, which is the
/// production `min_series_points` gate — the shortest series the history
/// detectors evaluate at all.
pub(crate) const HISTORY_COMMITS: usize = 2 * REGIME_COMMITS;

/// Base-side commits a branch-mode fixture holds. Branch mode collapses each
/// base commit's runs to that commit's level and needs `min_series_points` such
/// levels before it will judge the context commit against them, so a branch
/// fixture's base line is as long as a whole history fixture.
pub(crate) const BASE_COMMITS: usize = HISTORY_COMMITS;

/// Commits a selection-only fixture holds. Deliberately below
/// [`HISTORY_COMMITS`]: the tests that use it assert on which runs the selection
/// admits — topology, dirty handling, discriminant filters, `--since` — never on findings.
const SELECTION_COMMITS: usize = 4;

// The fixture sizes above are literals so the seeded shapes read plainly, but each one
// exists to satisfy a production gate. Bind them to the gates here, so moving a gate
// fails the build instead of silently making a fixture vacuous.
const _: () = assert!(
    REGIME_COMMITS == MIN_REGIME,
    "a seeded step must hold a full regime on each side of its split"
);
const _: () = assert!(
    HISTORY_COMMITS == MIN_SERIES_POINTS,
    "a history fixture must be long enough for the detectors to judge it"
);
const _: () = assert!(
    BASE_COMMITS <= MAX_BRANCH_BASE_COMMITS,
    "a branch fixture's whole base line must fit the comparison window"
);
const _: () = assert!(
    SELECTION_COMMITS < MIN_SERIES_POINTS,
    "the selection fixture is deliberately too short to be judged"
);

/// The name of the `index`th commit on a master fixture's line.
pub(crate) fn commit_name(index: usize) -> String {
    format!("c{index}")
}

/// Appends a linear chain of `commits` commits named `c0 … c{commits-1}` to
/// `git`, returning the tip's name.
///
/// Each `cN` carries committer time `ts(N)`, the same `effective`-second
/// convention the seeders use, so the topology-decided `--since` cutoff can be
/// exercised.
pub(crate) fn append_master_chain(git: &mut FakeGitHistory, commits: usize) -> String {
    let mut parent: Option<String> = None;
    for index in 0..commits {
        let commit = commit_name(index);
        git.commit_at(
            &commit,
            parent.as_deref(),
            ts(i64::try_from(index).unwrap()),
        );
        parent = Some(commit);
    }
    parent.expect("a chain fixture always holds at least one commit")
}

/// A linear master history of `commits` commits, HEAD at the tip and `master`
/// advertised as the default branch.
pub(crate) fn master_chain(commits: usize) -> FakeGitHistory {
    let mut git = FakeGitHistory::new();
    let tip = append_master_chain(&mut git, commits);
    git.branch("master", &tip)
        .head("master")
        .mark_default("master");
    git
}

/// A master history of `base_commits` commits with a two-commit feature branch
/// forked off `c{fork}`, HEAD on `feature`:
///
/// ```text
/// master:  c0 - … - c{fork} - … - c{base_commits-1}
///                        \
/// feature:                f1 - f2   (HEAD)
/// ```
pub(crate) fn feature_chain(base_commits: usize, fork: usize) -> FakeGitHistory {
    let mut git = FakeGitHistory::new();
    let master_tip = append_master_chain(&mut git, base_commits);
    let forked_at = i64::try_from(base_commits).unwrap();
    git.commit_at("f1", Some(&commit_name(fork)), ts(forked_at))
        .commit_at("f2", Some("f1"), ts(forked_at.saturating_add(1)))
        .branch("master", &master_tip)
        .branch("feature", "f2")
        .head("feature")
        .mark_default("master");
    git
}

/// A feature branch forked off the tip of a `base_commits`-long master line, so
/// every base commit is an ancestor of the feature tip.
fn feature_off_tip(base_commits: usize) -> FakeGitHistory {
    feature_chain(base_commits, base_commits.saturating_sub(1))
}

/// A short linear master history `c0 - c1 - c2 - c3`, HEAD at the tip.
///
/// Deliberately too short to be judged (see [`SELECTION_COMMITS`]): it serves
/// the tests that assert on which runs the selection admits.
pub(crate) fn linear_git() -> FakeGitHistory {
    master_chain(SELECTION_COMMITS)
}

/// A short master history with a feature branch off `c1`, HEAD on the feature
/// branch. Like [`linear_git`], it serves the selection-only tests.
pub(crate) fn feature_git() -> FakeGitHistory {
    feature_chain(SELECTION_COMMITS, 1)
}

/// A linear master history long enough for the history detectors to reach a
/// verdict ([`HISTORY_COMMITS`] commits), HEAD at the tip.
pub(crate) fn history_git() -> FakeGitHistory {
    master_chain(HISTORY_COMMITS)
}

/// A feature branch off the master tip, over a base line long enough for branch
/// mode to judge the tip against ([`BASE_COMMITS`] commits).
pub(crate) fn branch_git() -> FakeGitHistory {
    feature_off_tip(BASE_COMMITS)
}

/// A feature branch off a master tip that carries no base data.
///
/// Master runs one commit past the [`BASE_COMMITS`] base line the seeders fill,
/// and the merge-base is that unmeasured tip, so a surviving branch finding's
/// comparison base lags the merge-base by exactly one commit.
pub(crate) fn lagging_branch_git() -> FakeGitHistory {
    feature_off_tip(BASE_COMMITS.saturating_add(1))
}

/// A linear master history whose tip carries no clean run: master runs one
/// commit past the [`BASE_COMMITS`] base line the seeders fill, so a fixture can
/// place dirty snapshots on a tip that holds nothing else.
pub(crate) fn unmeasured_tip_git() -> FakeGitHistory {
    master_chain(BASE_COMMITS.saturating_add(1))
}

/// The master commit just past the seeded base line — the tip of both
/// [`lagging_branch_git`] and [`unmeasured_tip_git`].
pub(crate) fn unmeasured_tip() -> String {
    commit_name(BASE_COMMITS)
}

/// The values of a sustained step: [`REGIME_COMMITS`] points at `before`
/// followed by [`REGIME_COMMITS`] at `after` — the shortest series that can hold
/// a change point, and exactly [`HISTORY_COMMITS`] points long.
pub(crate) fn step_values(before: f64, after: f64) -> Vec<f64> {
    [before; REGIME_COMMITS]
        .into_iter()
        .chain([after; REGIME_COMMITS])
        .collect()
}

/// Stores one clean `Ir` run per value under the default partition: `values[N]`
/// on commit `cN`, observed at `ts(N)`.
pub(crate) fn seed_master(storage: &MemoryStorage, values: &[f64]) {
    for (index, &value) in values.iter().enumerate() {
        let commit = commit_name(index);
        let second = i64::try_from(index).unwrap();
        store(
            storage,
            &clean_key(&commit),
            &ir_set(second, &commit, value),
        );
    }
}

/// Seeds a clean linear sustained-step history under the default partition, so
/// the change-point detector flags a single major regression at the split.
pub(crate) fn seed_linear_step(storage: &MemoryStorage) {
    seed_master(storage, &step_values(100.0, 130.0));
}

/// Seeds a flat base line of `base_commits` clean runs (`c0 …`) plus a raised
/// feature regime. Returns the number of runs stored.
pub(crate) fn seed_raised_feature(storage: &MemoryStorage, base_commits: usize) -> usize {
    seed_feature_over(storage, &vec![100.0; base_commits])
}

/// Seeds `base` as the base line (`c0 …`) plus a raised feature regime: clean `f1`
/// and `f2` runs and a dirty `f2` snapshot on top of them. Returns the number of runs
/// stored.
fn seed_feature_over(storage: &MemoryStorage, base: &[f64]) -> usize {
    seed_master(storage, base);
    let observed = i64::try_from(base.len()).unwrap();
    let dirty_at = observed.saturating_add(2);
    store(storage, &clean_key("f1"), &ir_set(observed, "f1", 130.0));
    store(
        storage,
        &clean_key("f2"),
        &ir_set(observed.saturating_add(1), "f2", 130.0),
    );
    store(
        storage,
        &dirty_key("f2", dirty_at),
        &ir_set(dirty_at, "f2", 130.0),
    );
    base.len().saturating_add(3)
}

/// The observation second the extra merge-base run in a lagging-base fixture
/// carries: past every run [`seed_lagging_branch`] stores, so it is
/// unambiguously the newest base observation.
pub(crate) const SIBLING_OBSERVED: i64 = 100;

/// Seeds the PR runner's (`m1`) runs for [`lagging_branch_git`]: the flat base
/// line stops at `c{BASE_COMMITS-1}`, one commit short of the merge-base tip
/// that `m1` never measured, so a surviving branch finding's comparison base
/// lags by one commit.
pub(crate) fn seed_lagging_branch(storage: &MemoryStorage) {
    seed_raised_feature(storage, BASE_COMMITS);
}

pub(crate) fn options() -> AnalyzeOptions {
    AnalyzeOptions::default()
}

/// A fixed clock anchor for the history-mode default `--since` window in unit
/// tests. The seeded data sits at the Unix epoch (`ts(0..)`); anchoring here
/// keeps the default six-month look-back well before it, so the default window
/// never drops a seeded point.
pub(crate) fn now_anchor() -> Timestamp {
    Timestamp::from_second(0).unwrap()
}

/// The auto-detected discriminant values the unit-test data is seeded under
/// (`x86_64-unknown-linux-gnu`, `m1` machine).
pub(crate) fn auto() -> AutoDiscriminants {
    AutoDiscriminants {
        triple: "x86_64-unknown-linux-gnu".to_owned(),
        machine_key: "m1".into(),
    }
}

/// An inline spawner that runs the detection's blocking tasks on the calling
/// thread, so `analyze_with` needs no Tokio runtime under `block_on` or Miri.
pub(crate) fn spawner() -> Spawner {
    cbh_detect::testing::synchronous_spawner()
}

/// Runs `analyze_with` requesting the JSON report, returning the JSON text, the
/// regression count, and the recording reporter so a test can assert on the
/// machine-readable report and the verbose trail together. The text report is
/// suppressed, so the JSON is the only rendered output.
pub(crate) fn analyze_json(
    git: &FakeGitHistory,
    storage: &MemoryStorage,
    project: &str,
    options: &AnalyzeOptions,
) -> (String, usize, RecordingReporter) {
    let reporter = RecordingReporter::new();
    let (report, regressions) =
        analyze_json_with_reporter(git, storage, project, options, &reporter);
    (report, regressions, reporter)
}

/// Runs output assertions without constructing an unused verbose diagnostic trail.
pub(crate) fn analyze_quiet_json(
    git: &FakeGitHistory,
    storage: &MemoryStorage,
    project: &str,
    options: &AnalyzeOptions,
) -> (String, usize) {
    analyze_json_with_reporter(git, storage, project, options, &RecordingReporter::quiet())
}

fn analyze_json_with_reporter(
    git: &FakeGitHistory,
    storage: &MemoryStorage,
    project: &str,
    options: &AnalyzeOptions,
    reporter: &dyn Reporter,
) -> (String, usize) {
    let mut options = options.clone();
    options.no_text = true;
    options.markdown = None;
    options.json = Some(PathBuf::from("report.json"));
    let (rendered, regressions) =
        analyze_reports_with_reporter(git, storage, project, &options, reporter);
    let report = rendered
        .json
        .expect("the JSON report was rendered for the requested path");
    let outcome = rendered.outcome.expect("analysis returns a typed outcome");
    let parsed: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["outcome"], outcome.as_str(), "{report}");
    assert_eq!(
        parsed["notable"],
        outcome == AnalysisOutcome::Findings,
        "{report}"
    );
    (report, regressions)
}

/// Requests both report surfaces from one load and detection pass.
///
/// Warning tests compare presentations of the same analysis, not independent executions.
pub(crate) fn analyze_text_and_json(
    git: &FakeGitHistory,
    storage: &MemoryStorage,
) -> (String, String, usize) {
    let mut options = options();
    options.json = Some(PathBuf::from("report.json"));
    let (rendered, regressions) =
        analyze_reports_with_reporter(git, storage, "folo", &options, &RecordingReporter::quiet());
    (
        rendered.text.expect("the text report was requested"),
        rendered.json.expect("the JSON report was requested"),
        regressions,
    )
}

/// Runs the in-memory pipeline once with the requested output formats.
pub(crate) fn analyze_reports_with_reporter(
    git: &FakeGitHistory,
    storage: &MemoryStorage,
    project: &str,
    options: &AnalyzeOptions,
    reporter: &dyn Reporter,
) -> (RenderedReports, usize) {
    block_on(analyze_with(
        git,
        storage,
        project,
        &config(),
        options,
        &auto(),
        now_anchor(),
        reporter,
        false,
        &spawner(),
        NonZero::<usize>::MIN,
    ))
    .unwrap()
}

/// Asserts that a rendered report reached the history detectors at all: exactly
/// one series survived selection, the report itself states that it judged that
/// series, and it carries at least [`HISTORY_COMMITS`] runs — the shortest series
/// the detectors evaluate.
///
/// A "nothing was flagged" assertion only says something about the gates when the
/// data cleared that bar; without this check the same silence is also what an
/// unanalyzed or ghost-filtered series produces.
pub(crate) fn assert_history_was_judged(parsed: &Value) {
    assert_eq!(parsed["series"], 1, "{parsed}");
    assert_eq!(
        parsed["census"]["judged"], 1,
        "the report must account for the series as judged: {parsed}"
    );
    assert_eq!(
        parsed["census"]["unjudged"], 0,
        "nothing may have been silently dropped: {parsed}"
    );
    let runs = parsed["runs"]
        .as_u64()
        .expect("the report tallies the runs it loaded");
    assert!(
        runs >= u64::try_from(HISTORY_COMMITS).unwrap(),
        "the analyzed series must be long enough to be judged: {parsed}"
    );
}

/// Runs `analyze_with` and unwraps the rendered text report and regression count.
pub(crate) fn analyze(
    git: &FakeGitHistory,
    storage: &MemoryStorage,
    project: &str,
    options: &AnalyzeOptions,
) -> (String, usize) {
    let (rendered, regressions) =
        analyze_reports_with_reporter(git, storage, project, options, &RecordingReporter::quiet());
    (rendered.text.unwrap_or_default(), regressions)
}

/// A stored result set naming several benchmarks, each carrying one `Ir` metric,
/// so one commit's object can present or omit specific benchmarks — the shape a
/// ghost (a benchmark that disappears before the tip) needs.
pub(crate) fn multi_bench(effective: i64, commit: &str, benches: &[(&str, f64)]) -> Run {
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

/// Drives configured exclusions through the same load and report path as real analysis.
pub(crate) fn analyze_configured_json(
    git: &FakeGitHistory,
    storage: &MemoryStorage,
    config: &Config,
    options: &AnalyzeOptions,
) -> (Value, RecordingReporter) {
    let options = AnalyzeOptions {
        no_text: true,
        json: Some("report.json".into()),
        ..options.clone()
    };
    let reporter = RecordingReporter::new();
    let (reports, regressions) = block_on(analyze_with(
        git,
        storage,
        "folo",
        config,
        &options,
        &auto(),
        now_anchor(),
        &reporter,
        false,
        &spawner(),
        NonZero::<usize>::MIN,
    ))
    .unwrap();
    let report: Value = serde_json::from_str(reports.json.as_ref().unwrap()).unwrap();
    assert_eq!(report["regressions"], regressions);
    assert_eq!(report["outcome"], reports.outcome.unwrap().as_str());
    (report, reporter)
}
