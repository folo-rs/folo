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
//! `list discriminants` lists the distinct discriminant sets present in storage
//! and, like that listing, needs no repository.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use crate::config::{Config, load_config};
use crate::git_history::{GitHistory, SystemGitHistory};
use crate::report::{Reporter, ReporterExt, StderrReporter};
use crate::storage::{Storage, build_storage};
use crate::text::count_noun;
use crate::wiring::{
    cache_env, resolve_cache_path, resolve_config_path, resolve_local_path, resolve_project_id,
    resolve_repo, storage_env,
};
use crate::{ListOptions, ListSubject, RunError, RunOutcome};

use anyspawn::Spawner;
use jiff::Timestamp;

use super::{
    AutoFacets, Selection, detect_auto_facets, dirty_base_exception_warning, empty_history_hint,
    facet_filtered_candidates, resolve_facets, select_dataset,
};
use super::{ReportFormat, RunIndex, Series, SeriesFilter, apply_blessings};
use crate::model::BlessingRecord;
use crate::model::DiscriminantSet;
use crate::output::{OutputSelection, OutputWriter, TokioOutputWriter, emit};

/// The real `list`: load configuration, wire the configured storage and git
/// history, and orchestrate.
///
/// `now_override` anchors the history-mode default `--since` lookback to a fixed
/// instant (see [`execute`](super::execute)); production passes `None`.
pub(crate) async fn execute(
    options: &ListOptions,
    workspace_dir: &Path,
    now_override: Option<Timestamp>,
) -> Result<RunOutcome, RunError> {
    let reporter = StderrReporter::new(options.verbose);

    let config_path = resolve_config_path(workspace_dir, options.config_path.as_deref());
    reporter.note(&format!(
        "loading configuration from {}",
        config_path.display()
    ));
    let config = load_config(&config_path, options.config_path.is_some()).await?;

    let project_id = resolve_project_id(&config, workspace_dir);
    let local = resolve_local_path(options.local.as_ref(), storage_env().as_deref())?;
    let cache = resolve_cache_path(options.cache.as_ref(), cache_env().as_deref())?;
    if local.is_some() && cache.is_some() {
        reporter.note(
            "cache: --cache was given, but --local selects filesystem storage whose reads are \
             already local, so the read-through cache is ignored",
        );
    }
    let storage = build_storage(
        local.as_deref(),
        &config,
        workspace_dir,
        cache.as_deref(),
        &project_id,
    )?;
    storage.synchronize_cache(&project_id, &reporter).await?;

    let git = SystemGitHistory::new(resolve_repo(workspace_dir, options.repo.as_deref()));
    let auto = detect_auto_facets().await?;

    let now = now_override.unwrap_or_else(Timestamp::now);
    // The object-load and detection work shares the ambient Tokio worker threads
    // (mirrors `analyze::execute`).
    let spawner = Spawner::new_tokio();
    let writer = TokioOutputWriter::new(workspace_dir.to_path_buf());
    let outcome = list_with(
        &git,
        &storage,
        &project_id,
        &config,
        options,
        &auto,
        now,
        &reporter,
        &writer,
        &spawner,
    )
    .await;
    storage.report_cache_tally(&reporter);
    outcome
}

