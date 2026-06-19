//! The `analyze` command: resolve which stored runs make up each benchmark's
//! history from live git topology, reconstruct the series, and report the notable
//! changes.
//!
//! Unlike a snapshot tool, `analyze` orders a series by *git history* rather than
//! by ingest time (see DESIGN §8.4): it resolves the target ref's first-parent
//! ancestry, splits it at the merge-base with a base branch, and admits dirty
//! (uncommitted-tree) snapshots only on the target side of that split. The pure
//! logic (selection, series reconstruction, finding detection, report rendering)
//! stays sync and Miri-safe; only the git queries and object loads touch async
//! ports. [`execute`] wires the real adapters; [`analyze_with`] is the
//! storage- and git-generic orchestrator the in-memory tests drive.

mod discriminant;
mod findings;
mod report;
mod selection;
mod series;

use std::collections::HashMap;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use crate::comparability::{EngineSystem, sanitize_segment};
use crate::config::{Config, load_config};
use crate::git_history::{GitHistory, SystemGitHistory};
use crate::model::ResultSet;
use crate::report::{Reporter, StderrReporter};
use crate::storage::{Storage, build_storage};
use crate::text::count_noun;
use crate::wiring::{default_config_path, resolve_project_id};
use crate::{AnalyzeOptions, RunError, RunOutcome};

use discriminant::{DiscriminantSet, Facets, ParsedKey, parse_key};
use findings::{RegressionConfig, find_changes};
use report::{ReportFormat, ReportInput, SetSummary, render};
use selection::select_commits;
use series::{LoadedObject, SeriesFilter, build_series};

/// The real `analyze`: load configuration, wire the configured storage and git
/// history, and orchestrate.
pub(crate) async fn execute(options: &AnalyzeOptions) -> Result<RunOutcome, RunError> {
    let reporter = StderrReporter::new(options.verbose);

    let config_path = options
        .config_path
        .clone()
        .unwrap_or_else(default_config_path);
    reporter.note(&format!(
        "loading configuration from {}",
        config_path.display()
    ));
    let config = load_config(&config_path).await?;

    let workspace_dir = std::env::current_dir().map_err(RunError::Io)?;
    let project_id = resolve_project_id(&config, &workspace_dir);
    let storage = build_storage(&config)?;

    let repo = options.repo.clone().unwrap_or(workspace_dir);
    let git = SystemGitHistory::new(repo);

    analyze_with(&git, &storage, &project_id, &config, options, &reporter).await
}

