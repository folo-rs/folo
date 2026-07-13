//! `select_dataset`: resolve the git timeline, enumerate and fold the in-selection
//! objects into a `SelectedDataSet`, and explain an empty outcome.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyspawn::Spawner;
use cbh_config::Config;
use cbh_detect::{AnalysisMode, BlessingPlacement, DiscriminantSetQuery, Series, SeriesFilter};
use cbh_diag::{Reporter, ReporterExt, count_noun};
use cbh_git::GitHistory;
use cbh_model::{BenchmarkIdPrefix, BlessingRecord, DiscriminantSet, StorageKey};
use cbh_storage::Storage;
use jiff::Timestamp;

use super::announce::{AnnouncedBase, AnnouncedSince, selection_announcement};
use super::facets::{
    AutoFacets, describe_effective_facets, facets_are_unconstrained, resolve_facets,
};
use super::history::{DirtyTipPolicy, ResolvedHistory, resolve_history};
use super::load::{
    RunIndex, WorkerFold, facet_filtered_candidates, fold_runs_chunked, load_objects_concurrently,
};
use super::selection::Selection;
use super::window::{auto_mode, before_since_cutoff, resolve_since, since_cutoff_reason};
use crate::AnalyzeError;

/// The data an analysis (or listing) draws on, plus the bookkeeping needed to
/// explain an empty outcome and warn about ephemeral data.
pub(crate) struct SelectedDataSet {
    /// The reconstructed series for the in-window runs, built with the caller's
    /// series filter and ordered by git topology. Pre-blessing: the caller applies
    /// blessings (history mode) or leaves them unapplied (branch, listings).
    pub(crate) series: Vec<Series>,
    /// Compact per-set, per-commit run tallies, standing in for a retained copy of
    /// every loaded object (which a large history cannot afford to keep resident).
    pub(crate) run_index: RunIndex,
    /// How many facet-matching candidates existed before topology filtering.
    pub(crate) candidate_count: usize,
    /// Why candidates were excluded, for the empty-history hint.
    pub(crate) tally: ExclusionTally,
    /// Whether a dirty run was admitted solely by the base-branch dirty-tree
    /// exception (triggers the ephemeral-data warning).
    pub(crate) included_dirty_base_exception: bool,
    /// The target ref the timeline was resolved against (for diagnostics).
    pub(crate) target_ref: String,
    /// The resolved discriminant-set query (the effective, possibly auto-detected
    /// engine / target-triple / machine-key facets), so an empty outcome can name
    /// the exact partition it searched.
    pub(crate) facets: DiscriminantSetQuery,
    /// Subject line of each in-history commit that has one, so `examine` can label
    /// each data point with what its commit changed. A commit absent here has an
    /// empty subject; only `examine` reads this.
    pub(crate) commit_subjects: HashMap<String, String>,
    /// The full commit ID of the analyzed tip commit (the resolved `--context`/HEAD),
    /// carried into the report so it names the exact commit the findings describe.
    pub(crate) tip_commit: String,
    /// Whether the working tree carried uncommitted changes when the analysis ran;
    /// the report annotates the tip `+ uncommitted changes` when set.
    pub(crate) tip_dirty: bool,
    /// The resolved analysis mode, auto-detected from the git topology.
    pub(crate) mode: AnalysisMode,
    /// First-parent topological index of the merge-base, used by branch mode to
    /// split base-side history from the branch's own commits.
    pub(crate) merge_base_index: Option<usize>,
    /// Blessings recorded on in-window commits, grouped by discriminant set. Each
    /// entry pairs the blessed commit's first-parent topological index and its
    /// committer date (from topology, for the report anchor) with the record;
    /// history-mode re-baselining picks, per series, the latest matching blessing.
    /// Empty in branch mode (it ignores blessings).
    pub(crate) blessings: HashMap<DiscriminantSet, Vec<BlessingPlacement>>,
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
    auto: &AutoFacets,
    now: Timestamp,
    reporter: &dyn Reporter,
    spawner: &Spawner,
) -> Result<SelectedDataSet, AnalyzeError>
where
    G: GitHistory,
    S: Storage + Clone + 'static,
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
        tip_commit,
        tip_dirty,
        order,
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

    // The always-on effective-selection announcement: one line, printed regardless
    // of `--verbose`, naming the resolved (possibly auto-detected) partition, base
    // branch, and look-back window a plain run would otherwise resolve silently.
    reporter.announce(&effective_selection_summary(
        &facets,
        &base_name,
        selection.base.is_none(),
        since,
        selection.since.is_some(),
        mode,
    ));

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
    let series = builder.finish();
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

    // Load the blessing sidecars on in-window commits into a per-set map. A
    // blessing on a commit outside the analyzed history (or that fails to parse) is
    // irrelevant and skipped. Branch mode ignores blessings entirely, so
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
            let text = String::from_utf8(bytes).map_err(|error| AnalyzeError::Analyze {
                message: format!("stored blessing {key} is not valid UTF-8: {error}"),
            })?;
            BlessingRecord::from_json(&text).map_err(|error| AnalyzeError::Analyze {
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
        },
        included_dirty_base_exception,
        target_ref,
        facets,
        commit_subjects,
        tip_commit,
        tip_dirty,
        mode,
        merge_base_index,
        blessings,
    })
}

