//! `select_dataset`: resolve the git timeline, enumerate and fold the in-selection
//! objects into a `SelectedDataSet`, and explain an empty outcome.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyspawn::Spawner;
use cbh_config::Config;
use cbh_detect::{
    AnalysisMode, BlessingPlacement, DiscriminantSetQuery, MAX_BRANCH_BASE_COMMITS, Series,
    SeriesFilter, attach_base_windows,
};
use cbh_diag::{Reporter, ReporterExt, count_noun};
use cbh_git::GitHistory;
use cbh_model::{BenchmarkIdPrefix, BlessingRecord, DiscriminantSet, StorageKey};
use cbh_storage::Storage;
use jiff::Timestamp;

use super::announce::{AnnouncedBase, AnnouncedSince, announce_selection, selection_announcement};
use super::discriminants::{
    AutoDiscriminants, describe_effective_discriminants, discriminants_are_unconstrained,
    resolve_discriminants,
};
use super::history::{DirtyTipPolicy, ResolvedHistory, resolve_history};
use super::load::{
    CandidateListing, RunIndex, WorkerFold, fold_runs_chunked, list_candidates,
    load_objects_concurrently,
};
use super::selection::Selection;
use super::window::{auto_mode, before_since_cutoff, resolve_since, since_cutoff_reason};
use crate::{
    AnalyzeError, FirstParentWalkFailedError, InvalidBlessingError, InvalidStoredUtf8Error,
};

/// The data an analysis (or listing) draws on, plus the bookkeeping needed to
/// explain an empty outcome and warn about ephemeral data.
pub(crate) struct SelectedDataSet {
    /// The reconstructed series for the in-window runs, built with the caller's
    /// series filter and ordered by git topology. Pre-blessing: the caller applies
    /// blessings to the mode-appropriate evidence line; listings leave them unapplied.
    pub(crate) series: Vec<Series>,
    /// Compact per-set, per-commit run tallies, standing in for a retained copy of
    /// every loaded object (which a large history cannot afford to keep resident).
    pub(crate) run_index: RunIndex,
    /// How many discriminant-matching candidates existed before topology filtering.
    pub(crate) candidate_count: usize,
    /// Why candidates were excluded, for the empty-history hint.
    pub(crate) tally: ExclusionTally,
    /// Whether a dirty run was admitted solely by the base-branch dirty-tree
    /// exception (triggers the ephemeral-data warning).
    pub(crate) included_dirty_base_exception: bool,
    /// The target ref the timeline was resolved against (for diagnostics).
    pub(crate) target_ref: String,
    /// The resolved discriminant-set query (the effective, possibly auto-detected
    /// engine / target-triple / machine-key filters), so an empty outcome can name
    /// the exact partition it searched.
    pub(crate) discriminants: DiscriminantSetQuery,
    /// Subject line of each in-history commit that has one, so `examine` can label
    /// each data point with what its commit changed. A commit absent here has an
    /// empty subject; only `examine` reads this.
    pub(crate) commit_subjects: HashMap<String, String>,
    /// The analyzed commits in first-parent order, oldest first, indexed by
    /// topological position. It lets `examine` name every commit in a range,
    /// including the ones that carry no observation; only `examine` reads this.
    pub(crate) ordered_commits: Vec<String>,
    /// The full commit ID of the analyzed tip commit (the resolved `--context`/HEAD),
    /// carried into the report so it names the exact commit the findings describe.
    pub(crate) tip_commit: String,
    /// First-parent topological index of the analyzed tip commit. History-mode chart
    /// building uses it as the trailing-fill target so a series that stops short of the
    /// tip renders the data-less commits after its last observation as a gap.
    pub(crate) tip_index: usize,
    /// Whether the working tree carried uncommitted changes when the analysis ran;
    /// the report annotates the tip `+ uncommitted changes` when set.
    pub(crate) tip_dirty: bool,
    /// The resolved analysis mode, auto-detected from the git topology.
    pub(crate) mode: AnalysisMode,
    /// First-parent topological index of the merge-base on the context line, when it
    /// lies there. List, prune, and examine use the context line exactly as resolved;
    /// branch analysis loads the base-ref comparison window separately.
    pub(crate) merge_base_index: Option<usize>,
    /// First-parent topological index of the resolved base ref on its own history.
    ///
    /// Present in branch mode when detection requested base-ref windows. The detector
    /// and lag classifier use this base-ref coordinate independently of the context
    /// ref's first-parent line.
    pub(crate) base_ref_index: Option<usize>,
    /// Blessings recorded on in-window commits, grouped by discriminant set. Each
    /// entry pairs the blessed commit's first-parent topological index and its
    /// committer date (from topology, for the report anchor) with the record. History
    /// positions these on the context line; branch mode positions them on the base-ref
    /// line so pre-blessing base evidence cannot enter a comparison.
    pub(crate) blessings: HashMap<DiscriminantSet, Vec<BlessingPlacement>>,
    /// Base-ref clean-run observations retained (branch mode only) so the analysis can
    /// classify why a finding's comparison base lags the base ref. Entries may come
    /// from selected candidates or machine-relaxed sibling candidates; the classifier
    /// keeps only observations under a different machine key from the finding it is
    /// explaining. Empty in history mode. Each entry carries the storage key,
    /// discriminant set, and first-parent topological index; the payload is fetched
    /// lazily by the pipeline only when a lagging finding needs it.
    pub(crate) sibling_observations: Vec<SiblingObservation>,
}

