//! Concurrent object loading and the streaming `RunIndex` fold: candidate
//! enumeration, the bounded-concurrency fetch, and the per-worker tally
//! recombination that keeps a long history's runs from all being held resident.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use anyspawn::Spawner;
use cbh_detect::{
    DiscriminantSetQuery, FacetFilter, RunPoints, SeriesBuilder, balanced_chunk_sizes, worker_count,
};
use cbh_diag::{Reporter, ReporterExt, count_noun};
use cbh_model::{
    BenchmarkIdPrefix, DiscriminantSet, STORAGE_VERSION, StorageKey, parse_key, sanitize_segment,
};
use cbh_storage::{Storage, project_objects_prefix};
use futures::{StreamExt as _, TryStreamExt as _};

use super::facets::describe_facets;
use crate::AnalyzeError;

/// One commit's run tally within a discriminant set, the granularity the report
/// summaries and the `list runs` breakdown need.
#[derive(Clone, Debug)]
pub(crate) struct CommitCounts {
    /// The commit the runs were measured against (full commit ID, or a label in tests).
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

    /// Folds another index's tallies into this one.
    ///
    /// The recombination step of the parallel fold: each worker records the runs of
    /// its disjoint object chunk into its own index, and the main thread merges the
    /// per-worker indices. Because the chunks are disjoint, summing the totals and
    /// the per-`(set, commit)` clean/dirty counts reproduces the single-threaded
    /// tally exactly, independent of the order the workers' indices are merged.
    fn merge(&mut self, other: Self) {
        self.total = self.total.saturating_add(other.total);
        for (set, by_commit) in other.sets {
            let dest = self.sets.entry(set).or_default();
            for (topo_index, counts) in by_commit {
                let entry = dest.entry(topo_index).or_insert_with(|| CommitCounts {
                    commit: counts.commit.clone(),
                    clean: 0,
                    dirty: 0,
                });
                entry.clean = entry.clean.saturating_add(counts.clean);
                entry.dirty = entry.dirty.saturating_add(counts.dirty);
            }
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

    /// The oldest and newest commit that contributed a run, by first-parent
    /// topological position, as `(first, last)` full SHAs. `None` when no run was
    /// admitted. The report header uses it to state the span of analyzed history.
    // The oldest/newest tie-break is unobservable, so its comparison mutants are
    // equivalent: a first-parent topological position denotes exactly one commit, so
    // every set records the same commit ID at a given position and `<` vs `<=` (or `>` vs
    // `>=`) only ever chooses between identical strings.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn commit_span(&self) -> Option<(&str, &str)> {
        // A given first-parent position maps to exactly one commit, so every set records
        // the same commit under it: the span is simply the commit at the lowest position
        // and the one at the highest. Reading those extremes with `min_by_key`/`max_by_key`
        // keeps the ordering in the standard library rather than a hand-rolled comparison.
        let first = self
            .sets
            .values()
            .flat_map(|by_commit| by_commit.iter())
            .min_by_key(|entry| *entry.0)?
            .1
            .commit
            .as_str();
        let last = self
            .sets
            .values()
            .flat_map(|by_commit| by_commit.iter())
            .max_by_key(|entry| *entry.0)?
            .1
            .commit
            .as_str();
        Some((first, last))
    }
}

/// The recognized objects a single project listing produced.
///
/// A large history is listed once (a single [`Storage::list`] round-trip); this
/// splits its keys into the facet-selected candidates and, when requested, the
/// machine-relaxed clean-run siblings used to explain a lagging branch comparison
/// base.
pub(crate) struct CandidateListing {
    /// Objects whose discriminant set matches every facet filter — the selection the
    /// analysis (or listing) operates on.
    pub(crate) selected: Vec<(String, StorageKey)>,
    /// Potential sibling observations for branch-mode comparison-base lag
    /// classification: exact `clean.json` objects that match the engine and
    /// target-triple facets but whose machine key the selection does not cover. Empty
    /// unless siblings were requested (`collect_siblings`).
    pub(crate) siblings: Vec<(String, StorageKey)>,
}

/// Lists the stored objects under the project's partition and keeps the ones whose
/// discriminant set matches the facet filters. Shared by the topology-aware
/// selection and the discriminant listing (which needs no repository).
pub(crate) async fn facet_filtered_candidates<S: Storage>(
    storage: &S,
    project_id: &str,
    facets: &DiscriminantSetQuery,
    reporter: &dyn Reporter,
) -> Result<Vec<(String, StorageKey)>, AnalyzeError> {
    Ok(
        list_candidates(storage, project_id, facets, false, reporter)
            .await?
            .selected,
    )
}

/// Lists the project's stored objects once and partitions the recognized keys into
/// the facet-selected candidates and, when `collect_siblings` is set, the
/// machine-relaxed clean-run siblings (same engine and target triple, under a machine
/// key the selection does not cover).
///
/// Sibling discovery relaxes only the machine-key facet, so it never widens the
/// selection: the selected set is exactly what [`facet_filtered_candidates`] returns.
/// It exists solely to let branch-mode analysis discover whether a newer base-side run
/// for a lagging finding exists under a different machine key, without a second list
/// round-trip.
pub(crate) async fn list_candidates<S: Storage>(
    storage: &S,
    project_id: &str,
    facets: &DiscriminantSetQuery,
    collect_siblings: bool,
    reporter: &dyn Reporter,
) -> Result<CandidateListing, AnalyzeError> {
    // The listing prefix must use the same sanitized project segment that
    // `DiscriminantSet` writes its storage keys under. A project id containing a
    // character that sanitizes (a space, `/`, a non-ASCII letter, ...) is stored
    // mangled, so listing under the raw id would silently find an empty history.
    let project = sanitize_segment(project_id);
    let prefix = project_objects_prefix(project_id);

    reporter.if_enabled(|notes| {
        notes.note(&format!(
            "project id: {project_id} (storage segment: {project})"
        ));
        notes.note(&format!("listing stored objects under prefix {prefix}"));
        notes.note(&format!("facet filters: {}", describe_facets(facets)));
    });

    let list_started = Instant::now();
    let keys = storage.list(&prefix).await.map_err(AnalyzeError::Storage)?;
    reporter.timing("storage.list(prefix) round-trip", list_started.elapsed());
    reporter.note_with(|| format!("storage returned {}", count_noun(keys.len(), "object key")));

    // Sibling discovery keeps the engine and target-triple facets but relaxes the
    // machine key, so a run under any machine key that shares the comparable axes can
    // surface as a potential newer base-side observation.
    let sibling_query = collect_siblings.then(|| DiscriminantSetQuery {
        engine: facets.engine.clone(),
        target_triple: facets.target_triple.clone(),
        machine_key: FacetFilter::All,
    });

    let mut selected: Vec<(String, StorageKey)> = Vec::new();
    let mut siblings: Vec<(String, StorageKey)> = Vec::new();
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
        let matches_selection = facets.matches(&parsed.set);
        // A sibling is a clean run the selection does not cover but that shares the
        // comparable axes — the only base-side evidence that could explain a lagging
        // comparison base by machine-key rotation. `--machine-key all` selects every
        // key, so this stays empty and classification falls back to loaded series.
        let retained_as_sibling = sibling_query.as_ref().is_some_and(|query| {
            !matches_selection && parsed.is_clean() && query.matches(&parsed.set)
        });
        if retained_as_sibling {
            siblings.push((key.clone(), parsed.clone()));
        }
        if matches_selection {
            selected.push((key, parsed));
        } else if retained_as_sibling {
            reporter.note_with(|| {
                format!(
                    "retaining {key} as a machine-relaxed sibling: discriminant {}",
                    parsed.set
                )
            });
        } else {
            reporter.note_with(|| {
                format!(
                    "skipping {key}: discriminant {} does not match the facet filters",
                    parsed.set
                )
            });
        }
    }
    reporter.note_with(|| {
        format!(
            "{} match the facet filters",
            count_noun(selected.len(), "object")
        )
    });
    if sibling_query.is_some() {
        reporter.note_with(|| {
            format!(
                "{} retained as machine-relaxed sibling candidates",
                count_noun(siblings.len(), "clean-run object")
            )
        });
    }
    Ok(CandidateListing { selected, siblings })
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
pub(crate) async fn load_objects_concurrently<S, T, F>(
    storage: &S,
    keys: Vec<(String, StorageKey)>,
    parse: F,
) -> Result<Vec<(String, StorageKey, T)>, AnalyzeError>
where
    S: Storage,
    F: Fn(&str, Vec<u8>) -> Result<T, AnalyzeError>,
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
) -> Result<(String, StorageKey, T), AnalyzeError>
where
    S: Storage,
    F: Fn(&str, Vec<u8>) -> Result<T, AnalyzeError>,
{
    let bytes = storage.get(&key).await.map_err(AnalyzeError::Storage)?;
    let value = parse(&key, bytes)?;
    Ok((key, parsed, value))
}

