//! The `list` command: preview the data set a matching `analyze` pass would
//! consume, without running the analysis.
//!
//! `list` accepts exactly the same data-set-selection options as `analyze` (see
//! [`AnalyzeOptions`](crate::AnalyzeOptions) and [`ListOptions`]) and resolves the
//! identical selected runs via the shared [`select_dataset`](super::select_dataset)
//! path, so a `list` invocation is a faithful dry run of the corresponding
//! `analyze`. Instead of detecting changes, it groups the selected runs by
//! comparable discriminant set and reports the run, series, and per-commit counts
//! of each — letting a user inspect the exact data range an analysis would cover.
//!
//! `list --discriminants` lists the distinct discriminant sets present in storage
//! and, like that listing, needs no repository.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::config::{Config, load_config};
use crate::git_history::{GitHistory, SystemGitHistory};
use crate::report::{Reporter, StderrReporter};
use crate::storage::{Storage, build_storage};
use crate::text::count_noun;
use crate::wiring::{default_config_path, resolve_project_id};
use crate::{ListOptions, RunError, RunOutcome};

use super::discriminant::DiscriminantSet;
use super::report::ReportFormat;
use super::series::{LoadedObject, Series, SeriesFilter, build_series};
use super::{
    SelectedDataSet, Selection, dirty_base_exception_warning, empty_history_hint,
    facet_filtered_candidates, parse_format, parsed_facets, select_dataset,
};

/// The real `list`: load configuration, wire the configured storage and git
/// history, and orchestrate.
pub(crate) async fn execute(options: &ListOptions) -> Result<RunOutcome, RunError> {
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

    list_with(&git, &storage, &project_id, &config, options, &reporter).await
}

