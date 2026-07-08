//! `select_dataset`: resolve the git timeline, enumerate and fold the in-selection
//! objects into a `SelectedDataSet`, and explain an empty outcome.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyspawn::Spawner;
use cbh_analysis::{AnalysisMode, BlessingPlacement, Series, SeriesFilter, StorageKey};
use cbh_config::Config;
use cbh_diag::{Reporter, ReporterExt, count_noun};
use cbh_git::GitHistory;
use cbh_model::{BenchmarkIdPrefix, BlessingRecord, DiscriminantSet};
use cbh_run::RunError;
use cbh_storage::Storage;
use jiff::Timestamp;

use super::facets::{AutoFacets, resolve_facets};
use super::history::{DirtyTipPolicy, ResolvedHistory, resolve_history};
use super::load::{
    RunIndex, WorkerFold, facet_filtered_candidates, fold_runs_chunked, load_objects_concurrently,
};
use super::selection::Selection;
use super::window::{
    WindowEdge, auto_mode, parse_until, resolve_since, since_cutoff_reason, window_excludes,
};

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
) -> Result<SelectedDataSet, RunError>
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
    let until = parse_until(selection.until, now)?;
    reporter.note_with(|| {
        format!(
            "until cutoff: {}",
            until.map_or_else(|| "none".to_owned(), |until| until.to_string())
        )
    });

    // Tally why candidates do not enter the analysis, so a `0 runs` outcome can
    // explain itself (via `--verbose` per object, and via a summary hint when
    // candidates existed but none were admitted).
    let candidate_count = candidates.len();
    let mut excluded_outside_history = 0_usize;
    let mut excluded_dirty_base = 0_usize;
    let mut excluded_since = 0_usize;
    let mut excluded_until = 0_usize;

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
         {excluded_dirty_base} dirty-on-base, {excluded_since} before --since, \
         {excluded_until} after --until)",
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
        commit_subjects,
        tip_commit,
        tip_dirty,
        mode,
        merge_base_index,
        blessings,
    })
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
    /// Effective time is after the `--until` cutoff.
    until: usize,
}

/// Builds a diagnostic hint for the case where stored runs matched the facet
/// filters but none entered the analysis, so the empty outcome explains itself.
///
/// Returns `None` when at least one run was loaded, or when there were no
/// candidates at all (a genuinely empty history needs no special explanation).
pub(crate) fn empty_history_hint(
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

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
}
