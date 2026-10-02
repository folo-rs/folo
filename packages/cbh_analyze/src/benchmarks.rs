//! In-memory preparation workloads shared by the benchmark executables.
//!
//! These fixtures exercise production selection and folding, not detection or
//! compression. See the preparation-benchmark section of `docs/implementation.md`.

#![cfg_attr(coverage_nightly, coverage(off))]
#![allow(
    clippy::arithmetic_side_effects,
    clippy::missing_panics_doc,
    clippy::unwrap_used,
    reason = "Bounded benchmark fixtures assert their expected shape outside measurement"
)]
#![allow(
    missing_debug_implementations,
    reason = "Opaque benchmark fixtures and outputs are not a diagnostic API"
)]

use std::collections::HashMap;
use std::num::NonZero;
use std::sync::Arc;

use anyspawn::Spawner;
use cbh_detect::testing::synchronous_spawner;
use cbh_detect::{DiscriminantFilter, DiscriminantSetQuery, balanced_chunk_sizes, worker_count};
use cbh_diag::StderrReporter;
use cbh_model::{BenchmarkIdPrefix, Engine, MetricKind, StorageKey, parse_key};
use cbh_storage::{MemoryStorage, Storage};
use futures::FutureExt as _;
use serde_json::json;

use crate::load::{CandidateListing, WorkerFold, filter_candidates, fold_runs_chunked};

/// Enough mixed keys to exercise each admission branch repeatedly.
pub const LOW_CANDIDATE_BATCHES: usize = 8;
/// Scales only the listed-object population, retaining the same admission ratios.
pub const HIGH_CANDIDATE_BATCHES: usize = 64;
/// A short history with repeated observations and topology distinct from key order.
pub const LOW_COMMITS: usize = 4;
/// Scales history length independently of the benchmark population.
pub const HIGH_COMMITS: usize = 16;
/// A small suite with repeated identities across stored runs.
pub const LOW_BENCHMARKS: usize = 4;
/// Scales the suite independently of history length.
pub const HIGH_BENCHMARKS: usize = 16;
/// Uneven chunks split a commit's clean/dirty pair across workers in the low fixture.
pub const MERGE_WORKERS: NonZero<usize> = NonZero::new(3).unwrap();
/// Independent comparable partitions keep folding from degenerating to a single set.
const MACHINES: [&str; 2] = ["m0", "m1"];
/// A committed observation and an uncommitted observation share each topology position.
const SNAPSHOTS: [&str; 2] = ["clean", "dirty-1"];
/// One batch includes selected clean/dirty runs, a sibling, and distinct exclusion paths.
const KEYS_PER_BATCH: usize = 8;
/// A stable, positive metric value makes lost or corrupted observations observable.
const METRIC_VALUE: f64 = 100.0;

/// A listed project's controlled mixture of admitted and excluded object keys.
pub struct CandidateFixture {
    keys: Vec<String>,
    query: DiscriminantSetQuery,
    batches: usize,
}

// Benchmark setup and smoke assertions are not library behavior. The production
// filter remains covered by its own mutation-tested unit tests.
#[cfg_attr(test, mutants::skip)]
impl CandidateFixture {
    /// Builds and verifies a mixed listing before measurement.
    #[must_use]
    pub fn new(batches: usize) -> Self {
        assert!(batches > 0);
        let mut keys = Vec::with_capacity(batches * KEYS_PER_BATCH);
        for commit in 0..batches {
            let prefix = "v1/folo/objects";
            for (engine, triple, machine, snapshot) in [
                ("callgrind", "x86_64-unknown-linux-gnu", "m0", "clean"),
                ("callgrind", "x86_64-unknown-linux-gnu", "m0", "dirty-1"),
                ("callgrind", "x86_64-unknown-linux-gnu", "m1", "clean"),
                ("callgrind", "x86_64-unknown-linux-gnu", "m1", "dirty-1"),
                ("criterion", "x86_64-unknown-linux-gnu", "m0", "clean"),
                ("callgrind", "aarch64-unknown-linux-gnu", "m0", "clean"),
            ] {
                keys.push(format!(
                    "{prefix}/{engine}/{triple}/{machine}/c{commit:04}/{snapshot}.json"
                ));
            }
            keys.push(format!("{prefix}/unrecognized-{commit}.json"));
            keys.push(format!("{prefix}/not-json-{commit}.txt"));
        }
        let fixture = Self {
            keys,
            query: DiscriminantSetQuery {
                engine: DiscriminantFilter::Auto("callgrind".into()),
                target_triple: DiscriminantFilter::Auto("x86_64-unknown-linux-gnu".into()),
                machine_key: DiscriminantFilter::Auto("m0".into()),
            },
            batches,
        };
        fixture.verify();
        fixture
    }