/// Storage- and git-generic `analyze`: facet-filter the stored objects, resolve
/// the git topology, select the comparable commits, build the series, detect
/// changes, and render a report for the requested format.
pub(crate) async fn analyze_with<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    options: &AnalyzeOptions,
    reporter: &dyn Reporter,
) -> Result<RunOutcome, RunError>
where
    G: GitHistory,
    S: Storage,
{
    let format = parse_format(options.format.as_deref())?;
    let since = parse_since(options.since.as_deref())?;
    let engine = parse_engine(options.engine.as_deref())?;

    // The listing prefix must use the same sanitized project segment that
    // `ComparabilityKey` writes its storage keys under. A project id containing a
    // character that sanitizes (a space, `/`, a non-ASCII letter, ...) is stored
    // mangled, so listing under the raw id would silently find an empty history.
    let project = sanitize_segment(project_id);
    let prefix = match engine {
        Some(engine) => format!("v2/{project}/{}/", engine.as_str()),
        None => format!("v2/{project}/"),
    };

    let facets = Facets {
        engine: engine.map(EngineSystem::as_str),
        os: options.os.as_deref(),
        architecture: options.architecture.as_deref(),
        machine_key: options.machine_key.as_deref(),
    };

    reporter.note(&format!(
        "project id: {project_id} (storage segment: {project})"
    ));
    reporter.note(&format!("listing stored objects under prefix {prefix}"));
    if reporter.enabled() {
        reporter.note(&format!("facet filters: {}", describe_facets(&facets)));
    }

    let keys = storage.list(&prefix).await.map_err(RunError::Storage)?;
    reporter.note(&format!(
        "storage returned {}",
        count_noun(keys.len(), "object key")
    ));

    // Parse and facet-filter the candidate keys up front so both the discriminant
    // listing and the analysis work from the same selected set.
    let mut candidates: Vec<(String, ParsedKey)> = Vec::new();
    for key in keys {
        if !key.ends_with(".json") {
            reporter.note(&format!("skipping {key}: not a .json object"));
            continue;
        }
        let Some(parsed) = parse_key(&key) else {
            reporter.note(&format!("skipping {key}: not a recognized v2 storage key"));
            continue;
        };
        if !parsed.set.matches(&facets) {
            reporter.note(&format!(
                "skipping {key}: discriminant {} does not match the facet filters",
                parsed.set
            ));
            continue;
        }
        candidates.push((key, parsed));
    }
    reporter.note(&format!(
        "{} match the facet filters",
        count_noun(candidates.len(), "object")
    ));

    if options.list_discriminants {
        let mut sets: Vec<DiscriminantSet> = candidates
            .into_iter()
            .map(|(_, parsed)| parsed.set)
            .collect();
        sets.sort();
        sets.dedup();
        return Ok(RunOutcome::Completed {
            message: render_discriminants(&sets, format),
        });
    }

    // Analysis requires a repository: the timeline is resolved from git topology,
    // not from stored timestamps. An unresolvable target ref means there is no
    // repository here (or the branch does not exist), which is an error rather than
    // an empty success.
    let target_ref = options.branch.as_deref().unwrap_or("HEAD");
    let Some(target_sha) = git.resolve(target_ref).await.map_err(RunError::Io)? else {
        return Err(RunError::Analyze {
            message: format!(
                "analyze requires a git repository: could not resolve {target_ref:?}. \
                 Run inside a repository (or pass --repo / --branch)."
            ),
        });
    };

    let base_sha = resolve_base_ref(git, config, options).await?;
    let ancestry = git.first_parent(&target_sha).await.map_err(RunError::Io)?;
    let merge_base = match &base_sha {
        Some(base) => git
            .merge_base(&target_sha, base)
            .await
            .map_err(RunError::Io)?,
        None => None,
    };

    reporter.note(&format!(
        "target ref {target_ref} resolves to {target_sha}; {} on its first-parent line",
        count_noun(ancestry.len(), "commit")
    ));
    reporter.note(&format!(
        "base ref resolves to {}; merge-base with target is {}",
        base_sha.as_deref().unwrap_or("<none>"),
        merge_base.as_deref().unwrap_or("<none>")
    ));

    // When the target tip is base-side (an official / on-the-base-branch view) but
    // the working tree is currently dirty, the dirty snapshots stored on the tip
    // are the user's in-flight work rather than stale leftovers, so they are
    // admitted (and the user warned). `--no-dirty` skips both the probe and the
    // exception.
    let working_tree_dirty = if options.no_dirty {
        false
    } else {
        git.is_dirty().await.map_err(RunError::Io)?
    };
    if working_tree_dirty {
        reporter.note("working tree is dirty: dirty snapshots on a base-side tip will be admitted");
    }

    let selection = select_commits(
        &ancestry,
        merge_base.as_deref(),
        !options.no_dirty,
        working_tree_dirty,
    );
    // First-parent position of each selected commit; an object whose commit is not
    // here is outside the analyzed history and contributes nothing.
    let order: HashMap<String, usize> = selection
        .iter()
        .enumerate()
        .map(|(index, selected)| (selected.commit.clone(), index))
        .collect();
    let admit_dirty: HashMap<&str, bool> = selection
        .iter()
        .map(|selected| (selected.commit.as_str(), selected.admit_dirty))
        .collect();
    // Commits whose dirty runs are admitted only by the base-branch dirty-tree
    // exception, so including one triggers the ephemeral-data warning.
    let dirty_base_exception: HashMap<&str, bool> = selection
        .iter()
        .map(|selected| (selected.commit.as_str(), selected.dirty_base_exception))
        .collect();

    // Tally why candidates do not enter the analysis, so a `0 runs` outcome can
    // explain itself (via `--verbose` per object, and via a summary hint when
    // candidates existed but none were admitted).
    let candidate_count = candidates.len();
    let mut excluded_outside_history = 0_usize;
    let mut excluded_dirty_base = 0_usize;
    let mut excluded_since = 0_usize;
    // Whether at least one dirty run was admitted solely by the base-branch
    // dirty-tree exception, so the report can warn that it is ephemeral.
    let mut included_dirty_base_exception = false;

    // Load each in-selection object, admitting a dirty snapshot only on a commit
    // whose side of the merge-base allows it, then apply the `--since` window.
    let mut loaded: Vec<LoadedObject> = Vec::new();
    for (key, parsed) in candidates {
        if !order.contains_key(&parsed.commit) {
            excluded_outside_history = excluded_outside_history.saturating_add(1);
            reporter.note(&format!(
                "excluding {key}: commit {} is not on {target_ref}'s analyzed history",
                parsed.commit
            ));
            continue;
        }
        if parsed.is_dirty()
            && !admit_dirty
                .get(parsed.commit.as_str())
                .copied()
                .unwrap_or(false)
        {
            excluded_dirty_base = excluded_dirty_base.saturating_add(1);
            reporter.note(&format!(
                "excluding {key}: dirty snapshot on a base-side commit ({} \
                 only admits clean runs); dirty runs count only on the target side",
                parsed.commit
            ));
            continue;
        }
        let bytes = storage.get(&key).await.map_err(RunError::Storage)?;
        let text = String::from_utf8(bytes).map_err(|error| RunError::Analyze {
            message: format!("stored object {key} is not valid UTF-8: {error}"),
        })?;
        let result = ResultSet::from_json(&text).map_err(|error| RunError::Analyze {
            message: format!("stored object {key} is not a valid result set: {error}"),
        })?;
        if since.is_some_and(|since| result.context.timestamps.effective < since) {
            excluded_since = excluded_since.saturating_add(1);
            reporter.note(&format!(
                "excluding {key}: effective time is before the --since cutoff"
            ));
            continue;
        }
        if parsed.is_dirty()
            && dirty_base_exception
                .get(parsed.commit.as_str())
                .copied()
                .unwrap_or(false)
        {
            included_dirty_base_exception = true;
            reporter.note(&format!(
                "including {key}: dirty snapshot on the base-branch tip, admitted \
                 because the working tree is dirty (ephemeral — see the warning)"
            ));
        } else {
            reporter.note(&format!("including {key}"));
        }
        loaded.push(LoadedObject {
            key: parsed,
            object_key: key,
            result,
        });
    }
    reporter.note(&format!(
        "{} entered the analysis ({excluded_outside_history} outside history, \
         {excluded_dirty_base} dirty-on-base, {excluded_since} before --since)",
        count_noun(loaded.len(), "object")
    ));

    let filter = SeriesFilter {
        metric: options.metric.as_deref(),
    };
    let series = build_series(&loaded, &order, &filter);
    let findings = find_changes(&series, &RegressionConfig::default());
    let regressions = findings
        .iter()
        .filter(|finding| finding.is_regression())
        .count();

    // Break the report down by comparable set so each partition reads on its own.
    let mut sets: Vec<DiscriminantSet> = series.iter().map(|one| one.set.clone()).collect();
    sets.sort();
    sets.dedup();
    let summaries: Vec<SetSummary<'_>> = sets
        .iter()
        .map(|set| SetSummary {
            set,
            runs: loaded
                .iter()
                .filter(|object| &object.key.set == set)
                .count(),
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
        loaded.is_empty(),
        candidate_count,
        target_ref,
        ExclusionTally {
            outside_history: excluded_outside_history,
            dirty_base: excluded_dirty_base,
            since: excluded_since,
        },
    );

    // Admitting a dirty snapshot on the base branch's tip is a courtesy for the
    // "evaluating the tool" / "accidentally working on the base branch" cases; warn
    // that such data is not persisted across commits.
    let warning = included_dirty_base_exception.then(|| {
        "Warning: analysis included dirty runs (with uncommitted changes) on top of the \
         base branch. These may be excluded from future analysis. Switch to a new branch \
         to persist benchmark history of your changes."
            .to_owned()
    });

    let input = ReportInput {
        project: project_id,
        runs: loaded.len(),
        series: series.len(),
        findings: &findings,
        sets: &summaries,
        hint: hint.as_deref(),
        warning: warning.as_deref(),
    };
    let report = render(&input, format);

    Ok(RunOutcome::Analyzed {
        report,
        regressions,
        fail_on_regression: options.fail_on_regression,
    })
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
    options: &AnalyzeOptions,
) -> Result<Option<String>, RunError> {
    if let Some(base) = options.base.as_deref() {
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

/// Renders the distinct discriminant sets present in storage for `--list-discriminants`.
fn render_discriminants(sets: &[DiscriminantSet], format: ReportFormat) -> String {
    match format {
        ReportFormat::Json => {
            #[derive(serde::Serialize)]
            struct JsonDiscriminant<'a> {
                engine: &'a str,
                target_triple: &'a str,
                os: &'a str,
                architecture: &'a str,
                machine: &'a str,
            }
            let list: Vec<JsonDiscriminant<'_>> = sets
                .iter()
                .map(|set| JsonDiscriminant {
                    engine: &set.engine,
                    target_triple: &set.target_triple,
                    os: set.os(),
                    architecture: set.architecture(),
                    machine: &set.machine,
                })
                .collect();
            serde_json::to_string_pretty(&list).expect("discriminant list serializes to JSON")
        }
        ReportFormat::Markdown => {
            let mut lines = vec!["# Discriminant sets".to_owned(), String::new()];
            if sets.is_empty() {
                lines.push("No discriminant sets found.".to_owned());
                return format!("{}\n", lines.join("\n"));
            }
            lines.push("| Engine | OS | Architecture | Machine | Target triple |".to_owned());
            lines.push("| --- | --- | --- | --- | --- |".to_owned());
            for set in sets {
                lines.push(format!(
                    "| {} | {} | {} | {} | {} |",
                    set.engine,
                    set.os(),
                    set.architecture(),
                    set.machine,
                    set.target_triple
                ));
            }
            format!("{}\n", lines.join("\n"))
        }
        ReportFormat::Text => {
            if sets.is_empty() {
                return "No discriminant sets found.\n".to_owned();
            }
            let mut lines = vec!["Discriminant sets:".to_owned()];
            for set in sets {
                lines.push(format!(
                    "  - {set} (os={} arch={} machine={})",
                    set.os(),
                    set.architecture(),
                    set.machine
                ));
            }
            format!("{}\n", lines.join("\n"))
        }
    }
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
fn describe_facets(facets: &Facets<'_>) -> String {
    let mut parts = Vec::new();
    if let Some(engine) = facets.engine {
        parts.push(format!("engine={engine}"));
    }
    if let Some(os) = facets.os {
        parts.push(format!("os={os}"));
    }
    if let Some(architecture) = facets.architecture {
        parts.push(format!("architecture={architecture}"));
    }
    if let Some(machine_key) = facets.machine_key {
        parts.push(format!("machine={machine_key}"));
    }
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
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
             or analyze a feature branch with --branch.",
            count_noun(tally.dirty_base, "dirty (uncommitted-tree) snapshot")
        ));
    }
    if tally.outside_history > 0 {
        lines.push(format!(
            "  - {} on commits outside {target_ref}'s analyzed history — check out the \
             branch they were recorded on, or pass --branch.",
            count_noun(tally.outside_history, "run")
        ));
    }
    if tally.since > 0 {
        lines.push(format!(
            "  - {} older than the --since cutoff.",
            count_noun(tally.since, "run")
        ));
    }
    lines.push("Re-run with --verbose for a per-object explanation.".to_owned());
    Some(lines.join("\n"))
}