/// Builds the always-on, one-line summary of a run's effective selection: the
/// discriminant partition it searched (naming auto-detected facets), the base
/// branch it split history against, and the resolved look-back cutoff.
///
/// Emitted to standard error regardless of `--verbose` so a plain run never hides
/// a value it auto-detected or defaulted. `base_auto` marks the base as
/// auto-detected (no explicit `--base`); `since_explicit` selects the wording for
/// why the `--since` cutoff is what it is.
fn effective_selection_summary(
    facets: &DiscriminantSetQuery,
    base_name: &str,
    base_auto: bool,
    since: Option<Timestamp>,
    since_explicit: bool,
    mode: AnalysisMode,
) -> String {
    selection_announcement(
        facets,
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

/// How many facet-matching candidates were excluded, by reason.
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
/// Two empty cases are named. When *no* stored run matched the facet filters, the
/// hint names the effective (possibly auto-detected) partition it searched — so an
/// auto-detected target-triple / machine-key that simply does not match the stored
/// data explains itself — and distinguishes a genuinely empty project from a
/// missed partition. When runs matched the facets but topology or the `--since`
/// cutoff excluded them all, the hint breaks down the dominant exclusion reasons.
///
/// Returns `None` when at least one run was loaded.
pub(crate) fn empty_history_hint(
    loaded_is_empty: bool,
    candidate_count: usize,
    target_ref: &str,
    tally: ExclusionTally,
    facets: &DiscriminantSetQuery,
) -> Option<String> {
    if !loaded_is_empty {
        return None;
    }

    if candidate_count == 0 {
        // No stored run matched the facets at all. Either the project holds no runs
        // yet, or an auto-detected facet points at a partition nothing was recorded
        // under. Name the partition so the second case is not mistaken for the first.
        if facets_are_unconstrained(facets) {
            return Some(
                "No benchmark runs are stored for this project yet. Record some with a \
                 `collect` (or `backfill`) run, then try again."
                    .to_owned(),
            );
        }
        return Some(format!(
            "No stored runs matched the current selection ({}). Nothing has been \
             collected for this discriminant partition yet, or an auto-detected facet \
             does not match the stored data. Pass --target-triple all / --machine-key all \
             to widen the search, or run `list discriminants` to see which partitions \
             hold data.",
            describe_effective_facets(facets)
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
    use cbh_detect::FacetFilter;
    use nonempty::nonempty;

    use super::*;

    /// A facet query with no auto-detected or explicit constraints, for the
    /// exclusion-reason cases whose hint text does not depend on the facets.
    fn unconstrained_facets() -> DiscriminantSetQuery {
        DiscriminantSetQuery {
            engine: FacetFilter::All,
            target_triple: FacetFilter::All,
            machine_key: FacetFilter::All,
        }
    }

    #[test]
    fn empty_history_hint_explains_only_when_runs_were_excluded() {
        let no_exclusions = ExclusionTally {
            outside_history: 0,
            dirty_base: 0,
            since: 0,
        };
        let facets = unconstrained_facets();
        // Runs were actually loaded → no hint regardless of candidate count.
        assert_eq!(
            empty_history_hint(false, 3, "master", no_exclusions, &facets),
            None
        );

        let tally = ExclusionTally {
            outside_history: 2,
            dirty_base: 1,
            since: 4,
        };
        let hint = empty_history_hint(true, 7, "master", tally, &facets).unwrap();
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
        let hint = empty_history_hint(true, 3, "master", dirty_only, &facets).unwrap();
        assert!(hint.contains("dirty (uncommitted-tree)"), "{hint}");
        assert!(!hint.contains("outside"), "{hint}");
        assert!(!hint.contains("--since cutoff"), "{hint}");

        // Only the outside-history reason is present here (dirty omitted).
        let outside_only = ExclusionTally {
            outside_history: 2,
            dirty_base: 0,
            since: 0,
        };
        let hint = empty_history_hint(true, 2, "master", outside_only, &facets).unwrap();
        assert!(hint.contains("outside master"), "{hint}");
        assert!(!hint.contains("dirty (uncommitted-tree)"), "{hint}");
        assert!(!hint.contains("--since cutoff"), "{hint}");

        // Only the since reason is present here.
        let since_only = ExclusionTally {
            outside_history: 0,
            dirty_base: 0,
            since: 5,
        };
        let hint = empty_history_hint(true, 5, "master", since_only, &facets).unwrap();
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

        // Unconstrained facets that matched nothing → a genuinely empty project.
        let hint =
            empty_history_hint(true, 0, "master", no_exclusions, &unconstrained_facets()).unwrap();
        assert!(hint.contains("No benchmark runs are stored"), "{hint}");
        assert!(hint.contains("collect"), "{hint}");
        // It must not misdescribe an empty project as a missed partition.
        assert!(!hint.contains("auto-detected facet"), "{hint}");

        // Auto-detected facets that matched nothing → name the searched partition so
        // the user learns which auto-detected values missed.
        let auto = DiscriminantSetQuery {
            engine: FacetFilter::All,
            target_triple: FacetFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: FacetFilter::Auto("abcd".to_owned()),
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
        let facets = DiscriminantSetQuery {
            engine: FacetFilter::All,
            target_triple: FacetFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: FacetFilter::Auto("abcd".to_owned()),
        };
        let since = Timestamp::from_second(1_700_000_000).unwrap();
        // History mode, auto-detected base, default look-back: every defaulted value
        // is named and marked.
        let summary = effective_selection_summary(
            &facets,
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
        let facets = DiscriminantSetQuery {
            engine: FacetFilter::Explicit(nonempty!["criterion".to_owned()]),
            target_triple: FacetFilter::All,
            machine_key: FacetFilter::All,
        };
        // Branch mode, explicit base, no default cutoff: nothing is marked
        // auto-detected.
        let summary = effective_selection_summary(
            &facets,
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
}