    /// Copies consumed input outside the measured filtering operation.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.keys.clone()
    }

    /// The number of listed keys, including rejected objects.
    #[must_use]
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// Runs production filtering with machine-relaxed sibling discovery.
    #[must_use]
    pub fn select(&self, keys: Vec<String>) -> CandidateOutput {
        CandidateOutput {
            listing: filter_candidates(keys, &self.query, true, &StderrReporter::new(false)),
        }
    }

    fn verify(&self) {
        let CandidateOutput { listing } = self.select(self.keys());
        assert_eq!(self.key_count(), self.batches * KEYS_PER_BATCH);
        assert_eq!(listing.selected.len(), self.batches * SNAPSHOTS.len());
        assert_eq!(listing.siblings.len(), self.batches);
        let selected: Vec<_> = listing.selected.iter().map(|(key, _)| key).collect();
        let siblings: Vec<_> = listing.siblings.iter().map(|(key, _)| key).collect();
        let expected_selected: Vec<_> = self
            .keys
            .as_chunks::<KEYS_PER_BATCH>()
            .0
            .iter()
            .flat_map(|batch| batch.iter().take(SNAPSHOTS.len()))
            .collect();
        let expected_siblings: Vec<_> = self
            .keys
            .as_chunks::<KEYS_PER_BATCH>()
            .0
            .iter()
            .map(|batch| batch.get(SNAPSHOTS.len()).unwrap())
            .collect();
        assert_eq!(selected, expected_selected);
        assert_eq!(siblings, expected_siblings);
        for (key, parsed) in listing.selected.iter().chain(&listing.siblings) {
            assert_eq!(*parsed, parse_key(key).unwrap());
        }
    }
}

/// Owns the real filter result so the harness consumes and drops it without projecting it.
pub struct CandidateOutput {
    listing: CandidateListing,
}

/// Stored JSON and explicit topology for the production streaming fold.
///
/// All storage futures and spawned tasks complete inline. The measured operation
/// includes the fake's key validation, map lookup and byte copy, not storage latency.
pub struct FoldFixture {
    storage: MemoryStorage,
    spawner: Spawner,
    input: FoldInput,
    order: Arc<HashMap<String, usize>>,
    exceptions: Arc<HashMap<String, bool>>,
    prefixes: Arc<[BenchmarkIdPrefix]>,
    commits: usize,
    benchmarks: usize,
}

// Only benchmark fixtures live here; mutations target the production fold instead.
#[cfg_attr(test, mutants::skip)]
impl FoldFixture {
    /// Builds and verifies a bounded history with a prefix-excluded result in every run.
    #[must_use]
    pub fn new(commits: usize, benchmarks: usize, workers: NonZero<usize>) -> Self {
        assert!(commits > 0);
        assert!(benchmarks > 0);
        let storage = MemoryStorage::new();
        let results: Vec<_> = (0..benchmarks)
            .map(|benchmark| {
                json!({
                    "id": { "segments": ["keep", format!("bench-{benchmark:04}")] },
                    "metrics": [{ "kind": "instruction_count", "value": METRIC_VALUE }]
                })
            })
            .chain([json!({
                "id": { "segments": ["excluded"] },
                "metrics": [{ "kind": "instruction_count", "value": METRIC_VALUE }]
            })])
            .collect();
        let bytes = json!({ "results": results }).to_string().into_bytes();
        let mut order = HashMap::new();
        for commit in 0..commits {
            let name = format!("c{commit:04}");
            // Reverse key order so finish must restore topology, not insertion order.
            order.insert(name.clone(), commits - commit - 1);
            for machine in MACHINES {
                for snapshot in SNAPSHOTS {
                    let key = format!(
                        "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/\
                         {machine}/{name}/{snapshot}.json"
                    );
                    storage.put(&key, &bytes).now_or_never().unwrap().unwrap();
                }
            }
        }
        let ranked = storage
            .keys()
            .into_iter()
            .enumerate()
            .map(|(rank, key)| {
                let parsed = parse_key(&key).unwrap();
                (rank, key, parsed)
            })
            .collect();
        let fixture = Self {
            storage,
            spawner: synchronous_spawner(),
            input: FoldInput { ranked },
            order: Arc::new(order),
            // The newest topology position is a dirty-base admission exception.
            exceptions: Arc::new(HashMap::from([("c0000".into(), true)])),
            prefixes: Arc::from([BenchmarkIdPrefix::new("keep/").unwrap()]),
            commits,
            benchmarks,
        };
        if workers.get() > 1 {
            let worker_count = worker_count(fixture.input.ranked.len(), workers);
            let mut boundary = 0;
            let overlaps_commit = balanced_chunk_sizes(fixture.input.ranked.len(), worker_count)
                .take(worker_count - 1)
                .any(|chunk_len| {
                    boundary += chunk_len;
                    let before = &fixture.input.ranked.get(boundary - 1).unwrap().2;
                    let after = &fixture.input.ranked.get(boundary).unwrap().2;
                    before.set == after.set && before.commit == after.commit
                });
            assert!(overlaps_commit);
        }
        fixture.verify(fixture.fold(fixture.input(), workers));
        fixture
    }