/// Storage- and git-generic `list`: either list the discriminant sets present in
/// storage (`--discriminants`, no repository required), or resolve the same data
/// set `analyze` would and report its per-set counts.
pub(crate) async fn list_with<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    options: &ListOptions,
    reporter: &dyn Reporter,
) -> Result<RunOutcome, RunError>
where
    G: GitHistory,
    S: Storage,
{
    let format = parse_format(options.format.as_deref())?;
    let selection = Selection::from_list(options);

    if options.discriminants {
        // The discriminant listing is a facet-only view of storage; it never
        // resolves git topology, so it works without a repository.
        let (engine, facets) = parsed_facets(&selection)?;
        let candidates =
            facet_filtered_candidates(storage, project_id, engine, &facets, reporter).await?;
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

    let dataset = select_dataset(git, storage, project_id, config, &selection, reporter).await?;
    let filter = SeriesFilter {
        metric: options.metric.as_deref(),
    };
    let series = build_series(&dataset.loaded, &dataset.order, &filter);
    let listing = build_listing(project_id, &dataset, &series);

    // The same self-explaining diagnostics `analyze` shows: a hint when stored
    // runs matched the facets but none entered the selection, and a warning when a
    // dirty base-branch-tip run was admitted because the working tree is dirty.
    let hint = empty_history_hint(
        dataset.loaded.is_empty(),
        dataset.candidate_count,
        &dataset.target_ref,
        dataset.tally,
    );
    let warning = dataset
        .included_dirty_base_exception
        .then(dirty_base_exception_warning);

    let message = render_listing(&listing, format, hint.as_deref(), warning.as_deref());
    Ok(RunOutcome::Completed { message })
}

/// One commit's contribution to a discriminant set, in first-parent order.
#[derive(Clone, Debug)]
struct CommitEntry {
    /// The commit the runs were measured against (full SHA, or a label in tests).
    commit: String,
    /// Stored runs on this commit in the set.
    runs: usize,
    /// How many of them are clean (committed-tree) runs.
    clean: usize,
    /// How many of them are dirty (uncommitted-tree) snapshots.
    dirty: usize,
}

/// One discriminant set's slice of the listing.
#[derive(Clone, Debug)]
struct SetListing {
    /// The comparable partition this slice covers.
    set: DiscriminantSet,
    /// Stored runs selected for this set.
    runs: usize,
    /// Distinct series the runs reconstruct in this set.
    series: usize,
    /// Per-commit breakdown, oldest first by git topology.
    commits: Vec<CommitEntry>,
}

/// The fully resolved data-set preview, ready to render.
#[derive(Clone, Debug)]
struct Listing {
    /// The project the data set belongs to.
    project: String,
    /// The per-set breakdown, one entry per discriminant set with selected runs.
    sets: Vec<SetListing>,
    /// Total selected runs across every set.
    total_runs: usize,
    /// Total distinct series across every set.
    total_series: usize,
}

/// Groups the selected runs by discriminant set and counts each set's runs,
/// series, and per-commit breakdown (ordered by first-parent topology).
fn build_listing(project_id: &str, dataset: &SelectedDataSet, series: &[Series]) -> Listing {
    let mut sets: Vec<DiscriminantSet> = dataset
        .loaded
        .iter()
        .map(|object| object.key.set.clone())
        .collect();
    sets.sort();
    sets.dedup();

    let set_listings: Vec<SetListing> = sets
        .iter()
        .map(|set| {
            let objects: Vec<&LoadedObject> = dataset
                .loaded
                .iter()
                .filter(|object| &object.key.set == set)
                .collect();
            let series_count = series.iter().filter(|one| &one.set == set).count();

            // Key by first-parent position so commits read oldest-first, matching
            // the order their series points are compared in.
            let mut by_commit: BTreeMap<usize, CommitEntry> = BTreeMap::new();
            for object in &objects {
                let index = dataset
                    .order
                    .get(&object.key.commit)
                    .copied()
                    .unwrap_or(usize::MAX);
                let entry = by_commit.entry(index).or_insert_with(|| CommitEntry {
                    commit: object.key.commit.clone(),
                    runs: 0,
                    clean: 0,
                    dirty: 0,
                });
                entry.runs = entry.runs.saturating_add(1);
                if object.key.is_dirty() {
                    entry.dirty = entry.dirty.saturating_add(1);
                } else {
                    entry.clean = entry.clean.saturating_add(1);
                }
            }

            SetListing {
                set: set.clone(),
                runs: objects.len(),
                series: series_count,
                commits: by_commit.into_values().collect(),
            }
        })
        .collect();

    Listing {
        project: project_id.to_owned(),
        sets: set_listings,
        total_runs: dataset.loaded.len(),
        total_series: series.len(),
    }
}

/// Spells out a series count without the irregular plural `count_noun` would
/// produce (`series` is its own plural, so a trailing `-s` would read `seriess`).
fn series_noun(count: usize) -> String {
    format!("{count} series")
}

/// Renders the data-set preview in the requested format, appending the diagnostic
/// hint and ephemeral-data warning (if any).
fn render_listing(
    listing: &Listing,
    format: ReportFormat,
    hint: Option<&str>,
    warning: Option<&str>,
) -> String {
    match format {
        ReportFormat::Text => render_listing_text(listing, hint, warning),
        ReportFormat::Markdown => render_listing_markdown(listing, hint, warning),
        ReportFormat::Json => render_listing_json(listing, hint, warning),
    }
}

fn render_listing_text(listing: &Listing, hint: Option<&str>, warning: Option<&str>) -> String {
    let mut lines = vec![format!("Data set for project {}", listing.project)];
    if listing.sets.is_empty() {
        lines.push(String::new());
        lines.push("No stored run matches the selection.".to_owned());
    } else {
        for set in &listing.sets {
            lines.push(String::new());
            lines.push(format!(
                "{} (os={} arch={})",
                set.set,
                set.set.os(),
                set.set.architecture()
            ));
            lines.push(format!(
                "  {}, {} across {}",
                count_noun(set.runs, "run"),
                series_noun(set.series),
                count_noun(set.commits.len(), "commit")
            ));
            for commit in &set.commits {
                lines.push(format!(
                    "    {}  {} ({} clean, {} dirty)",
                    commit.commit,
                    count_noun(commit.runs, "run"),
                    commit.clean,
                    commit.dirty
                ));
            }
        }
        lines.push(String::new());
        lines.push(format!(
            "Total: {}, {} across {}",
            count_noun(listing.total_runs, "run"),
            series_noun(listing.total_series),
            count_noun(listing.sets.len(), "discriminant set")
        ));
    }
    append_hint_and_warning(&mut lines, hint, warning);
    format!("{}\n", lines.join("\n"))
}

fn render_listing_markdown(listing: &Listing, hint: Option<&str>, warning: Option<&str>) -> String {
    let mut lines = vec![format!("# Data set for {}", listing.project)];
    if listing.sets.is_empty() {
        lines.push(String::new());
        lines.push("No stored run matches the selection.".to_owned());
    } else {
        for set in &listing.sets {
            lines.push(String::new());
            lines.push(format!(
                "## {} (os={} arch={})",
                set.set,
                set.set.os(),
                set.set.architecture()
            ));
            lines.push(String::new());
            lines.push(format!(
                "{}, {} across {}",
                count_noun(set.runs, "run"),
                series_noun(set.series),
                count_noun(set.commits.len(), "commit")
            ));
            lines.push(String::new());
            lines.push("| Commit | Runs | Clean | Dirty |".to_owned());
            lines.push("| --- | --- | --- | --- |".to_owned());
            for commit in &set.commits {
                lines.push(format!(
                    "| {} | {} | {} | {} |",
                    commit.commit, commit.runs, commit.clean, commit.dirty
                ));
            }
        }
        lines.push(String::new());
        lines.push(format!(
            "**Total:** {}, {} across {}",
            count_noun(listing.total_runs, "run"),
            series_noun(listing.total_series),
            count_noun(listing.sets.len(), "discriminant set")
        ));
    }
    append_hint_and_warning(&mut lines, hint, warning);
    format!("{}\n", lines.join("\n"))
}

fn render_listing_json(listing: &Listing, hint: Option<&str>, warning: Option<&str>) -> String {
    #[derive(Serialize)]
    struct JsonCommit<'a> {
        commit: &'a str,
        runs: usize,
        clean: usize,
        dirty: usize,
    }
    #[derive(Serialize)]
    struct JsonSet<'a> {
        engine: &'a str,
        target_triple: &'a str,
        os: &'a str,
        architecture: &'a str,
        machine: &'a str,
        runs: usize,
        series: usize,
        commits: Vec<JsonCommit<'a>>,
    }
    #[derive(Serialize)]
    struct JsonTotals {
        runs: usize,
        series: usize,
        discriminant_sets: usize,
    }
    #[derive(Serialize)]
    struct JsonListing<'a> {
        project: &'a str,
        sets: Vec<JsonSet<'a>>,
        totals: JsonTotals,
        #[serde(skip_serializing_if = "Option::is_none")]
        hint: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        warning: Option<&'a str>,
    }

    let sets: Vec<JsonSet<'_>> = listing
        .sets
        .iter()
        .map(|set| JsonSet {
            engine: &set.set.engine,
            target_triple: &set.set.target_triple,
            os: set.set.os(),
            architecture: set.set.architecture(),
            machine: &set.set.machine,
            runs: set.runs,
            series: set.series,
            commits: set
                .commits
                .iter()
                .map(|commit| JsonCommit {
                    commit: &commit.commit,
                    runs: commit.runs,
                    clean: commit.clean,
                    dirty: commit.dirty,
                })
                .collect(),
        })
        .collect();

    let document = JsonListing {
        project: &listing.project,
        sets,
        totals: JsonTotals {
            runs: listing.total_runs,
            series: listing.total_series,
            discriminant_sets: listing.sets.len(),
        },
        hint,
        warning,
    };
    serde_json::to_string_pretty(&document).expect("listing structures always serialize to JSON")
}

