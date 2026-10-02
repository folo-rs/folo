//! Bounded preparation stages, separate from detector and codec workloads.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::num::NonZero;

use cbh_analyze::benchmarks::{
    CandidateFixture, FoldFixture, HIGH_BENCHMARKS, HIGH_CANDIDATE_BATCHES, HIGH_COMMITS,
    LOW_BENCHMARKS, LOW_CANDIDATE_BATCHES, LOW_COMMITS, MERGE_WORKERS,
};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

fn candidates(c: &mut Criterion) {
    let mut group = c.benchmark_group("cbh_analyze_preparation/candidates");
    for batches in [LOW_CANDIDATE_BATCHES, HIGH_CANDIDATE_BATCHES] {
        let fixture = CandidateFixture::new(batches);
        group.bench_function(format!("filter/{}-keys", fixture.key_count()), |b| {
            b.iter_batched(
                || fixture.keys(),
                |keys| black_box(fixture.select(black_box(keys))),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn fold(c: &mut Criterion) {
    let mut group = c.benchmark_group("cbh_analyze_preparation/fold");
    // Share the low case between independent history/suite scaling comparisons.
    // The uneven multi-worker case measures recombination, not parallel speedup:
    // every task executes inline through the existing synchronous spawner.
    for (commits, benchmarks, workers) in [
        (LOW_COMMITS, LOW_BENCHMARKS, NonZero::<usize>::MIN),
        (HIGH_COMMITS, LOW_BENCHMARKS, NonZero::<usize>::MIN),
        (LOW_COMMITS, HIGH_BENCHMARKS, NonZero::<usize>::MIN),
        (LOW_COMMITS, LOW_BENCHMARKS, MERGE_WORKERS),
    ] {
        let fixture = FoldFixture::new(commits, benchmarks, workers);
        let name = format!("{commits}-commits/{benchmarks}-benchmarks/{workers}-workers");
        group.bench_function(name, |b| {
            b.iter_batched(
                || fixture.input(),
                |input| black_box(fixture.fold(black_box(input), black_box(workers))),
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, candidates, fold);
criterion_main!(benches);

::testing::set_allocator!();