/// A base-ref clean run retained for branch-mode comparison-base lag classification.
#[derive(Clone, Debug)]
pub(crate) struct SiblingObservation {
    /// The object's storage key, for the lazy payload fetch.
    pub(crate) key: String,
    /// The parsed storage key, carrying the sibling's discriminant set (a machine key
    /// the selection did not cover). Retained so the lazy fetch reuses it without a
    /// re-parse.
    pub(crate) parsed: StorageKey,
    /// First-parent topological index of the sibling's commit.
    pub(crate) topo_index: usize,
}

/// Resolves the git topology, selects the comparable commits, and loads the
/// in-selection objects into a [`SelectedDataSet`]. Requires a repository: the
/// timeline is reconstructed from git history, not from stored timestamps.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the analyze selection pipeline, which threads the same injected ports"
)]
pub(crate) async fn select_dataset<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    selection: &Selection<'_>,
    filter: SeriesFilter<'_>,
    load_branch_base_windows: bool,
    auto: &AutoDiscriminants,
    now: Timestamp,
    reporter: &dyn Reporter,
    spawner: &Spawner,
) -> Result<SelectedDataSet, AnalyzeError>
where
    G: GitHistory,
    S: Storage + Clone + 'static,
{
    let discriminants = resolve_discriminants(selection, Some(auto))?;
    let listing_started = Instant::now();
    let CandidateListing {
        selected: candidates,
        siblings: sibling_candidates,
    } = list_candidates(
        storage,
        project_id,
        &discriminants,
        load_branch_base_windows,
        reporter,
    )
    .await?;
    reporter.timing(
        "candidate listing + discriminant filter (includes storage.list)",
        listing_started.elapsed(),
    );

    // Separate blessing sidecars from run objects: they share the partition prefix
    // but carry a different payload and are loaded into their own map rather than
    // the series.
    let (candidates, bless_candidates): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|(_, parsed)| !parsed.is_bless());
    if !bless_candidates.is_empty() {
        reporter.note_with(|| {
            format!(
                "{} of those are blessing sidecars",
                count_noun(bless_candidates.len(), "object")
            )
        });
    }

    let topology_started = Instant::now();
    let ResolvedHistory {
        target_ref,
        base_name,
        base_commit,
        tip_commit,
        tip_dirty,
        order,
        ordered_commits,
        commit_times,
        commit_subjects,
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

    // The topology lookups the parallel fold needs — the commit -> first-parent
    // index map and the base-branch dirty-tree exceptions — are shared read-only
    // across every worker, so wrap them in `Arc` once and hand each worker a cheap
    // clone. Downstream main-thread reads still go through `Deref`.
    let order = Arc::new(order);
    let dirty_base_exception = Arc::new(dirty_base_exception);

    // Mode auto-detection keys off git topology and the *admitted* data set. The
    // branch view compares a feature branch's runs against its base, so it applies
    // only when feature-branch data is actually present: commits past the
    // merge-base, or a dirty run admitted on top of the base tip. That base-tip
    // dirty run is admitted only while the working tree is currently dirty (the
    // exception in `resolve_history`), so this single signal tracks the working
    // tree — a clean tree neither admits the run nor leaves the history view.
    let dirty_tip_run_present = candidates.iter().any(|(_, parsed)| {
        parsed.is_dirty()
            && dirty_base_exception
                .get(parsed.commit.as_str())
                .copied()
                .unwrap_or(false)
    });

    // The mode steers the analysis and the default `--since`; it is auto-detected
    // from the git topology and the recorded data.
    let mode = auto_mode(tip_is_merge_base, dirty_tip_run_present);
    reporter.note_with(|| {
        format!(
            "analysis mode: {} (auto-detected because the target tip {} its own merge-base \
             with the base branch and {} admitted on top of it; a base-tip dirty run is \
             admitted only while the working tree is currently dirty)",
            mode.as_str(),
            if tip_is_merge_base { "is" } else { "is not" },
            if dirty_tip_run_present {
                "a dirty run is"
            } else {
                "no dirty run is"
            },
        )
    });
    let since = resolve_since(selection.since, mode, now)?;
    reporter.note_with(|| {
        format!(
            "since cutoff: {} ({})",
            since.map_or_else(|| "none".to_owned(), |since| since.to_string()),
            since_cutoff_reason(selection.since.is_some(), mode)
        )
    });

    let base_ref_history = if mode == AnalysisMode::Branch && load_branch_base_windows {
        Some(resolve_base_ref_history(git, &base_commit, reporter).await?)
    } else {
        None
    };
    let base_ref_index = base_ref_history
        .as_ref()
        .and_then(|history| history.tip_index);
    let base_series = if let Some(history) = base_ref_history.as_ref() {
        let load_started = Instant::now();
        let series = load_branch_base_series(
            storage,
            spawner,
            &candidates,
            history,
            since,
            filter,
            reporter,
        )
        .await?;
        reporter.timing(
            "branch base-ref window load (filter + fetch + parse + fold)",
            load_started.elapsed(),
        );
        series
    } else {
        Vec::new()
    };

    // Retain the base-side sibling observations branch mode uses to explain a lagging
    // comparison base. Only clean runs on the base ref can serve as a comparison
    // base, and the same `--since` admission the selection uses applies.
    // History mode has no single comparison base, so it keeps none. This is key-only
    // work (no fetches); the payloads are read lazily only if a finding actually lags.
    let sibling_observations = admit_comparison_base_observations(
        candidates.iter().cloned().chain(sibling_candidates),
        mode,
        base_ref_history.as_ref(),
        since,
        reporter,
    );

    // The always-on effective-selection announcement: one line, printed regardless
    // of `--verbose`, naming the resolved (possibly auto-detected) partition, base
    // branch, and look-back window a plain run would otherwise resolve silently.
    announce_selection(
        reporter,
        &effective_selection_summary(
            &discriminants,
            &base_name,
            selection.base.is_none(),
            since,
            selection.since.is_some(),
            mode,
        ),
    );

    // Tally why candidates do not enter the analysis, so a `0 runs` outcome can
    // explain itself (via `--verbose` per object, and via a summary hint when
    // candidates existed but none were admitted).
    let candidate_count = candidates.len();
    let mut excluded_outside_history = 0_usize;
    let mut excluded_dirty_base = 0_usize;
    let mut excluded_since = 0_usize;

    // Phase 1 — key-only filtering, in candidate order. Every exclusion that does
    // not need the object's payload runs here, before anything is fetched, so an
    // excluded candidate never costs a round-trip: history membership, base-side
    // dirty admission, and the `--since` cutoff (decided from each commit's
    // committer time, which git reports with the topology).
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
        if before_since_cutoff(commit_times.get(&parsed.commit).copied(), since) {
            excluded_since = excluded_since.saturating_add(1);
            reporter.note_with(|| {
                format!(
                    "excluding {key}: commit {} is before the --since cutoff",
                    parsed.commit
                )
            });
            continue;
        }
        to_fetch.push((key, parsed));
    }
    reporter.timing(
        "phase 1 — key-only candidate filtering (no fetches)",
        phase1_started.elapsed(),
    );

    // Phase 2/3 — fetch the survivors and fold each into the series. The fetch +
    // decompress + JSON parse (the CPU-dominated cost) is spread across the
    // runtime's worker threads: the storage-key-sorted survivors are split into
    // balanced contiguous chunks, and one spawned task fetches, parses, *and folds*
    // each chunk into its own series builder — dropping each parsed run the instant
    // its compact points are extracted. The main thread then merges the per-worker
    // builders, run tallies, and admission lists into one. Each object's ordinal —
    // the final point tie-break — is its rank in storage-key order, assigned up
    // front, so the single `builder.finish()` sort reproduces the in-order result.
    // The per-object verbose notes are collected during the fold and emitted in
    // storage-key order afterwards, so the diagnostics stay byte-identical to a
    // deterministic in-order pass.
    //
    // Folding inside each worker keeps only the compact per-worker points resident
    // between fetch and merge — never the whole parsed data set — so the parallel
    // parse does not buy its throughput with the full-buffer memory peak.
    to_fetch.sort_by(|left, right| left.0.cmp(&right.0));

    let fetch_fold_started = Instant::now();
    let ranked: Vec<(usize, String, StorageKey)> = to_fetch
        .into_iter()
        .enumerate()
        .map(|(rank, (key, parsed))| (rank, key, parsed))
        .collect();
    let prefixes: Arc<[BenchmarkIdPrefix]> = Arc::from(filter.prefixes);
    let WorkerFold {
        builder,
        run_index,
        mut admitted,
    } = fold_runs_chunked(
        storage,
        spawner,
        ranked,
        &order,
        &dirty_base_exception,
        prefixes,
    )
    .await?;
    // Whether at least one dirty run was admitted solely by the base-branch
    // dirty-tree exception, so the report can warn that it is ephemeral.
    let included_dirty_base_exception = admitted.iter().any(|(_, is_exception)| *is_exception);
    reporter.timing(
        "phase 2/3 — chunked parallel fetch + parse + per-worker fold, then merge",
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
    let mut series = builder.finish();
    if !base_series.is_empty() {
        attach_base_windows(&mut series, &base_series, MAX_BRANCH_BASE_COMMITS);
    }
    reporter.timing(
        "series build finalization (builder.finish: assemble + serial point sort)",
        finish_started.elapsed(),
    );
    reporter.note_with(|| {
        format!(
            "{} entered the analysis ({excluded_outside_history} outside history, \
         {excluded_dirty_base} dirty-on-base, {excluded_since} before --since)",
            count_noun(run_index.total(), "object")
        )
    });

    // Load blessing sidecars into the topology the active analysis uses. History
    // positions them on the context line; branch detection positions them on the base
    // ref's own first-parent line so pre-blessing base evidence is excluded even after
    // the feature branch diverged.
    let mut blessings: HashMap<DiscriminantSet, Vec<BlessingPlacement>> = HashMap::new();
    if mode == AnalysisMode::History || load_branch_base_windows {
        let blessing_started = Instant::now();
        // Phase 1 — key-only filtering: drop blessings whose commit is not on the
        // analyzed history before fetching, in candidate order.
        let mut to_fetch: Vec<(String, StorageKey)> = Vec::new();
        for (key, parsed) in bless_candidates {
            let on_analysis_history = match mode {
                AnalysisMode::History => order.contains_key(&parsed.commit),
                AnalysisMode::Branch => base_ref_history
                    .as_ref()
                    .is_some_and(|history| history.order.contains_key(&parsed.commit)),
            };
            if on_analysis_history {
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
            let text = String::from_utf8(bytes).map_err(|error| {
                InvalidStoredUtf8Error::caused_by("stored blessing", key, error)
            })?;
            BlessingRecord::from_json(&text).map_err(|error| {
                InvalidBlessingError::caused_by("stored blessing", key, "blessing record", error)
                    .into()
            })
        })
        .await?;
        fetched.sort_by(|left, right| left.0.cmp(&right.0));
        // Phase 3 — record each blessing against its commit's topological index
        // and committer date (resolved from topology, for the report anchor).
        for (key, parsed, record) in fetched {
            let (topo_index, commit_time) = match mode {
                AnalysisMode::History => (
                    order.get(&parsed.commit).copied(),
                    commit_times.get(&parsed.commit).copied(),
                ),
                AnalysisMode::Branch => base_ref_history.as_ref().map_or((None, None), |history| {
                    (
                        history.order.get(&parsed.commit).copied(),
                        history.commit_times.get(&parsed.commit).copied(),
                    )
                }),
            };
            let topo_index =
                topo_index.expect("phase 1 admitted only blessings on the analysis topology");
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
            "blessing sidecar load (filter + fetch + parse)",
            blessing_started.elapsed(),
        );
    }

    // The analyzed tip's first-parent index (in practice `order.len() - 1`). History
    // mode charts use it as the trailing-fill target so a series that stops short of
    // the tip renders the data-less commits after its last observation as a gap.
    let tip_index = order
        .get(&tip_commit)
        .copied()
        .unwrap_or_else(|| order.len().saturating_sub(1));

    Ok(SelectedDataSet {
        series,
        run_index,
        candidate_count,
        tally: ExclusionTally {
            outside_history: excluded_outside_history,
            dirty_base: excluded_dirty_base,
            since: excluded_since,
        },
        included_dirty_base_exception,
        target_ref,
        discriminants,
        commit_subjects,
        ordered_commits,
        tip_commit,
        tip_index,
        tip_dirty,
        mode,
        merge_base_index,
        base_ref_index,
        blessings,
        sibling_observations,
    })
}