    /// Copies consumed keys outside measurement; stored payloads remain shared.
    #[must_use]
    pub fn input(&self) -> FoldInput {
        self.input.clone()
    }

    /// Runs the real fetch/parse/fold/recombine path without a runtime or real adapter.
    #[must_use]
    pub fn fold(&self, input: FoldInput, workers: NonZero<usize>) -> FoldOutput {
        let fold = fold_runs_chunked(
            &self.storage,
            &self.spawner,
            workers,
            input.ranked,
            &self.order,
            &self.exceptions,
            Arc::clone(&self.prefixes),
        )
        .now_or_never()
        .unwrap()
        .unwrap();
        FoldOutput { fold }
    }

    fn verify(&self, output: FoldOutput) {
        let WorkerFold {
            builder,
            run_index,
            admitted,
        } = output.fold;
        let runs_per_set = self.commits * SNAPSHOTS.len();
        assert_eq!(run_index.total(), runs_per_set * MACHINES.len());
        assert_eq!(run_index.sets().count(), MACHINES.len());
        let first = format!("c{:04}", self.commits - 1);
        assert_eq!(run_index.commit_span(), Some((first.as_str(), "c0000")));
        for (set, counts) in run_index.sets() {
            assert_eq!(run_index.runs_in_set(set), runs_per_set);
            assert_eq!(counts.len(), self.commits);
            for (topo, counts) in counts {
                assert_eq!(counts.commit, format!("c{:04}", self.commits - topo - 1));
                assert_eq!((counts.clean, counts.dirty), (1, 1));
            }
        }
        let expected_admitted: Vec<_> = self
            .input
            .ranked
            .iter()
            .map(|(_, key, parsed)| (key.clone(), parsed.is_dirty() && parsed.commit == "c0000"))
            .collect();
        assert_eq!(admitted, expected_admitted);
        let series = builder.finish();
        assert_eq!(series.len(), MACHINES.len() * self.benchmarks);
        for series in series {
            assert_eq!(series.set.engine, Engine::Callgrind);
            assert!(MACHINES.contains(&series.set.machine_key.as_str()));
            assert_eq!(series.kind, MetricKind::InstructionCount);
            assert!(series.id.qualified().starts_with("keep/bench-"));
            assert_eq!(series.points.len(), runs_per_set);
            for (position, point) in series.points.iter().enumerate() {
                let topo = position.checked_div(SNAPSHOTS.len()).unwrap();
                let dirty = position % SNAPSHOTS.len() != 0;
                let commit = format!("c{:04}", self.commits - topo - 1);
                assert_eq!(point.topo_index, topo);
                assert_eq!(point.dirty, dirty);
                assert_eq!(point.commit.as_deref(), Some(commit.as_str()));
                assert_eq!(point.value.to_bits(), METRIC_VALUE.to_bits());
                let expected_rank = self.input.ranked.iter().find(|(_, _, parsed)| {
                    parsed.set == series.set
                        && parsed.commit == commit
                        && parsed.is_dirty() == dirty
                });
                assert_eq!(
                    point.object_ordinal,
                    u32::try_from(expected_rank.unwrap().0).unwrap()
                );
            }
        }
    }
}

/// Owned admission inputs prepared separately from folding.
#[derive(Clone)]
pub struct FoldInput {
    ranked: Vec<(usize, String, StorageKey)>,
}

/// The unprojected production fold, including the series builder and run tallies.
pub struct FoldOutput {
    fold: WorkerFold,
}