/// One worker's (or the merged) folded contribution: the series builder it folded
/// its object chunk into, the run tally it recorded, and the per-object admission
/// flags for the key-ordered verbose notes.
///
/// Returned from each spawned fold task and merged on the main thread; the merged
/// value carries the same shape so the caller treats one worker and many uniformly.
pub(crate) struct WorkerFold {
    /// Series points folded from this worker's objects (compact; the parsed runs
    /// are dropped inside the worker as each is folded).
    pub(crate) builder: SeriesBuilder,
    /// Per-set, per-commit run tally for this worker's objects.
    pub(crate) run_index: RunIndex,
    /// `(storage key, admitted-by-dirty-base-exception)` per folded object, for the
    /// key-ordered verbose notes the caller emits once every worker has folded.
    pub(crate) admitted: Vec<(String, bool)>,
}

/// Loads, parses, and **folds** the in-selection survivors across CPU cores.
///
/// `ranked` is the storage-key-sorted survivor list, each carrying its
/// storage-key ordinal (`rank`). It is split into [`worker_count`] balanced
/// contiguous chunks ([`balanced_chunk_sizes`]); one spawned task per chunk
/// fetches, decompresses, parses, and folds its slice into its *own*
/// [`SeriesBuilder`] / [`RunIndex`], dropping each parsed run the moment its
/// compact points are extracted. The main thread awaits the chunks in spawn order
/// and merges their builders, run tallies, and admission lists into one
/// [`WorkerFold`]; a single [`SeriesBuilder::finish`] then sorts the combined
/// series. The merge is associative and the final sort is global, so the result is
/// identical to a single-threaded fold in storage-key order.
///
/// Folding inside each worker (rather than buffering every chunk's parsed runs and
/// folding serially on the main thread) parallelizes the fold as well as the parse:
/// a worker drops each parsed run the moment its compact points are extracted, so it
/// never buffers its chunk's parsed runs. This does **not** lower peak memory — at
/// merge time every worker's finished builder is resident at once alongside the
/// growing combined builder (~2x the compact point output transiently), which measured
/// ~5% above the buffered-parallel variant; the win it buys is the ~10% faster wall
/// time and better core use from moving the fold off the main thread. The decompress +
/// JSON parse — the CPU-dominated cost — is spread across the runtime's worker threads.
/// See the "Architecture" section of `docs/DESIGN.md` and the load section of
/// `docs/analyze.md`.
pub(crate) async fn fold_runs_chunked<S>(
    storage: &S,
    spawner: &Spawner,
    ranked: Vec<(usize, String, StorageKey)>,
    order: &Arc<HashMap<String, usize>>,
    dirty_base_exception: &Arc<HashMap<String, bool>>,
    prefixes: Arc<[BenchmarkIdPrefix]>,
) -> Result<WorkerFold, AnalyzeError>
where
    S: Storage + Clone + 'static,
{
    let total = ranked.len();
    let mut combined = WorkerFold {
        builder: SeriesBuilder::with_prefixes(Arc::clone(&prefixes)),
        run_index: RunIndex::new(),
        admitted: Vec::with_capacity(total),
    };
    if total == 0 {
        return Ok(combined);
    }
    let workers = worker_count(total);

    let mut items = ranked.into_iter();
    let mut handles = Vec::with_capacity(workers);
    for chunk_len in balanced_chunk_sizes(total, workers) {
        let chunk: Vec<(usize, String, StorageKey)> = items.by_ref().take(chunk_len).collect();
        let storage = storage.clone();
        let order = Arc::clone(order);
        let dirty_base_exception = Arc::clone(dirty_base_exception);
        let prefixes = Arc::clone(&prefixes);
        handles.push(spawner.spawn(async move {
            let mut builder = SeriesBuilder::with_prefixes(prefixes);
            let mut run_index = RunIndex::new();
            let mut admitted: Vec<(String, bool)> = Vec::with_capacity(chunk.len());
            for (rank, key, parsed) in chunk {
                let bytes = storage.get(&key).await.map_err(AnalyzeError::Storage)?;
                let text = str::from_utf8(&bytes).map_err(|error| AnalyzeError::Analyze {
                    message: format!("stored object {key} is not valid UTF-8: {error}"),
                })?;
                let run = RunPoints::from_json(text).map_err(|error| AnalyzeError::Analyze {
                    message: format!("stored object {key} is not a valid result set: {error}"),
                })?;
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
                run_index.record(&parsed.set, topo_index, &parsed.commit, dirty);
                builder.push(
                    &parsed.set,
                    topo_index,
                    dirty,
                    ordinal_of(rank),
                    &parsed.commit,
                    &run,
                );
                admitted.push((key, is_exception));
                // `run` is dropped here; only the extracted (compact) points are kept.
            }
            Ok::<WorkerFold, AnalyzeError>(WorkerFold {
                builder,
                run_index,
                admitted,
            })
        }));
    }

    for handle in handles {
        let fold = handle.await?;
        combined.builder.merge(fold.builder);
        combined.run_index.merge(fold.run_index);
        combined.admitted.extend(fold.admitted);
    }
    Ok(combined)
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cbh_model::{DiscriminantSet, Engine};

    use super::*;

    #[test]
    fn run_index_counts_runs_and_reports_emptiness() {
        let set = DiscriminantSet {
            engine: Engine::Criterion,
            target_triple: "x86_64-unknown-linux-gnu".into(),
            machine_key: "m1".into(),
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
    fn run_index_merge_sums_totals_and_per_commit_counts() {
        // Each worker records its disjoint object chunk into its own index; merging
        // the per-worker indices must reproduce the single-threaded tally exactly,
        // summing the totals and the per-(set, commit) clean/dirty counts.
        let set = DiscriminantSet {
            engine: Engine::Criterion,
            target_triple: "x86_64-unknown-linux-gnu".into(),
            machine_key: "m1".into(),
        };
        let other_set = DiscriminantSet {
            engine: Engine::Callgrind,
            target_triple: "x86_64-unknown-linux-gnu".into(),
            machine_key: "m1".into(),
        };

        // The reference index folds every run in one pass.
        let mut reference = RunIndex::new();
        reference.record(&set, 0, "c0", false);
        reference.record(&set, 0, "c0", true);
        reference.record(&set, 1, "c1", false);
        reference.record(&other_set, 0, "c0", false);

        // Two workers split the same runs; c0/set is touched by both so the merge has
        // to sum the per-commit counts rather than overwrite them.
        let mut first = RunIndex::new();
        first.record(&set, 0, "c0", false);
        first.record(&other_set, 0, "c0", false);
        let mut second = RunIndex::new();
        second.record(&set, 0, "c0", true);
        second.record(&set, 1, "c1", false);

        first.merge(second);

        assert_eq!(first.total(), reference.total());
        assert_eq!(first.runs_in_set(&set), reference.runs_in_set(&set));
        assert_eq!(
            first.runs_in_set(&other_set),
            reference.runs_in_set(&other_set)
        );

        let summarize = |index: &RunIndex| {
            index
                .sets()
                .map(|(set, by_commit)| {
                    let counts: Vec<(usize, String, usize, usize)> = by_commit
                        .iter()
                        .map(|(topo, counts)| {
                            (*topo, counts.commit.clone(), counts.clean, counts.dirty)
                        })
                        .collect();
                    (set.clone(), counts)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(summarize(&first), summarize(&reference));
    }

    #[test]
    fn commit_span_spans_the_oldest_and_newest_analyzed_commit() {
        let set = DiscriminantSet {
            engine: Engine::Criterion,
            target_triple: "x86_64-unknown-linux-gnu".into(),
            machine_key: "m1".into(),
        };
        let other_set = DiscriminantSet {
            engine: Engine::Callgrind,
            target_triple: "x86_64-unknown-linux-gnu".into(),
            machine_key: "m1".into(),
        };

        let mut index = RunIndex::new();
        assert_eq!(index.commit_span(), None, "an empty index spans nothing");

        // Record out of topological order and across two sets: the span must key off
        // the first-parent position, not the insertion order, and must consider every
        // set so the header covers the whole analyzed history.
        index.record(&set, 2, "c2", false);
        index.record(&set, 0, "c0", false);
        index.record(&other_set, 1, "c1", false);

        assert_eq!(
            index.commit_span(),
            Some(("c0", "c2")),
            "the span runs from the lowest to the highest topological position"
        );
    }

    #[test]
    fn commit_span_collapses_to_a_single_commit() {
        let set = DiscriminantSet {
            engine: Engine::Criterion,
            target_triple: "x86_64-unknown-linux-gnu".into(),
            machine_key: "m1".into(),
        };
        let mut index = RunIndex::new();
        index.record(&set, 0, "solo", false);
        index.record(&set, 0, "solo", true);

        assert_eq!(
            index.commit_span(),
            Some(("solo", "solo")),
            "a single analyzed commit is both ends of the span"
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

    /// A discriminant-set query pinned to one engine/triple and machine key.
    fn query(engine: &str, triple: &str, machine: &str) -> DiscriminantSetQuery {
        DiscriminantSetQuery {
            engine: FacetFilter::Auto(engine.to_owned()),
            target_triple: FacetFilter::Auto(triple.to_owned()),
            machine_key: FacetFilter::Auto(machine.to_owned()),
        }
    }

    /// Lists candidates and siblings from `storage`, unwrapping the result.
    fn list(
        storage: &cbh_storage::MemoryStorage,
        facets: &DiscriminantSetQuery,
    ) -> CandidateListing {
        list_reported(storage, facets).0
    }

    /// Lists candidates and siblings, returning the recording reporter so a test can
    /// inspect the per-key diagnostics.
    fn list_reported(
        storage: &cbh_storage::MemoryStorage,
        facets: &DiscriminantSetQuery,
    ) -> (CandidateListing, cbh_diag::RecordingReporter) {
        let reporter = cbh_diag::RecordingReporter::new();
        let listing =
            futures::executor::block_on(list_candidates(storage, "folo", facets, true, &reporter))
                .unwrap();
        (listing, reporter)
    }

    #[test]
    fn sibling_listing_relaxes_only_the_machine_key() {
        // The selection covers only m1, but a clean run under m2 that shares the
        // engine and triple is retained as a machine-relaxed sibling — never as a
        // selected candidate.
        let storage = cbh_storage::MemoryStorage::new();
        let put = |key: &str| {
            futures::executor::block_on(storage.put(key, b"{}")).unwrap();
        };
        put("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/c0/clean.json");
        put("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m2/c0/clean.json");

        let listing = list(
            &storage,
            &query("callgrind", "x86_64-unknown-linux-gnu", "m1"),
        );
        assert_eq!(listing.selected.len(), 1, "only m1 is selected");
        assert_eq!(listing.selected.first().unwrap().1.set.machine_key, "m1");
        assert_eq!(listing.siblings.len(), 1, "m2 is a sibling");
        assert_eq!(listing.siblings.first().unwrap().1.set.machine_key, "m2");
    }

    #[test]
    fn a_retained_sibling_is_not_diagnosed_as_skipped() {
        // A machine-relaxed sibling is kept for lag classification, so its verbose note
        // must say it was retained, never that it was skipped for not matching the
        // facets. A genuinely non-matching key (a dirty run) still reads as skipped.
        let storage = cbh_storage::MemoryStorage::new();
        let put = |key: &str| {
            futures::executor::block_on(storage.put(key, b"{}")).unwrap();
        };
        let selected = "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/c0/clean.json";
        let sibling = "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m2/c0/clean.json";
        let skipped = "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m3/c0/dirty-5.json";
        put(selected);
        put(sibling);
        put(skipped);

        let (listing, reporter) = list_reported(
            &storage,
            &query("callgrind", "x86_64-unknown-linux-gnu", "m1"),
        );
        assert_eq!(listing.selected.len(), 1);
        assert_eq!(listing.siblings.len(), 1);

        assert!(
            reporter.contains(&format!("retaining {sibling} as a machine-relaxed sibling")),
            "the sibling is reported as retained: {:?}",
            reporter.notes()
        );
        assert!(
            !reporter.contains(&format!("skipping {sibling}")),
            "the retained sibling is never reported as skipped: {:?}",
            reporter.notes()
        );
        assert!(
            reporter.contains(&format!("skipping {skipped}")),
            "a genuinely non-matching key is still reported as skipped: {:?}",
            reporter.notes()
        );
        assert!(
            !reporter.contains(&format!("retaining {skipped}")),
            "a non-sibling is never reported as retained: {:?}",
            reporter.notes()
        );
    }

    #[test]
    fn sibling_listing_keeps_only_exact_clean_runs_under_other_engines_or_triples() {
        // A sibling must be an exact clean.json under the same engine and triple. A
        // dirty snapshot, a blessing sidecar, and a run under a different engine or
        // triple are all rejected even though their machine key differs from m1.
        let storage = cbh_storage::MemoryStorage::new();
        let put = |key: &str| {
            futures::executor::block_on(storage.put(key, b"{}")).unwrap();
        };
        put("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m2/c0/clean.json");
        put("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m2/c0/dirty-5.json");
        put("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m2/c0/bless-5.json");
        put("v1/folo/objects/criterion/x86_64-unknown-linux-gnu/m2/c0/clean.json");
        put("v1/folo/objects/callgrind/aarch64-apple-darwin/m2/c0/clean.json");

        let listing = list(
            &storage,
            &query("callgrind", "x86_64-unknown-linux-gnu", "m1"),
        );
        assert!(listing.selected.is_empty(), "nothing matches m1");
        assert_eq!(
            listing.siblings.len(),
            1,
            "only the exact clean run under the same engine and triple is a sibling"
        );
        let sibling = &listing.siblings.first().unwrap().1;
        assert!(sibling.is_clean());
        assert_eq!(sibling.set.engine, Engine::Callgrind);
        assert_eq!(sibling.set.target_triple, "x86_64-unknown-linux-gnu");
    }

    #[test]
    fn sibling_listing_is_empty_without_collection() {
        // The sibling query commands do not need siblings, so `collect_siblings =
        // false` keeps the extra list empty regardless of what the partition holds.
        let storage = cbh_storage::MemoryStorage::new();
        futures::executor::block_on(storage.put(
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m2/c0/clean.json",
            b"{}",
        ))
        .unwrap();

        let listing = futures::executor::block_on(list_candidates(
            &storage,
            "folo",
            &query("callgrind", "x86_64-unknown-linux-gnu", "m1"),
            false,
            &cbh_diag::RecordingReporter::new(),
        ))
        .unwrap();
        assert!(listing.siblings.is_empty());
    }
}