/// The base ref's own first-parent topology.
struct BaseRefHistory {
    /// First-parent position of each base-ref commit.
    order: HashMap<String, usize>,
    /// Committer timestamp of each base-ref commit that reported one.
    commit_times: HashMap<String, Timestamp>,
    /// First-parent position of the resolved base ref commit.
    tip_index: Option<usize>,
}

async fn resolve_base_ref_history<G>(
    git: &G,
    base_commit: &str,
    reporter: &dyn Reporter,
) -> Result<BaseRefHistory, AnalyzeError>
where
    G: GitHistory,
{
    let first_parent_started = Instant::now();
    let first_parent = git
        .first_parent(base_commit)
        .await
        .map_err(|error| FirstParentWalkFailedError::caused_by(base_commit, error))?;
    reporter.timing(
        "git.first_parent ancestry walk (base ref's first-parent line)",
        first_parent_started.elapsed(),
    );

    let commit_count = first_parent.len();
    let mut order = HashMap::with_capacity(commit_count);
    let mut commit_times = HashMap::new();
    for (index, commit) in first_parent.into_iter().enumerate() {
        if let Some(time) = commit.committer_time {
            commit_times.insert(commit.commit_id.clone(), time);
        }
        order.insert(commit.commit_id, index);
    }
    reporter.note_with(|| {
        format!(
            "base ref first-parent line contributes {} for branch comparison windows",
            count_noun(commit_count, "commit")
        )
    });
    Ok(BaseRefHistory {
        order,
        commit_times,
        tip_index: commit_count.checked_sub(1),
    })
}