/// Parses the `--engine` facet into an [`EngineSystem`], if set.
fn parse_engine(name: Option<&str>) -> Result<Option<EngineSystem>, RunError> {
    match name {
        None => Ok(None),
        Some(name) => EngineSystem::from_name(name)
            .map(Some)
            .ok_or_else(|| RunError::Analyze {
                message: format!("unknown engine {name:?}; expected criterion or callgrind"),
            }),
    }
}

/// Parses the `--since` option as an RFC 3339 timestamp or a bare `YYYY-MM-DD`
/// date (interpreted at UTC midnight), if set.
fn parse_since(value: Option<&str>) -> Result<Option<Timestamp>, RunError> {
    let Some(value) = value else {
        return Ok(None);
    };
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
    Err(RunError::Analyze {
        message: format!(
            "invalid --since value {value:?}; expected an RFC 3339 timestamp or a YYYY-MM-DD date"
        ),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use futures::executor::block_on;
    use jiff::Timestamp;

    use crate::config::{Config, parse_config};
    use crate::context::{CiInfo, GitInfo, RunContext, Timestamps, ToolchainInfo};
    use crate::git_history::FakeGitHistory;
    use crate::model::{BenchmarkId, Metric, MetricKind, ResultRecord};
    use crate::report::RecordingReporter;
    use crate::storage::{MemoryStorage, Storage};

    use super::*;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).expect("seconds within range")
    }

    /// A minimal configuration; `analyze_with` only reads `project.default_branch`.
    fn config() -> Config {
        parse_config("[storage.local]\npath = \"./data\"\n").expect("config parses")
    }

    /// Builds a stored result set carrying one record with one `Ir` metric.
    fn ir_set(effective: i64, commit: &str, value: f64) -> ResultSet {
        let time = ts(effective);
        let context = RunContext::new(
            Timestamps::new(time, time, time),
            GitInfo {
                commit: Some(commit.to_owned()),
                short_commit: Some(commit.to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            CiInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        let record = ResultRecord::new(
            BenchmarkId::new(
                Some("nm".to_owned()),
                "nm::observe".to_owned(),
                Some("pull".to_owned()),
                None,
            ),
            vec![Metric::new(
                "Ir".to_owned(),
                MetricKind::InstructionCount,
                value,
                Some("count".to_owned()),
            )],
        );
        ResultSet::new(context, vec![record])
    }

    /// The clean object key for `commit` in the callgrind/linux partition.
    fn clean_key(commit: &str) -> String {
        format!("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json")
    }

    /// The clean object key for `commit` in an arbitrary engine/triple/machine partition.
    fn clean_key_in(engine: &str, triple: &str, machine: &str, commit: &str) -> String {
        format!("v2/folo/{engine}/{triple}/{machine}/{commit}/clean.json")
    }

    /// A stored result set whose single record carries two metrics (`Ir` and
    /// `EstimatedCycles`), so its partition reconstructs two distinct series.
    fn two_metric_set(effective: i64, commit: &str, ir: f64, cycles: f64) -> ResultSet {
        let time = ts(effective);
        let context = RunContext::new(
            Timestamps::new(time, time, time),
            GitInfo {
                commit: Some(commit.to_owned()),
                short_commit: Some(commit.to_owned()),
                branch: Some("main".to_owned()),
                dirty: false,
            },
            CiInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        let record = ResultRecord::new(
            BenchmarkId::new(
                Some("nm".to_owned()),
                "nm::observe".to_owned(),
                Some("pull".to_owned()),
                None,
            ),
            vec![
                Metric::new(
                    "Ir".to_owned(),
                    MetricKind::InstructionCount,
                    ir,
                    Some("count".to_owned()),
                ),
                Metric::new(
                    "EstimatedCycles".to_owned(),
                    MetricKind::EstimatedCycles,
                    cycles,
                    Some("count".to_owned()),
                ),
            ],
        );
        ResultSet::new(context, vec![record])
    }

    /// A dirty snapshot key for `commit` taken at `unix`.
    fn dirty_key(commit: &str, unix: i64) -> String {
        format!("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/dirty-{unix}.json")
    }

    /// Stores a value at `key` in `storage`, panicking on failure (test helper).
    fn store(storage: &MemoryStorage, key: &str, set: &ResultSet) {
        let json = set.to_json().expect("result set serializes");
        block_on(storage.put(key, json.as_bytes())).expect("store succeeds");
    }

    /// A linear master history `c0 - c1 - c2 - c3`, HEAD at the tip.
    fn linear_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
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

    fn options() -> AnalyzeOptions {
        AnalyzeOptions::default()
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
            &reporter,
        ))
        .expect("analysis runs");
        match outcome {
            RunOutcome::Analyzed {
                report,
                regressions,
                ..
            } => (report, regressions),
            RunOutcome::Completed { message } => (message, 0),
        }
    }

    /// Seeds a clean linear regression history (`c0..c3` = 100,100,100,130).
    fn seed_linear_regression(storage: &MemoryStorage) {
        for (index, value) in [100.0, 100.0, 100.0, 130.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
    }

    #[test]
    fn analyze_without_a_repository_is_an_error() {
        let storage = MemoryStorage::new();
        seed_linear_regression(&storage);
        let git = FakeGitHistory::new(); // No commits: HEAD does not resolve.
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &options(),
            &RecordingReporter::new(),
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
        seed_linear_regression(&storage);
        let git = linear_git();
        let (report, regressions) = analyze(&git, &storage, "folo", &options());
        assert_eq!(regressions, 1);
        assert!(report.contains("regression"), "{report}");
        assert!(report.contains("nm::observe/pull/Ir"), "{report}");
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

        let parsed: serde_json::Value = serde_json::from_str(&report).expect("report is JSON");
        let sets = parsed["sets"].as_array().expect("sets array present");

        let set_a = sets
            .iter()
            .find(|set| set["target_triple"] == "x86_64-unknown-linux-gnu")
            .expect("linux set present");
        assert_eq!(set_a["runs"], 3, "{report}");
        assert_eq!(set_a["series"], 2, "{report}");

        let set_b = sets
            .iter()
            .find(|set| set["target_triple"] == "aarch64-apple-darwin")
            .expect("darwin set present");
        assert_eq!(set_b["runs"], 2, "{report}");
        assert_eq!(set_b["series"], 1, "{report}");
    }

    #[test]
    fn series_order_follows_topology_not_effective_time() {
        // Topology is c0,c1,c2,c3 but the effective times are reversed; topology
        // must win, so the regression (the c3 value) is detected as the latest.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0, 130.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            // Reverse the clock: c0 is newest, c3 is oldest by effective time.
            let second = 100 - i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        let git = linear_git();
        let (_, regressions) = analyze(&git, &storage, "folo", &options());
        assert_eq!(regressions, 1, "c3 must be the latest by topology");
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
        // feature branched at c1; a dirty snapshot on f2 (target side) is admitted.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 100.0));
        store(&storage, &dirty_key("f2", 3), &ir_set(3, "f2", 130.0));
        let git = feature_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, regressions) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 4, "the dirty f2 snapshot is admitted");
        assert_eq!(regressions, 1, "the dirty f2 value is the latest point");
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
            &reporter,
        ))
        .expect("analysis runs");
        let RunOutcome::Analyzed { report, .. } = outcome else {
            panic!("expected an analysis outcome");
        };

        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(
            parsed["runs"], 0,
            "every dirty-on-base snapshot is excluded"
        );
        let hint = parsed["hint"]
            .as_str()
            .expect("a diagnostic hint is present");
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
        // admitted, with a warning that they are ephemeral.
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
            &reporter,
        ))
        .expect("analysis runs");
        let RunOutcome::Analyzed {
            report,
            regressions,
            ..
        } = outcome
        else {
            panic!("expected an analysis outcome");
        };

        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 4, "the dirty tip snapshot is admitted");
        assert_eq!(regressions, 1, "the dirty c3 value is the latest point");
        let warning = parsed["warning"]
            .as_str()
            .expect("the ephemeral-data warning is present");
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
            &reporter,
        ))
        .expect("analysis runs");
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
        // From a feature checkout, `--branch master` analyzes master's own history.
        let storage = MemoryStorage::new();
        for (index, value) in [100.0, 100.0, 100.0, 130.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &ir_set(second, &commit, value),
            );
        }
        let git = feature_git();

        let opts = AnalyzeOptions {
            branch: Some("master".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, regressions) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 4, "master's four commits");
        assert_eq!(regressions, 1);
    }

    #[test]
    fn within_a_commit_clean_precedes_dirty() {
        // On a target-side commit, a clean run and a dirty snapshot both load; the
        // clean run is the baseline and the later dirty value is the latest point.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(&storage, &clean_key("c1"), &ir_set(1, "c1", 100.0));
        store(&storage, &clean_key("f1"), &ir_set(2, "f1", 100.0));
        store(&storage, &clean_key("f2"), &ir_set(3, "f2", 100.0));
        store(&storage, &dirty_key("f2", 4), &ir_set(4, "f2", 130.0));
        let git = feature_git();

        let opts = AnalyzeOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let (_, regressions) = analyze(&git, &storage, "folo", &opts);
        assert_eq!(regressions, 1, "the dirty f2 value is the latest point");
    }

    #[test]
    fn list_discriminants_lists_present_sets_without_a_repo() {
        // `--list-discriminants` never requires a repository.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v2/folo/criterion/x86_64-pc-windows-msvc/m1/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = FakeGitHistory::new(); // No repo, but listing does not need one.

        let opts = AnalyzeOptions {
            list_discriminants: true,
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let sets = parsed.as_array().expect("a JSON array of sets");
        assert_eq!(sets.len(), 2, "{report}");
        let engines: Vec<&str> = sets
            .iter()
            .map(|set| set["engine"].as_str().unwrap())
            .collect();
        assert!(engines.contains(&"callgrind"), "{report}");
        assert!(engines.contains(&"criterion"), "{report}");
    }

    #[test]
    fn list_discriminants_text_format_lists_each_set() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        let git = FakeGitHistory::new();
        let opts = AnalyzeOptions {
            list_discriminants: true,
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        assert!(report.contains("Discriminant sets:"), "{report}");
        assert!(report.contains("os=linux"), "{report}");
    }

    #[test]
    fn os_facet_selects_one_set() {
        // Two sets differing only by OS; `--os windows` reports just the one.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v2/folo/callgrind/x86_64-pc-windows-msvc/synthetic/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            os: Some("windows".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["runs"], 1, "only the windows set is loaded");
        assert_eq!(parsed["sets"].as_array().unwrap().len(), 1, "{report}");
    }

    #[test]
    fn two_sets_produce_two_report_sections() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v2/folo/callgrind/x86_64-pc-windows-msvc/synthetic/c0/clean.json",
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
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 100.0));
        store(
            &storage,
            "v2/folo/criterion/x86_64-pc-windows-msvc/m1/c0/clean.json",
            &ir_set(0, "c0", 100.0),
        );
        let git = linear_git();

        let opts = AnalyzeOptions {
            engine: Some("callgrind".to_owned()),
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
    fn history_is_found_for_a_project_id_that_requires_sanitizing() {
        // `run` stores under the sanitized project segment, so `analyze` must list
        // under that same segment; listing under the raw id would miss the history.
        let storage = MemoryStorage::new();
        let raw_project = "my project/v2";
        let sanitized = sanitize_segment(raw_project);
        for (index, value) in [100.0, 100.0, 100.0, 130.0].into_iter().enumerate() {
            let commit = format!("c{index}");
            let second = i64::try_from(index).unwrap();
            let key = format!(
                "v2/{sanitized}/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json"
            );
            store(&storage, &key, &ir_set(second, &commit, value));
        }
        let git = linear_git();

        let (report, regressions) = analyze(&git, &storage, raw_project, &options());
        assert_eq!(
            regressions, 1,
            "history stored under the sanitized key must be found"
        );
        assert!(report.contains("nm::observe/pull/Ir"), "{report}");
    }

    #[test]
    fn fail_on_regression_is_threaded_into_outcome() {
        let storage = MemoryStorage::new();
        seed_linear_regression(&storage);
        let git = linear_git();

        let opts = AnalyzeOptions {
            fail_on_regression: true,
            ..options()
        };
        let outcome = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &RecordingReporter::new(),
        ))
        .expect("analysis runs");
        assert!(!outcome.is_success(), "a gated regression must fail");
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
        let parsed: serde_json::Value = serde_json::from_str(&report).expect("valid JSON");
        assert_eq!(parsed["project"], "folo");
        assert_eq!(parsed["runs"], 1);
    }

    #[test]
    fn non_json_objects_are_skipped() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &ir_set(0, "c0", 10.0));
        // A stray non-result object under the prefix must be ignored, not parsed.
        block_on(storage.put("v2/folo/callgrind/README.txt", b"not json")).unwrap();
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
            &RecordingReporter::new(),
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
            &RecordingReporter::new(),
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
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn unknown_engine_is_rejected() {
        let storage = MemoryStorage::new();
        let git = linear_git();
        let opts = AnalyzeOptions {
            engine: Some("dhat".to_owned()),
            ..options()
        };
        let error = block_on(analyze_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &RecordingReporter::new(),
        ))
        .unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn unresolvable_base_is_rejected() {
        let storage = MemoryStorage::new();
        seed_linear_regression(&storage);
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
            &RecordingReporter::new(),
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
        let config = parse_config(
            "[project]\ndefault_branch = \"master\"\n[storage.local]\npath = \"./d\"\n",
        )
        .unwrap();

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
            &RecordingReporter::new(),
        ))
        .expect("analysis runs");
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
    fn since_rejects_garbage() {
        let error = parse_since(Some("not-a-date")).unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn metric_filter_limits_series() {
        let storage = MemoryStorage::new();
        let mut set = ir_set(0, "c0", 10.0);
        set.results[0].metrics.push(Metric::new(
            "EstimatedCycles".to_owned(),
            MetricKind::EstimatedCycles,
            20.0,
            Some("count".to_owned()),
        ));
        store(&storage, &clean_key("c0"), &set);
        let git = linear_git();

        let opts = AnalyzeOptions {
            metric: Some("Ir".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let (report, _) = analyze(&git, &storage, "folo", &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["series"], 1, "only the Ir metric forms a series");
    }
}