/// Storage- and git-generic `list`: either list the discriminant sets present in
/// storage (`discriminants`, no repository required), or resolve the same data
/// set `analyze` would and report its per-set counts.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the analyze selection pipeline, which threads the same injected ports"
)]
pub(crate) async fn list_with<G, S, W>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    options: &ListOptions,
    auto: &AutoFacets,
    now: Timestamp,
    reporter: &dyn Reporter,
    writer: &W,
    spawner: &Spawner,
) -> Result<RunOutcome, RunError>
where
    G: GitHistory,
    S: Storage + Clone + 'static,
    W: OutputWriter,
{
    let output = OutputSelection::resolve(
        options.no_text,
        options.markdown.as_deref(),
        options.json.as_deref(),
    )?;
    if options.all && options.subject != ListSubject::Blessings {
        return Err(RunError::Analyze {
            message: "--all applies only to `list blessings`, where it widens the view from \
                      the current commit to the most recent blessing of every benchmark in \
                      the window; it has no meaning for `list runs` or `list discriminants`"
                .to_owned(),
        });
    }
    let selection = Selection::from_list(options);

    match options.subject {
        ListSubject::Discriminants => {
            // The discriminant listing is a facet-only view of storage; it never
            // resolves git topology, so it works without a repository. It is a
            // discovery catalog, so omitted facets default to no filter (every
            // stored partition) rather than the current machine — pass `None` so a
            // user can see machine keys and triples they do not already know.
            let facets = resolve_facets(&selection, None)?;
            let candidates =
                facet_filtered_candidates(storage, project_id, &facets, reporter).await?;
            let mut sets: Vec<DiscriminantSet> = candidates
                .into_iter()
                .map(|(_, parsed)| parsed.set)
                .collect();
            sets.sort();
            sets.dedup();
            let message = emit(&output, writer, reporter, |format| {
                render_discriminants(&sets, format)
            })
            .await?;
            Ok(RunOutcome::Completed { message })
        }
        ListSubject::Blessings => {
            let (head_label, entries) = list_blessings(
                git, storage, project_id, config, options, auto, now, reporter, spawner,
            )
            .await?;
            let message = emit(&output, writer, reporter, |format| {
                render_blessings(project_id, options.all, &head_label, &entries, format)
            })
            .await?;
            Ok(RunOutcome::Completed { message })
        }
        ListSubject::Runs => {
            let filter = SeriesFilter::default();
            let dataset = select_dataset(
                git, storage, project_id, config, &selection, filter, auto, now, reporter, spawner,
            )
            .await?;
            let series = dataset.series;
            let listing = build_listing(project_id, &dataset.run_index, &series);

            // The same self-explaining diagnostics `analyze` shows: a hint when
            // stored runs matched the facets but none entered the selection, and a
            // warning when a dirty base-branch-tip run was admitted because the
            // working tree is dirty.
            let hint = empty_history_hint(
                dataset.run_index.is_empty(),
                dataset.candidate_count,
                &dataset.target_ref,
                dataset.tally,
            );
            let warning = dataset
                .included_dirty_base_exception
                .then(dirty_base_exception_warning);

            let message = emit(&output, writer, reporter, |format| {
                render_listing(&listing, format, hint.as_deref(), warning.as_deref())
            })
            .await?;
            Ok(RunOutcome::Completed { message })
        }
    }
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
fn build_listing(project_id: &str, run_index: &RunIndex, series: &[Series]) -> Listing {
    let set_listings: Vec<SetListing> = run_index
        .sets()
        .map(|(set, by_commit)| {
            let series_count = series.iter().filter(|one| &one.set == set).count();

            // `by_commit` is keyed by first-parent position, so iterating it yields
            // commits oldest-first, matching the order their series points compare.
            let commits: Vec<CommitEntry> = by_commit
                .values()
                .map(|counts| CommitEntry {
                    commit: counts.commit.clone(),
                    runs: counts.clean.saturating_add(counts.dirty),
                    clean: counts.clean,
                    dirty: counts.dirty,
                })
                .collect();
            let runs = commits.iter().map(|entry| entry.runs).sum();

            SetListing {
                set: set.clone(),
                runs,
                series: series_count,
                commits,
            }
        })
        .collect();

    Listing {
        project: project_id.to_owned(),
        sets: set_listings,
        total_runs: run_index.total(),
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
            lines.push(set.set.to_string());
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
            lines.push(format!("## {}", set.set));
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
        machine_key: &'a str,
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
            machine_key: &set.set.machine_key,
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

/// Renders the distinct discriminant sets present in storage for the
/// `discriminants` subject.
fn render_discriminants(sets: &[DiscriminantSet], format: ReportFormat) -> String {
    match format {
        ReportFormat::Json => {
            #[derive(Serialize)]
            struct JsonDiscriminant<'a> {
                engine: &'a str,
                target_triple: &'a str,
                machine_key: &'a str,
            }
            let list: Vec<JsonDiscriminant<'_>> = sets
                .iter()
                .map(|set| JsonDiscriminant {
                    engine: &set.engine,
                    target_triple: &set.target_triple,
                    machine_key: &set.machine_key,
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
            lines.push("| Engine | Target triple | Machine key |".to_owned());
            lines.push("| --- | --- | --- |".to_owned());
            for set in sets {
                lines.push(format!(
                    "| {} | {} | {} |",
                    set.engine, set.target_triple, set.machine_key
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
                lines.push(format!("  - {set}"));
            }
            format!("{}\n", lines.join("\n"))
        }
    }
}

/// One blessing row in a `list blessings` report.
#[derive(Clone, Debug)]
struct BlessingEntry {
    /// The comparable partition the blessing lives in.
    set: DiscriminantSet,
    /// The benchmark this row describes; `Some` only in `--all` mode (the HEAD
    /// view reports each sidecar once, covering whichever benchmarks its prefixes
    /// match).
    benchmark: Option<String>,
    /// Abbreviated commit the blessing was issued at (the re-baseline point).
    commit: String,
    /// Committer date of the blessed commit, resolved from git topology; `None`
    /// when git did not report one.
    commit_time: Option<Timestamp>,
    /// When the blessing was issued; `Some` only in the HEAD view.
    issued_at: Option<Timestamp>,
    /// Accepted benchmark-id prefixes; populated only in the HEAD view.
    prefixes: Vec<String>,
}

/// Lists blessings for `list blessings`.
///
/// Default: every blessing recorded at the current commit (HEAD) in the
/// facet-selected sets — the sidecars a fresh `unbless` would remove. `--all`: the
/// most recent blessing of every benchmark across the analysis window `analyze`
/// would resolve, so a user can audit which benchmarks are currently re-baselined.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the analyze selection pipeline, which threads the same injected ports"
)]
async fn list_blessings<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    options: &ListOptions,
    auto: &AutoFacets,
    now: Timestamp,
    reporter: &dyn Reporter,
    spawner: &Spawner,
) -> Result<(String, Vec<BlessingEntry>), RunError>
where
    G: GitHistory,
    S: Storage + Clone + 'static,
{
    let selection = Selection::from_list(options);

    let (head_label, mut entries) = if options.all {
        blessings_across_window(
            git, storage, project_id, config, &selection, auto, now, reporter, spawner,
        )
        .await?
    } else {
        blessings_at_head(git, storage, project_id, &selection, auto, reporter).await?
    };
    entries.sort_by(|left, right| {
        left.set
            .cmp(&right.set)
            .then_with(|| left.benchmark.cmp(&right.benchmark))
            .then_with(|| left.commit.cmp(&right.commit))
    });

    Ok((head_label, entries))
}

/// Collects the blessings recorded at the current commit (HEAD) in the
/// facet-selected sets, returning the abbreviated HEAD label and the rows.
async fn blessings_at_head<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    selection: &Selection<'_>,
    auto: &AutoFacets,
    reporter: &dyn Reporter,
) -> Result<(String, Vec<BlessingEntry>), RunError>
where
    G: GitHistory,
    S: Storage,
{
    let head = git
        .resolve("HEAD")
        .await
        .map_err(RunError::Io)?
        .ok_or_else(|| RunError::Analyze {
            message: "this command requires a git repository: could not resolve HEAD. \
                      Run inside a repository (or pass --repo)."
                .to_owned(),
        })?;
    let facets = resolve_facets(selection, Some(auto))?;
    let candidates = facet_filtered_candidates(storage, project_id, &facets, reporter).await?;

    // The blessed commit is HEAD; its committer date comes from git topology, so
    // the sidecar itself need not carry a denormalized copy. A single-commit read
    // dates HEAD without walking its first-parent ancestry.
    let head_commit_time = git.committer_time("HEAD").await.map_err(RunError::Io)?;

    let mut entries = Vec::new();
    for (key, parsed) in candidates {
        if !(parsed.is_bless() && parsed.commit == head) {
            continue;
        }
        let bytes = storage.get(&key).await.map_err(RunError::Storage)?;
        let text = String::from_utf8(bytes).map_err(|error| RunError::Analyze {
            message: format!("stored object {key} is not valid UTF-8: {error}"),
        })?;
        let record = BlessingRecord::from_json(&text).map_err(|error| RunError::Analyze {
            message: format!("stored object {key} is not a valid blessing: {error}"),
        })?;
        reporter.note_with(|| format!("blessing {key}"));
        entries.push(BlessingEntry {
            set: parsed.set,
            benchmark: None,
            commit: short_sha(&record.commit).to_owned(),
            commit_time: head_commit_time,
            issued_at: Some(record.issued_at),
            prefixes: record.prefixes.into_iter().map(String::from).collect(),
        });
    }
    Ok((short_sha(&head).to_owned(), entries))
}

/// Collects the most recent blessing of every benchmark across the analysis window
/// (`--all`), resolved through the same selection an `analyze` pass would use.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the analyze selection pipeline, which threads the same injected ports"
)]
async fn blessings_across_window<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    selection: &Selection<'_>,
    auto: &AutoFacets,
    now: Timestamp,
    reporter: &dyn Reporter,
    spawner: &Spawner,
) -> Result<(String, Vec<BlessingEntry>), RunError>
where
    G: GitHistory,
    S: Storage + Clone + 'static,
{
    let filter = SeriesFilter::default();
    let dataset = select_dataset(
        git, storage, project_id, config, selection, filter, auto, now, reporter, spawner,
    )
    .await?;
    let mut series = dataset.series;
    apply_blessings(&mut series, &dataset.blessings);

    // A benchmark's metrics each form their own series but share a blessing, so the
    // same `(set, benchmark, commit)` is reported once.
    let mut seen: BTreeSet<(DiscriminantSet, String, String)> = BTreeSet::new();
    let mut entries = Vec::new();
    for one in &series {
        let Some(blessing) = &one.blessing else {
            continue;
        };
        let benchmark = one.id.qualified();
        if !seen.insert((one.set.clone(), benchmark.clone(), blessing.commit.clone())) {
            continue;
        }
        entries.push(BlessingEntry {
            set: one.set.clone(),
            benchmark: Some(benchmark),
            commit: short_sha(&blessing.commit).to_owned(),
            commit_time: blessing.commit_time,
            issued_at: None,
            prefixes: Vec::new(),
        });
    }
    Ok((String::new(), entries))
}