async fn load_branch_base_series<S>(
    storage: &S,
    spawner: &Spawner,
    candidates: &[(String, StorageKey)],
    history: &BaseRefHistory,
    since: Option<Timestamp>,
    filter: SeriesFilter<'_>,
    reporter: &dyn Reporter,
) -> Result<Vec<Series>, AnalyzeError>
where
    S: Storage + Clone + 'static,
{
    let mut excluded_outside_base = 0_usize;
    let mut excluded_dirty = 0_usize;
    let mut excluded_since = 0_usize;
    let mut to_fetch = Vec::new();
    for (key, parsed) in candidates {
        if !parsed.is_clean() {
            excluded_dirty = excluded_dirty.saturating_add(1);
            continue;
        }
        if !history.order.contains_key(&parsed.commit) {
            excluded_outside_base = excluded_outside_base.saturating_add(1);
            reporter.note_with(|| {
                format!(
                    "excluding {key} from the branch base window: commit {} is not on the \
                     base ref's first-parent history",
                    parsed.commit
                )
            });
            continue;
        }
        if before_since_cutoff(history.commit_times.get(&parsed.commit).copied(), since) {
            excluded_since = excluded_since.saturating_add(1);
            reporter.note_with(|| {
                format!(
                    "excluding {key} from the branch base window: commit {} is before the \
                     --since cutoff",
                    parsed.commit
                )
            });
            continue;
        }
        to_fetch.push((key.clone(), parsed.clone()));
    }
    reporter.note_with(|| {
        format!(
            "{} enter the branch base-ref window load ({excluded_outside_base} outside base, \
             {excluded_dirty} dirty, {excluded_since} before --since)",
            count_noun(to_fetch.len(), "clean-run object")
        )
    });

    to_fetch.sort_by(|left, right| left.0.cmp(&right.0));
    let ranked: Vec<(usize, String, StorageKey)> = to_fetch
        .into_iter()
        .enumerate()
        .map(|(rank, (key, parsed))| (rank, key, parsed))
        .collect();
    let prefixes: Arc<[BenchmarkIdPrefix]> = Arc::from(filter.prefixes);
    let order = Arc::new(history.order.clone());
    let empty_dirty_exceptions = Arc::new(HashMap::new());
    let WorkerFold { builder, .. } = fold_runs_chunked(
        storage,
        spawner,
        ranked,
        &order,
        &empty_dirty_exceptions,
        prefixes,
    )
    .await?;
    Ok(builder.finish())
}