/// Appends the hint and warning (if any) as trailing, blank-line-separated blocks
/// so they read at the very end of a text or markdown listing.
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

/// Renders the distinct discriminant sets present in storage for `--discriminants`.
fn render_discriminants(sets: &[DiscriminantSet], format: ReportFormat) -> String {
    match format {
        ReportFormat::Json => {
            #[derive(Serialize)]
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use futures::executor::block_on;
    use jiff::Timestamp;

    use crate::config::{Config, parse_config};
    use crate::context::{CiInfo, GitInfo, RunContext, Timestamps, ToolchainInfo};
    use crate::git_history::FakeGitHistory;
    use crate::model::{BenchmarkId, Metric, MetricKind, ResultRecord, ResultSet};
    use crate::report::RecordingReporter;
    use crate::storage::{MemoryStorage, Storage};

    use super::*;

    fn config() -> Config {
        parse_config("[storage.local]\npath = \"./data\"\n").expect("config parses")
    }

    fn options() -> ListOptions {
        ListOptions::default()
    }

    /// A result set with one record carrying two metrics, so its partition
    /// reconstructs two distinct series.
    fn two_metric_set(effective: i64, commit: &str) -> ResultSet {
        let time = Timestamp::from_second(effective).expect("seconds within range");
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
                    100.0,
                    Some("count".to_owned()),
                ),
                Metric::new(
                    "EstimatedCycles".to_owned(),
                    MetricKind::EstimatedCycles,
                    100.0,
                    Some("count".to_owned()),
                ),
            ],
        );
        ResultSet::new(context, vec![record])
    }

    fn clean_key(commit: &str) -> String {
        format!("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json")
    }

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

    /// Drives `list_with` and unwraps the rendered message.
    fn list(storage: &MemoryStorage, git: &FakeGitHistory, options: &ListOptions) -> String {
        let outcome = block_on(list_with(
            git,
            storage,
            "folo",
            &config(),
            options,
            &RecordingReporter::new(),
        ))
        .expect("listing runs");
        match outcome {
            RunOutcome::Completed { message } => message,
            RunOutcome::Analyzed { .. } => panic!("list returns a Completed outcome"),
        }
    }

    #[test]
    fn list_counts_runs_series_and_commits_per_set() {
        let storage = MemoryStorage::new();
        for index in 0..3 {
            let commit = format!("c{index}");
            store(
                &storage,
                &clean_key(&commit),
                &two_metric_set(index, &commit),
            );
        }
        let git = linear_git();

        let opts = ListOptions {
            format: Some("json".to_owned()),
            ..options()
        };
        let report = list(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();

        assert_eq!(parsed["totals"]["runs"], 3);
        assert_eq!(parsed["totals"]["series"], 2, "two metrics -> two series");
        assert_eq!(parsed["totals"]["discriminant_sets"], 1);

        let sets = parsed["sets"].as_array().unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0]["runs"], 3);
        assert_eq!(sets[0]["series"], 2);
        assert_eq!(sets[0]["engine"], "callgrind");

        let commits = sets[0]["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 3, "three distinct commits");
        // Oldest-first by topology.
        assert_eq!(commits[0]["commit"], "c0");
        assert_eq!(commits[2]["commit"], "c2");
        assert_eq!(commits[0]["clean"], 1);
        assert_eq!(commits[0]["dirty"], 0);
    }

    #[test]
    fn list_text_format_summarizes_each_set() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &two_metric_set(0, "c0"));
        let git = linear_git();

        let report = list(&storage, &git, &options());
        assert!(report.contains("Data set for project folo"), "{report}");
        assert!(
            report.contains("callgrind/x86_64-unknown-linux-gnu/synthetic"),
            "{report}"
        );
        // "series" must not be pluralized into "seriess".
        assert!(report.contains("2 series"), "{report}");
        assert!(!report.contains("seriess"), "{report}");
        assert!(report.contains("1 discriminant set"), "{report}");
    }

    #[test]
    fn list_markdown_format_renders_a_per_set_table() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &two_metric_set(0, "c0"));
        let git = linear_git();

        let opts = ListOptions {
            format: Some("markdown".to_owned()),
            ..options()
        };
        let report = list(&storage, &git, &opts);
        assert!(report.contains("# Data set for folo"), "{report}");
        assert!(
            report.contains("| Commit | Runs | Clean | Dirty |"),
            "{report}"
        );
        assert!(report.contains("**Total:**"), "{report}");
    }

    #[test]
    fn list_requires_a_repository() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &two_metric_set(0, "c0"));
        let git = FakeGitHistory::new(); // No commits: HEAD does not resolve.
        let error = block_on(list_with(
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
    fn list_engine_facet_restricts_the_data_set() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &two_metric_set(0, "c0"));
        store(
            &storage,
            "v2/folo/criterion/x86_64-unknown-linux-gnu/m1/c0/clean.json",
            &two_metric_set(0, "c0"),
        );
        let git = linear_git();

        let opts = ListOptions {
            engine: Some("callgrind".to_owned()),
            format: Some("json".to_owned()),
            ..options()
        };
        let report = list(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["totals"]["discriminant_sets"], 1, "{report}");
        assert_eq!(parsed["sets"][0]["engine"], "callgrind");
    }

    #[test]
    fn list_empty_selection_reports_no_match() {
        let storage = MemoryStorage::new();
        let git = linear_git();
        let report = list(&storage, &git, &options());
        assert!(
            report.contains("No stored run matches the selection."),
            "{report}"
        );
    }

    #[test]
    fn list_discriminants_lists_present_sets_without_a_repo() {
        // `--discriminants` never requires a repository.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &two_metric_set(0, "c0"));
        store(
            &storage,
            "v2/folo/criterion/x86_64-pc-windows-msvc/m1/c0/clean.json",
            &two_metric_set(0, "c0"),
        );
        let git = FakeGitHistory::new(); // No repo, but listing does not need one.

        let opts = ListOptions {
            discriminants: true,
            format: Some("json".to_owned()),
            ..options()
        };
        let report = list(&storage, &git, &opts);
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
        store(&storage, &clean_key("c0"), &two_metric_set(0, "c0"));
        let git = FakeGitHistory::new();
        let opts = ListOptions {
            discriminants: true,
            ..options()
        };
        let report = list(&storage, &git, &opts);
        assert!(report.contains("Discriminant sets:"), "{report}");
        assert!(report.contains("os=linux"), "{report}");
    }
}