/// The first twelve characters of a SHA (all of it when shorter), for display.
fn short_sha(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// Renders a blessing listing in the requested format.
fn render_blessings(
    project: &str,
    all: bool,
    head_label: &str,
    entries: &[BlessingEntry],
    format: ReportFormat,
) -> String {
    match format {
        ReportFormat::Text => render_blessings_text(project, all, head_label, entries),
        ReportFormat::Markdown => render_blessings_markdown(project, all, head_label, entries),
        ReportFormat::Json => render_blessings_json(project, all, head_label, entries),
    }
}

/// Groups blessing rows by discriminant set, preserving the entries' (already
/// sorted) order.
fn group_by_set(entries: &[BlessingEntry]) -> Vec<(&DiscriminantSet, Vec<&BlessingEntry>)> {
    let mut groups: Vec<(&DiscriminantSet, Vec<&BlessingEntry>)> = Vec::new();
    for entry in entries {
        match groups.last_mut() {
            Some((set, rows)) if **set == entry.set => rows.push(entry),
            _ => groups.push((&entry.set, vec![entry])),
        }
    }
    groups
}

fn render_blessings_text(
    project: &str,
    all: bool,
    head_label: &str,
    entries: &[BlessingEntry],
) -> String {
    let mut lines = if all {
        vec![format!("Most recent blessings for project {project}")]
    } else {
        vec![format!(
            "Blessings for project {project} at commit {head_label}"
        )]
    };
    if entries.is_empty() {
        lines.push(String::new());
        lines.push(if all {
            "No blessings in the analysis window.".to_owned()
        } else {
            format!("No blessings recorded at commit {head_label}.")
        });
        return format!("{}\n", lines.join("\n"));
    }
    for (set, rows) in group_by_set(entries) {
        lines.push(String::new());
        lines.push(set.to_string());
        for row in rows {
            if let Some(benchmark) = &row.benchmark {
                let at = row
                    .commit_time
                    .map_or_else(String::new, |time| format!(" ({time})"));
                lines.push(format!("  {benchmark}  blessed at {}{at}", row.commit));
            } else {
                let issued = row
                    .issued_at
                    .map_or_else(String::new, |issued| format!(" (issued {issued})"));
                lines.push(format!(
                    "  {} accepts {}{issued}",
                    row.commit,
                    row.prefixes.join(", ")
                ));
            }
        }
    }
    format!("{}\n", lines.join("\n"))
}

fn render_blessings_markdown(
    project: &str,
    all: bool,
    head_label: &str,
    entries: &[BlessingEntry],
) -> String {
    let mut lines = if all {
        vec![format!("# Most recent blessings for {project}")]
    } else {
        vec![format!("# Blessings for {project} at {head_label}")]
    };
    if entries.is_empty() {
        lines.push(String::new());
        lines.push(if all {
            "No blessings in the analysis window.".to_owned()
        } else {
            "No blessings recorded at this commit.".to_owned()
        });
        return format!("{}\n", lines.join("\n"));
    }
    for (set, rows) in group_by_set(entries) {
        lines.push(String::new());
        lines.push(format!("## {set}"));
        lines.push(String::new());
        if all {
            lines.push("| Benchmark | Blessed at | Commit time |".to_owned());
            lines.push("| --- | --- | --- |".to_owned());
            for row in rows {
                let time = row
                    .commit_time
                    .map_or_else(String::new, |time| time.to_string());
                lines.push(format!(
                    "| {} | {} | {} |",
                    row.benchmark.as_deref().unwrap_or(""),
                    row.commit,
                    time
                ));
            }
        } else {
            lines.push("| Commit | Prefixes | Issued |".to_owned());
            lines.push("| --- | --- | --- |".to_owned());
            for row in rows {
                let issued = row
                    .issued_at
                    .map_or_else(String::new, |issued| issued.to_string());
                lines.push(format!(
                    "| {} | {} | {} |",
                    row.commit,
                    row.prefixes.join(", "),
                    issued,
                ));
            }
        }
    }
    format!("{}\n", lines.join("\n"))
}

fn render_blessings_json(
    project: &str,
    all: bool,
    head_label: &str,
    entries: &[BlessingEntry],
) -> String {
    #[derive(Serialize)]
    struct JsonBlessing<'a> {
        engine: &'a str,
        target_triple: &'a str,
        machine_key: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        benchmark: Option<&'a str>,
        commit: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        commit_time: Option<Timestamp>,
        #[serde(skip_serializing_if = "Option::is_none")]
        issued_at: Option<Timestamp>,
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        prefixes: &'a [String],
    }
    #[derive(Serialize)]
    struct JsonDocument<'a> {
        project: &'a str,
        scope: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        commit: Option<&'a str>,
        blessings: Vec<JsonBlessing<'a>>,
    }

    let blessings: Vec<JsonBlessing<'_>> = entries
        .iter()
        .map(|entry| JsonBlessing {
            engine: &entry.set.engine,
            target_triple: &entry.set.target_triple,
            machine_key: &entry.set.machine_key,
            benchmark: entry.benchmark.as_deref(),
            commit: &entry.commit,
            commit_time: entry.commit_time,
            issued_at: entry.issued_at,
            prefixes: &entry.prefixes,
        })
        .collect();

    let document = JsonDocument {
        project,
        scope: if all { "window" } else { "head" },
        commit: (!all).then_some(head_label),
        blessings,
    };
    serde_json::to_string_pretty(&document).expect("blessing list serializes to JSON")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
    use futures::executor::block_on;
    use jiff::Timestamp;

    use crate::config::Config;
    use crate::git_history::FakeGitHistory;
    use crate::model::{BenchmarkId, BenchmarkIdPrefix, BenchmarkResult, Metric, MetricKind, Run};
    use crate::model::{EnvironmentInfo, GitInfo, RunContext, ToolchainInfo};
    use crate::report::RecordingReporter;
    use crate::storage::{MemoryStorage, Storage};

    use crate::output::MemoryOutputWriter;
    use std::path::PathBuf;

    use nonempty::nonempty;

    use super::*;

    fn config() -> Config {
        Config::default()
    }

    /// The auto-detected facets for the default synthetic partition the tests seed.
    fn auto() -> AutoFacets {
        AutoFacets {
            triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine_key: "synthetic".to_owned(),
        }
    }

    fn options() -> ListOptions {
        ListOptions::default()
    }

    /// An inline spawner that runs the load tasks on the calling thread, so the
    /// list tests need no Tokio runtime under `block_on` or Miri.
    fn spawner() -> Spawner {
        cargo_bench_history_core::testing::synchronous_spawner()
    }

    /// A result set with one record carrying two metrics, so its partition
    /// reconstructs two distinct series.
    fn two_metric_set(effective: i64, commit: &str) -> Run {
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
                Metric::new(MetricKind::InstructionCount, 100.0),
                Metric::new(MetricKind::EstimatedCycles, 100.0),
            ],
        );
        Run::new(context, vec![record])
    }

    /// A result set with two single-metric benchmarks that share a group but differ
    /// by case, so its partition reconstructs two distinct series.
    fn two_benchmark_set(effective: i64, commit: &str) -> Run {
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
        let case = |case: &str| {
            BenchmarkResult::new(
                BenchmarkId::new(nonempty![
                    "nm".to_owned(),
                    "nm::observe".to_owned(),
                    case.to_owned(),
                ]),
                vec![Metric::new(MetricKind::InstructionCount, 100.0)],
            )
        };
        Run::new(context, vec![case("pull"), case("push")])
    }

    /// A result set with one record carrying a single metric, so its partition
    /// reconstructs exactly one series.
    fn single_metric_set(effective: i64, commit: &str) -> Run {
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
            vec![Metric::new(MetricKind::InstructionCount, 100.0)],
        );
        Run::new(context, vec![record])
    }

    fn clean_key(commit: &str) -> String {
        format!("v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json")
    }

    fn store(storage: &MemoryStorage, key: &str, set: &Run) {
        let json = set.to_json().unwrap();
        block_on(storage.put(key, json.as_bytes())).unwrap();
    }

    fn linux_set() -> DiscriminantSet {
        DiscriminantSet {
            engine: "callgrind".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine_key: "synthetic".to_owned(),
        }
    }

    fn bts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    /// A discriminant set distinct from [`linux_set`], for grouping tests.
    fn mac_set() -> DiscriminantSet {
        DiscriminantSet {
            engine: "criterion".to_owned(),
            target_triple: "aarch64-apple-darwin".to_owned(),
            machine_key: "synthetic".to_owned(),
        }
    }

    #[test]
    fn group_by_set_runs_consecutive_entries_of_the_same_set_together() {
        let entry = |set: DiscriminantSet, benchmark: &str| BlessingEntry {
            set,
            benchmark: Some(benchmark.to_owned()),
            commit: "c0".to_owned(),
            commit_time: Some(bts(1)),
            issued_at: None,
            prefixes: Vec::new(),
        };
        // Two linux entries then one mac entry: the run of same-set rows collapses
        // into one group, and the differing set starts a new one.
        let entries = vec![
            entry(linux_set(), "a"),
            entry(linux_set(), "b"),
            entry(mac_set(), "c"),
        ];
        let groups = group_by_set(&entries);
        assert_eq!(groups.len(), 2, "two distinct sets form two groups");
        assert_eq!(*groups[0].0, linux_set());
        assert_eq!(groups[0].1.len(), 2, "both linux rows share one group");
        assert_eq!(*groups[1].0, mac_set());
        assert_eq!(groups[1].1.len(), 1, "the mac row is its own group");
    }

    #[test]
    fn append_hint_and_warning_pushes_blank_separated_sections() {
        let mut lines = vec!["body".to_owned()];
        append_hint_and_warning(&mut lines, Some("the hint"), Some("the warning"));
        assert_eq!(
            lines,
            vec![
                "body".to_owned(),
                String::new(),
                "the hint".to_owned(),
                String::new(),
                "the warning".to_owned(),
            ]
        );

        let mut none = vec!["body".to_owned()];
        append_hint_and_warning(&mut none, None, None);
        assert_eq!(none, vec!["body".to_owned()]);
    }

    #[test]
    fn render_blessings_text_renders_head_and_all_views() {
        let head = BlessingEntry {
            set: linux_set(),
            benchmark: None,
            commit: "abc123".to_owned(),
            commit_time: Some(bts(1_700_000_000)),
            issued_at: Some(bts(1_700_000_500)),
            prefixes: vec!["nm/observe".to_owned(), "nm/record".to_owned()],
        };
        let text = render_blessings_text("folo", false, "abc123", std::slice::from_ref(&head));
        assert!(
            text.contains("Blessings for project folo at commit abc123"),
            "{text}"
        );
        assert!(
            text.contains("callgrind/x86_64-unknown-linux-gnu/synthetic"),
            "{text}"
        );
        assert!(
            text.contains("abc123 accepts nm/observe, nm/record"),
            "{text}"
        );

        let all = BlessingEntry {
            set: linux_set(),
            benchmark: Some("nm/observe".to_owned()),
            commit: "abc123".to_owned(),
            commit_time: Some(bts(1_700_000_000)),
            issued_at: None,
            prefixes: Vec::new(),
        };
        let text = render_blessings_text("folo", true, "abc123", std::slice::from_ref(&all));
        assert!(
            text.contains("Most recent blessings for project folo"),
            "{text}"
        );
        assert!(text.contains("nm/observe  blessed at abc123"), "{text}");

        let empty = render_blessings_text("folo", false, "abc123", &[]);
        assert!(
            empty.contains("No blessings recorded at commit abc123."),
            "{empty}"
        );
    }

    #[test]
    fn render_blessings_markdown_renders_head_and_all_views() {
        let head = BlessingEntry {
            set: linux_set(),
            benchmark: None,
            commit: "abc123".to_owned(),
            commit_time: Some(bts(1_700_000_000)),
            issued_at: Some(bts(1_700_000_500)),
            prefixes: vec!["nm/observe".to_owned()],
        };
        let md = render_blessings_markdown("folo", false, "abc123", std::slice::from_ref(&head));
        assert!(md.contains("# Blessings for folo at abc123"), "{md}");
        assert!(md.contains("| Commit | Prefixes | Issued |"), "{md}");
        assert!(md.contains("| abc123 | nm/observe |"), "{md}");

        let all = BlessingEntry {
            set: linux_set(),
            benchmark: Some("nm/observe".to_owned()),
            commit: "abc123".to_owned(),
            commit_time: Some(bts(1_700_000_000)),
            issued_at: None,
            prefixes: Vec::new(),
        };
        let md = render_blessings_markdown("folo", true, "abc123", std::slice::from_ref(&all));
        assert!(md.contains("# Most recent blessings for folo"), "{md}");
        assert!(
            md.contains("| Benchmark | Blessed at | Commit time |"),
            "{md}"
        );
        assert!(md.contains("| nm/observe | abc123 |"), "{md}");

        let empty = render_blessings_markdown("folo", true, "abc123", &[]);
        assert!(
            empty.contains("No blessings in the analysis window."),
            "{empty}"
        );
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

    /// A throwaway in-memory output writer for list tests that assert on the
    /// returned text message rather than on written files.
    fn writer() -> MemoryOutputWriter {
        MemoryOutputWriter::new()
    }

    /// Drives `list_with` and unwraps the rendered message.
    fn list(storage: &MemoryStorage, git: &FakeGitHistory, options: &ListOptions) -> String {
        let outcome = block_on(list_with(
            git,
            storage,
            "folo",
            &config(),
            options,
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &writer(),
            &spawner(),
        ))
        .unwrap();
        match outcome {
            RunOutcome::Completed { message } => message,
            RunOutcome::Analyzed { .. } => panic!("list returns a Completed outcome"),
        }
    }

    /// Drives `list_with` requesting the JSON report into an in-memory writer and
    /// returns the JSON text. The text report is suppressed so the JSON file is
    /// the only rendered output.
    fn list_json(storage: &MemoryStorage, git: &FakeGitHistory, options: &ListOptions) -> String {
        let mut options = options.clone();
        options.no_text = true;
        options.markdown = None;
        options.json = Some(PathBuf::from("report.json"));
        let writer = MemoryOutputWriter::new();
        block_on(list_with(
            git,
            storage,
            "folo",
            &config(),
            &options,
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &writer,
            &spawner(),
        ))
        .unwrap();
        writer
            .written(Path::new("report.json"))
            .expect("the JSON report was written to the requested path")
    }

    /// Drives `list_with` requesting the Markdown report into an in-memory writer
    /// and returns the Markdown text.
    fn list_markdown(
        storage: &MemoryStorage,
        git: &FakeGitHistory,
        options: &ListOptions,
    ) -> String {
        let mut options = options.clone();
        options.no_text = true;
        options.json = None;
        options.markdown = Some(PathBuf::from("report.md"));
        let writer = MemoryOutputWriter::new();
        block_on(list_with(
            git,
            storage,
            "folo",
            &config(),
            &options,
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &writer,
            &spawner(),
        ))
        .unwrap();
        writer
            .written(Path::new("report.md"))
            .expect("the Markdown report was written to the requested path")
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

        let report = list_json(&storage, &git, &options());
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

        let report = list_markdown(&storage, &git, &options());
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
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &writer(),
            &spawner(),
        ))
        .unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }

    #[test]
    fn list_engine_facet_restricts_the_data_set() {
        // Two sets in the same triple/machine-key partition differing only by engine.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &two_metric_set(0, "c0"));
        store(
            &storage,
            "v1/folo/criterion/x86_64-unknown-linux-gnu/synthetic/c0/clean.json",
            &two_metric_set(0, "c0"),
        );
        let git = linear_git();

        let opts = ListOptions {
            engine: vec!["callgrind".to_owned()],
            ..options()
        };
        let report = list_json(&storage, &git, &opts);
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
    fn list_rejects_when_no_output_is_selected() {
        // Suppressing the text report without requesting a Markdown or JSON file
        // leaves nothing to produce, so the selection is rejected up front —
        // before any data is loaded or the subject is dispatched.
        let storage = MemoryStorage::new();
        let git = linear_git();
        let opts = ListOptions {
            no_text: true,
            ..options()
        };
        let error = block_on(list_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &writer(),
            &spawner(),
        ))
        .unwrap_err();
        match error {
            RunError::Analyze { message } => {
                assert!(message.contains("no output selected"), "{message}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn list_discriminants_shows_all_sets_by_default_and_facets_narrow() {
        // The discriminants index never requires a repository.
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &two_metric_set(0, "c0"));
        store(
            &storage,
            "v1/folo/criterion/x86_64-pc-windows-msvc/m1/c0/clean.json",
            &two_metric_set(0, "c0"),
        );
        let git = FakeGitHistory::new(); // No repo, but listing does not need one.

        // With no facets the catalog is unfiltered: it shows every stored partition
        // — including the windows/m1 set that does not match the current machine —
        // so a user can discover triples and machine keys they do not already know.
        let opts = ListOptions {
            subject: ListSubject::Discriminants,
            ..options()
        };
        let report = list_json(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let sets = parsed.as_array().unwrap();
        assert_eq!(
            sets.len(),
            2,
            "the default catalog lists every set: {report}"
        );
        let engines: Vec<&str> = sets
            .iter()
            .map(|set| set["engine"].as_str().unwrap())
            .collect();
        assert!(engines.contains(&"callgrind"), "{report}");
        assert!(engines.contains(&"criterion"), "{report}");

        // An explicit facet still narrows the catalog. (`--engine` is the facet
        // that always discriminates: synthetic partitions are exempt from the
        // triple and machine-key filters, but never from the engine filter.)
        let opts = ListOptions {
            subject: ListSubject::Discriminants,
            engine: vec!["criterion".to_owned()],
            ..options()
        };
        let report = list_json(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let sets = parsed.as_array().unwrap();
        assert_eq!(
            sets.len(),
            1,
            "an explicit engine narrows to its set: {report}"
        );
        assert_eq!(sets[0]["engine"], "criterion", "{report}");
    }

    #[test]
    fn list_runs_rejects_the_blessings_only_all_switch() {
        let storage = MemoryStorage::new();
        let git = linear_git();
        let opts = ListOptions {
            subject: ListSubject::Runs,
            all: true,
            ..options()
        };
        let error = block_on(list_with(
            &git,
            &storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &writer(),
            &spawner(),
        ))
        .unwrap_err();
        match error {
            RunError::Analyze { message } => {
                assert!(message.contains("--all"), "{message}");
                assert!(message.contains("list blessings"), "{message}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn list_discriminants_text_format_lists_each_set() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &two_metric_set(0, "c0"));
        let git = FakeGitHistory::new();
        let opts = ListOptions {
            subject: ListSubject::Discriminants,
            ..options()
        };
        let report = list(&storage, &git, &opts);
        assert!(report.contains("Discriminant sets:"), "{report}");
        assert!(
            report.contains("callgrind/x86_64-unknown-linux-gnu/synthetic"),
            "{report}"
        );
    }

    #[test]
    fn list_discriminants_markdown_renders_a_table() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c0"), &two_metric_set(0, "c0"));
        let git = FakeGitHistory::new();
        let opts = ListOptions {
            subject: ListSubject::Discriminants,
            ..options()
        };
        let report = list_markdown(&storage, &git, &opts);
        assert!(report.contains("# Discriminant sets"), "{report}");
        assert!(
            report.contains("| Engine | Target triple | Machine key |"),
            "{report}"
        );
        assert!(
            report.contains("| callgrind | x86_64-unknown-linux-gnu | synthetic |"),
            "{report}"
        );
    }

    #[test]
    fn list_discriminants_markdown_empty_reports_none() {
        let storage = MemoryStorage::new();
        let git = FakeGitHistory::new();
        let opts = ListOptions {
            subject: ListSubject::Discriminants,
            ..options()
        };
        let report = list_markdown(&storage, &git, &opts);
        assert!(report.contains("# Discriminant sets"), "{report}");
        assert!(report.contains("No discriminant sets found."), "{report}");
        // The empty markdown body has no table.
        assert!(!report.contains("| Engine |"), "{report}");
    }

    #[test]
    fn list_discriminants_text_empty_reports_none() {
        let storage = MemoryStorage::new();
        let git = FakeGitHistory::new();
        let opts = ListOptions {
            subject: ListSubject::Discriminants,
            ..options()
        };
        let report = list(&storage, &git, &opts);
        assert_eq!(report, "No discriminant sets found.\n", "{report}");
    }

    #[test]
    fn list_runs_markdown_empty_selection_reports_no_match() {
        let storage = MemoryStorage::new();
        let git = linear_git();
        let report = list_markdown(&storage, &git, &options());
        assert!(report.contains("# Data set for folo"), "{report}");
        assert!(
            report.contains("No stored run matches the selection."),
            "{report}"
        );
    }

    /// The blessing-sidecar key in the same partition as [`clean_key`].
    fn bless_key(commit: &str, issued_unix: i64) -> String {
        format!(
            "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/bless-{issued_unix}.json"
        )
    }

    fn store_bless(storage: &MemoryStorage, key: &str, record: &BlessingRecord) {
        let json = record.to_json().unwrap();
        block_on(storage.put(key, json.as_bytes())).unwrap();
    }

    #[test]
    fn list_blessings_lists_the_sidecars_at_head() {
        let storage = MemoryStorage::new();
        // HEAD is c3 in `linear_git`; record a clean run and a blessing there.
        store(&storage, &clean_key("c3"), &two_metric_set(3, "c3"));
        let record = BlessingRecord::new(
            "c3".to_owned(),
            Timestamp::from_second(100).unwrap(),
            vec![BenchmarkIdPrefix::new("nm/nm::observe").unwrap()],
            "0.0.1".to_owned(),
        );
        store_bless(&storage, &bless_key("c3", 100), &record);
        let git = linear_git();

        let opts = ListOptions {
            subject: ListSubject::Blessings,
            ..options()
        };
        let report = list_json(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["scope"], "head");
        assert_eq!(parsed["commit"], "c3");
        let blessings = parsed["blessings"].as_array().unwrap();
        assert_eq!(blessings.len(), 1, "{report}");
        assert_eq!(blessings[0]["commit"], "c3");
        assert_eq!(blessings[0]["prefixes"][0], "nm/nm::observe");
        // The HEAD view reports the sidecar itself, not a per-benchmark roll-up.
        assert!(blessings[0]["benchmark"].is_null(), "{report}");
    }

    #[test]
    fn list_blessings_reports_none_at_head_when_unblessed() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c3"), &two_metric_set(3, "c3"));
        let git = linear_git();

        let opts = ListOptions {
            subject: ListSubject::Blessings,
            ..options()
        };
        let report = list(&storage, &git, &opts);
        assert!(
            report.contains("No blessings recorded at commit c3."),
            "{report}"
        );
    }

    #[test]
    fn list_blessings_markdown_at_head_renders_the_sidecar() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c3"), &two_metric_set(3, "c3"));
        let record = BlessingRecord::new(
            "c3".to_owned(),
            Timestamp::from_second(100).unwrap(),
            vec![BenchmarkIdPrefix::new("nm/nm::observe").unwrap()],
            "0.0.1".to_owned(),
        );
        store_bless(&storage, &bless_key("c3", 100), &record);
        let git = linear_git();

        let opts = ListOptions {
            subject: ListSubject::Blessings,
            ..options()
        };
        let report = list_markdown(&storage, &git, &opts);
        assert!(report.contains("nm/nm::observe"), "{report}");
        assert!(
            report.contains('|'),
            "a markdown table is rendered: {report}"
        );
    }

    #[test]
    fn list_blessings_markdown_reports_none_at_head_when_unblessed() {
        let storage = MemoryStorage::new();
        store(&storage, &clean_key("c3"), &two_metric_set(3, "c3"));
        let git = linear_git();

        let opts = ListOptions {
            subject: ListSubject::Blessings,
            ..options()
        };
        let report = list_markdown(&storage, &git, &opts);
        assert!(
            report.contains("No blessings recorded at this commit."),
            "{report}"
        );
    }

    /// Drives `list blessings` expecting the load to fail, returning the error.
    fn list_blessings_error(storage: &MemoryStorage, git: &FakeGitHistory) -> RunError {
        let opts = ListOptions {
            subject: ListSubject::Blessings,
            ..options()
        };
        block_on(list_with(
            git,
            storage,
            "folo",
            &config(),
            &opts,
            &auto(),
            Timestamp::from_second(0).unwrap(),
            &RecordingReporter::new(),
            &writer(),
            &spawner(),
        ))
        .unwrap_err()
    }

    #[test]
    fn list_blessings_requires_a_repository() {
        let storage = MemoryStorage::new();
        // No commits: HEAD does not resolve.
        let error = list_blessings_error(&storage, &FakeGitHistory::new());
        match error {
            RunError::Analyze { message } => {
                assert!(message.contains("could not resolve HEAD"), "{message}");
            }
            other => panic!("expected an analyze error, got {other:?}"),
        }
    }

    #[test]
    fn list_blessings_rejects_a_non_utf8_sidecar_at_head() {
        let storage = MemoryStorage::new();
        // HEAD is c3 in `linear_git`; a sidecar there with corrupt bytes.
        block_on(storage.put(&bless_key("c3", 100), &[0xff, 0xfe, 0x00])).unwrap();
        let error = list_blessings_error(&storage, &linear_git());
        match error {
            RunError::Analyze { message } => {
                assert!(message.contains("is not valid UTF-8"), "{message}");
            }
            other => panic!("expected an analyze error, got {other:?}"),
        }
    }

    #[test]
    fn list_blessings_rejects_an_invalid_sidecar_at_head() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&bless_key("c3", 100), b"{ not a blessing record")).unwrap();
        let error = list_blessings_error(&storage, &linear_git());
        match error {
            RunError::Analyze { message } => {
                assert!(message.contains("is not a valid blessing"), "{message}");
            }
            other => panic!("expected an analyze error, got {other:?}"),
        }
    }

    #[test]
    fn list_blessings_all_rolls_up_the_latest_blessing_per_benchmark() {
        let storage = MemoryStorage::new();
        for index in 0..4 {
            let commit = format!("c{index}");
            store(
                &storage,
                &clean_key(&commit),
                &two_metric_set(index, &commit),
            );
        }
        // A blessing at c2 (mid-history) accepting the benchmark family.
        let record = BlessingRecord::new(
            "c2".to_owned(),
            Timestamp::from_second(100).unwrap(),
            vec![BenchmarkIdPrefix::new("nm/nm::observe").unwrap()],
            "0.0.1".to_owned(),
        );
        store_bless(&storage, &bless_key("c2", 100), &record);
        let git = linear_git();

        let opts = ListOptions {
            subject: ListSubject::Blessings,
            all: true,
            ..options()
        };
        let report = list_json(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(parsed["scope"], "window");
        // No HEAD anchor in the window view.
        assert!(parsed["commit"].is_null(), "{report}");
        let blessings = parsed["blessings"].as_array().unwrap();
        // The two metrics share one blessing, reported once per benchmark.
        assert_eq!(blessings.len(), 1, "{report}");
        assert_eq!(blessings[0]["benchmark"], "nm/nm::observe/pull");
        assert_eq!(blessings[0]["commit"], "c2");
    }

    #[test]
    fn list_blessings_all_lists_a_single_metric_benchmark_once() {
        // A benchmark with exactly one metric forms one series, so the window
        // roll-up must emit one entry: the dedup `seen.insert` guard keeps a first
        // occurrence rather than dropping it.
        let storage = MemoryStorage::new();
        for index in 0..4 {
            let commit = format!("c{index}");
            store(
                &storage,
                &clean_key(&commit),
                &single_metric_set(index, &commit),
            );
        }
        let record = BlessingRecord::new(
            "c2".to_owned(),
            Timestamp::from_second(100).unwrap(),
            vec![BenchmarkIdPrefix::new("nm/nm::observe").unwrap()],
            "0.0.1".to_owned(),
        );
        store_bless(&storage, &bless_key("c2", 100), &record);
        let git = linear_git();

        let opts = ListOptions {
            subject: ListSubject::Blessings,
            all: true,
            ..options()
        };
        let report = list_json(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let blessings = parsed["blessings"].as_array().unwrap();
        assert_eq!(blessings.len(), 1, "{report}");
        assert_eq!(blessings[0]["benchmark"], "nm/nm::observe/pull");
        assert_eq!(blessings[0]["commit"], "c2");
    }

    #[test]
    fn list_blessings_all_reports_an_empty_window_in_text() {
        // The window roll-up over clean runs with no blessing records skips every
        // series and renders the empty-window message in text form.
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
            subject: ListSubject::Blessings,
            all: true,
            ..options()
        };
        let report = list(&storage, &git, &opts);
        assert!(
            report.contains("No blessings in the analysis window."),
            "{report}"
        );
    }

    #[test]
    fn list_blessings_all_sorts_multiple_benchmark_entries() {
        // Two distinct benchmarks blessed in the same window roll up to two entries,
        // exercising the stable (set, benchmark, commit) ordering.
        let storage = MemoryStorage::new();
        for index in 0..3 {
            let commit = format!("c{index}");
            store(
                &storage,
                &clean_key(&commit),
                &two_benchmark_set(index, &commit),
            );
        }
        let record = BlessingRecord::new(
            "c1".to_owned(),
            Timestamp::from_second(100).unwrap(),
            vec![BenchmarkIdPrefix::new("nm/nm::observe").unwrap()],
            "0.0.1".to_owned(),
        );
        store_bless(&storage, &bless_key("c1", 100), &record);
        let git = linear_git();

        let opts = ListOptions {
            subject: ListSubject::Blessings,
            all: true,
            ..options()
        };
        let report = list_json(&storage, &git, &opts);
        let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
        let blessings = parsed["blessings"].as_array().unwrap();
        assert_eq!(blessings.len(), 2, "{report}");
        assert_eq!(blessings[0]["benchmark"], "nm/nm::observe/pull");
        assert_eq!(blessings[1]["benchmark"], "nm/nm::observe/push");
    }
}