/// Admits the base-ref observations branch mode uses to explain a lagging
/// comparison base, applying the same `--since` admission the selection uses.
/// Key-only work — no payloads are fetched. History mode, or an unavailable base-ref
/// history, yields none: neither has a single comparison base to lag.
fn admit_comparison_base_observations(
    candidates: impl IntoIterator<Item = (String, StorageKey)>,
    mode: AnalysisMode,
    base_ref_history: Option<&BaseRefHistory>,
    since: Option<Timestamp>,
    reporter: &dyn Reporter,
) -> Vec<SiblingObservation> {
    let (AnalysisMode::Branch, Some(history)) = (mode, base_ref_history) else {
        return Vec::new();
    };
    let mut observations = Vec::new();
    for (key, parsed) in candidates {
        if !parsed.is_clean() {
            continue;
        }
        let Some(&topo_index) = history.order.get(&parsed.commit) else {
            reporter.note_with(|| {
                format!(
                    "skipping comparison-base evidence {key}: commit {} is not on the base \
                     ref's history",
                    parsed.commit
                )
            });
            continue;
        };
        if before_since_cutoff(history.commit_times.get(&parsed.commit).copied(), since) {
            reporter.note_with(|| {
                format!(
                    "skipping comparison-base evidence {key}: commit {} is before the --since cutoff",
                    parsed.commit
                )
            });
            continue;
        }
        observations.push(SiblingObservation {
            key,
            parsed,
            topo_index,
        });
    }
    reporter.note_with(|| {
        format!(
            "retained {} for comparison-base classification",
            count_noun(observations.len(), "base-ref clean-run observation")
        )
    });
    observations
}

