//! In-process released-content patch benchmarks.

#![allow(
    missing_docs,
    reason = "Benchmark entry points are maintainer tooling."
)]

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use crp_versioning::__private::benchmark_patch_rendering;

// This explicit target ships, so its allocator cannot depend on the unpublished testing helper.
#[cfg(not(miri))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Keeps the low case above trivial fixed-cost behavior.
const LOW_LINE_COUNT: usize = 16;
/// Exposes distributed-edit scaling while keeping full sampling millisecond-scale.
const HIGH_LINE_COUNT: usize = 512;
/// Produces distributed edits instead of one contiguous replacement.
const CHANGED_LINE_INTERVAL: usize = 8;

criterion_group!(benches, patch_rendering);
criterion_main!(benches);

fn patch_rendering(c: &mut Criterion) {
    // Identifiers name the application workload independently of its implementation partition.
    let mut group = c.benchmark_group("cargo_release_plan_algorithms/patch_rendering");
    for (name, line_count) in [("low", LOW_LINE_COUNT), ("high", HIGH_LINE_COUNT)] {
        let (old, new) = patch_inputs(line_count);
        group.bench_function(name, |b| {
            b.iter(|| black_box(benchmark_patch_rendering(black_box(&old), black_box(&new))));
        });
    }
    group.finish();
}

fn patch_inputs(line_count: usize) -> (String, String) {
    let mut old = String::new();
    let mut new = String::new();
    for index in 0..line_count {
        writeln!(old, "unchanged context {index}").expect("writing to String");
        if index.is_multiple_of(CHANGED_LINE_INTERVAL) {
            writeln!(new, "changed context {index}").expect("writing to String");
        } else {
            writeln!(new, "unchanged context {index}").expect("writing to String");
        }
    }
    (old, new)
}