/// Builds the always-on, one-line summary of a run's effective selection: the
/// discriminant partition it searched (naming auto-detected discriminant values), the base
/// branch it split history against, and the resolved look-back cutoff.
///
/// Emitted to standard error regardless of `--verbose` so a plain run never hides
/// a value it auto-detected or defaulted. `base_auto` marks the base as
/// auto-detected (no explicit `--base`); `since_explicit` selects the wording for
/// why the `--since` cutoff is what it is.
fn effective_selection_summary(
    discriminants: &DiscriminantSetQuery,
    base_name: &str,
    base_auto: bool,
    since: Option<Timestamp>,
    since_explicit: bool,
    mode: AnalysisMode,
) -> String {
    selection_announcement(
        discriminants,
        Some(AnnouncedBase {
            name: base_name,
            auto: base_auto,
        }),
        None,
        Some(AnnouncedSince {
            cutoff: since,
            reason: since_cutoff_reason(since_explicit, mode),
        }),
    )
}

/// How many discriminant-matching candidates were excluded, by reason.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExclusionTally {
    /// Commit is not on the analyzed first-parent history.
    outside_history: usize,
    /// Dirty snapshot on a base-side commit (clean-only).
    dirty_base: usize,
    /// Effective time is before the `--since` cutoff.
    since: usize,
}

/// Builds a diagnostic hint explaining an empty outcome, so a bare `0 runs` never
/// leaves a user guessing.
///
/// Two empty cases are named. When *no* stored run matched the discriminant filters, the
/// hint names the effective (possibly auto-detected) partition it searched — so an
/// auto-detected target-triple / machine-key that simply does not match the stored
/// data explains itself — and distinguishes a genuinely empty project from a
/// missed partition. When runs matched the discriminant filters but topology or the `--since`
/// cutoff excluded them all, the hint breaks down the dominant exclusion reasons.
///
/// Returns `None` when at least one run was loaded.
pub(crate) fn empty_history_hint(
    loaded_is_empty: bool,
    candidate_count: usize,
    target_ref: &str,
    tally: ExclusionTally,
    discriminants: &DiscriminantSetQuery,
) -> Option<String> {
    if !loaded_is_empty {
        return None;
    }

    if candidate_count == 0 {
        // No stored run matched the discriminant filters at all. Either the project holds no runs
        // yet, or an auto-detected discriminant points at a partition nothing was recorded
        // under. Name the partition so the second case is not mistaken for the first.
        if discriminants_are_unconstrained(discriminants) {
            return Some(
                "No benchmark runs are stored for this project yet. Record some with a \
                 `collect` (or `backfill`) run, then try again."
                    .to_owned(),
            );
        }
        return Some(format!(
            "No stored runs matched the current selection ({}). Nothing has been \
             collected for this discriminant partition yet, or an auto-detected discriminant \
             does not match the stored data. Pass --target-triple all / --machine-key all \
             to widen the search, or run `list discriminants` to see which partitions \
             hold data.",
            describe_effective_discriminants(discriminants)
        ));
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
    lines.push("Re-run with --verbose for a per-object explanation.".to_owned());
    Some(lines.join("\n"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cbh_detect::DiscriminantFilter;
    use nonempty::nonempty;

    use super::*;

    /// A discriminant query with no auto-detected or explicit constraints, for the
    /// exclusion-reason cases whose hint text does not depend on the discriminant filters.
    fn unconstrained_discriminants() -> DiscriminantSetQuery {
        DiscriminantSetQuery {
            engine: DiscriminantFilter::All,
            target_triple: DiscriminantFilter::All,
            machine_key: DiscriminantFilter::All,
        }
    }

    #[test]
    fn empty_history_hint_explains_only_when_runs_were_excluded() {
        let no_exclusions = ExclusionTally {
            outside_history: 0,
            dirty_base: 0,
            since: 0,
        };
        let discriminants = unconstrained_discriminants();
        // Runs were actually loaded → no hint regardless of candidate count.
        assert_eq!(
            empty_history_hint(false, 3, "master", no_exclusions, &discriminants),
            None
        );

        let tally = ExclusionTally {
            outside_history: 2,
            dirty_base: 1,
            since: 4,
        };
        let hint = empty_history_hint(true, 7, "master", tally, &discriminants).unwrap();
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
        };
        let hint = empty_history_hint(true, 3, "master", dirty_only, &discriminants).unwrap();
        assert!(hint.contains("dirty (uncommitted-tree)"), "{hint}");
        assert!(!hint.contains("outside"), "{hint}");
        assert!(!hint.contains("--since cutoff"), "{hint}");

        // Only the outside-history reason is present here (dirty omitted).
        let outside_only = ExclusionTally {
            outside_history: 2,
            dirty_base: 0,
            since: 0,
        };
        let hint = empty_history_hint(true, 2, "master", outside_only, &discriminants).unwrap();
        assert!(hint.contains("outside master"), "{hint}");
        assert!(!hint.contains("dirty (uncommitted-tree)"), "{hint}");
        assert!(!hint.contains("--since cutoff"), "{hint}");

        // Only the since reason is present here.
        let since_only = ExclusionTally {
            outside_history: 0,
            dirty_base: 0,
            since: 5,
        };
        let hint = empty_history_hint(true, 5, "master", since_only, &discriminants).unwrap();
        assert!(
            hint.contains("5 runs older than the --since cutoff"),
            "{hint}"
        );
        assert!(!hint.contains("dirty (uncommitted-tree)"), "{hint}");
        assert!(!hint.contains("outside"), "{hint}");
    }

    #[test]
    fn empty_history_hint_names_the_empty_partition_when_nothing_matched() {
        let no_exclusions = ExclusionTally {
            outside_history: 0,
            dirty_base: 0,
            since: 0,
        };

        // Unconstrained discriminant filters that matched nothing → a genuinely empty project.
        let hint = empty_history_hint(
            true,
            0,
            "master",
            no_exclusions,
            &unconstrained_discriminants(),
        )
        .unwrap();
        assert!(hint.contains("No benchmark runs are stored"), "{hint}");
        assert!(hint.contains("collect"), "{hint}");
        // It must not misdescribe an empty project as a missed partition.
        assert!(!hint.contains("auto-detected discriminant"), "{hint}");

        // Auto-detected discriminant filters that matched nothing → name the searched partition so
        // the user learns which auto-detected values missed.
        let auto = DiscriminantSetQuery {
            engine: DiscriminantFilter::All,
            target_triple: DiscriminantFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: DiscriminantFilter::Auto("abcd".to_owned()),
        };
        let hint = empty_history_hint(true, 0, "master", no_exclusions, &auto).unwrap();
        assert!(
            hint.contains("target-triple=x86_64-pc-windows-msvc (auto-detected)"),
            "{hint}"
        );
        assert!(hint.contains("machine-key=abcd (auto-detected)"), "{hint}");
        assert!(hint.contains("--target-triple all"), "{hint}");
        assert!(hint.contains("list discriminants"), "{hint}");
    }

    #[test]
    fn effective_selection_summary_names_auto_detected_inputs() {
        let discriminants = DiscriminantSetQuery {
            engine: DiscriminantFilter::All,
            target_triple: DiscriminantFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: DiscriminantFilter::Auto("abcd".to_owned()),
        };
        let since = Timestamp::from_second(1_700_000_000).unwrap();
        // History mode, auto-detected base, default look-back: every defaulted value
        // is named and marked.
        let summary = effective_selection_summary(
            &discriminants,
            "main",
            true,
            Some(since),
            false,
            AnalysisMode::History,
        );
        assert!(
            summary.contains("target-triple=x86_64-pc-windows-msvc (auto-detected)"),
            "{summary}"
        );
        assert!(
            summary.contains("machine-key=abcd (auto-detected)"),
            "{summary}"
        );
        assert!(summary.contains("base=main (auto-detected)"), "{summary}");
        assert!(
            summary.contains("history-mode default six-month look-back"),
            "{summary}"
        );
    }

    #[test]
    fn effective_selection_summary_marks_explicit_inputs_without_auto() {
        let discriminants = DiscriminantSetQuery {
            engine: DiscriminantFilter::Explicit(nonempty!["criterion".to_owned()]),
            target_triple: DiscriminantFilter::All,
            machine_key: DiscriminantFilter::All,
        };
        // Branch mode, explicit base, no default cutoff: nothing is marked
        // auto-detected.
        let summary = effective_selection_summary(
            &discriminants,
            "release",
            false,
            None,
            false,
            AnalysisMode::Branch,
        );
        assert!(summary.contains("engine=criterion"), "{summary}");
        assert!(summary.contains("target-triple=all"), "{summary}");
        assert!(summary.contains("base=release"), "{summary}");
        assert!(!summary.contains("auto-detected"), "{summary}");
        assert!(
            summary.contains("no default look-back window outside history mode"),
            "{summary}"
        );
    }

    /// A clean sibling storage key parsed for `commit` under machine `m2`.
    fn sibling_key(commit: &str) -> (String, StorageKey) {
        let key =
            format!("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m2/{commit}/clean.json");
        let parsed = cbh_model::parse_key(&key).unwrap();
        (key, parsed)
    }

    /// The first-parent order and committer times for a `c0..c3` base-ref line.
    fn topology() -> BaseRefHistory {
        let mut order = HashMap::new();
        let mut commit_times = HashMap::new();
        for index in 0..=3 {
            let commit = format!("c{index}");
            order.insert(commit.clone(), index);
            commit_times.insert(
                commit,
                Timestamp::from_second(i64::try_from(index).unwrap()).unwrap(),
            );
        }
        BaseRefHistory {
            order,
            commit_times,
            tip_index: Some(3),
        }
    }

    #[test]
    fn comparison_base_admission_keeps_base_ref_history_commits() {
        // c2 is on the base ref's first-parent history, so it is admitted with its
        // topological index; c3 (the base ref itself) is also kept.
        let history = topology();
        let candidates = vec![sibling_key("c2"), sibling_key("c3")];
        let observations = admit_comparison_base_observations(
            candidates,
            AnalysisMode::Branch,
            Some(&history),
            None,
            &cbh_diag::RecordingReporter::new(),
        );
        let indices: Vec<usize> = observations.iter().map(|obs| obs.topo_index).collect();
        assert_eq!(indices, vec![2, 3]);
    }

    #[test]
    fn comparison_base_admission_drops_commits_off_the_base_ref_history() {
        // Unknown commits and feature-branch commits are absent from the base ref's own
        // first-parent history. Neither can serve as a base-ref comparison point.
        let history = topology();
        let candidates = vec![
            sibling_key("unknown-commit"),
            sibling_key("c2"),
            sibling_key("f1"),
        ];
        let observations = admit_comparison_base_observations(
            candidates,
            AnalysisMode::Branch,
            Some(&history),
            None,
            &cbh_diag::RecordingReporter::new(),
        );
        let indices: Vec<usize> = observations.iter().map(|obs| obs.topo_index).collect();
        assert_eq!(indices, vec![2], "only the on-history base-ref c2 survives");
    }

    #[test]
    fn comparison_base_admission_drops_commits_before_the_since_cutoff() {
        // A `--since` cutoff excludes commits committed before it, exactly as the
        // selection's own admission does.
        let history = topology();
        let candidates = vec![sibling_key("c0"), sibling_key("c2")];
        let observations = admit_comparison_base_observations(
            candidates,
            AnalysisMode::Branch,
            Some(&history),
            Some(Timestamp::from_second(2).unwrap()),
            &cbh_diag::RecordingReporter::new(),
        );
        let indices: Vec<usize> = observations.iter().map(|obs| obs.topo_index).collect();
        assert_eq!(indices, vec![2], "c0 is before the since cutoff");
    }

    #[test]
    fn comparison_base_admission_is_empty_outside_branch_mode_or_without_base_ref_history() {
        let history = topology();
        let reporter = cbh_diag::RecordingReporter::new();
        assert!(
            admit_comparison_base_observations(
                vec![sibling_key("c2")],
                AnalysisMode::History,
                Some(&history),
                None,
                &reporter,
            )
            .is_empty(),
            "history mode has no single comparison base"
        );
        assert!(
            admit_comparison_base_observations(
                vec![sibling_key("c2")],
                AnalysisMode::Branch,
                None,
                None,
                &reporter,
            )
            .is_empty(),
            "an unavailable base-ref history cannot anchor a lag"
        );
    }
}
