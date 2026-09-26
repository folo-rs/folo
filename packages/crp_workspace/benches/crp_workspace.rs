//! In-process installation-closure benchmarks.

#![allow(
    missing_docs,
    reason = "Benchmark entry points are maintainer tooling."
)]

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use crp_workspace::__private::benchmark_lockfile_closures;

// This explicit target ships, so its allocator cannot depend on the unpublished testing helper.
#[cfg(not(miri))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Keeps the low lockfile case representative of a dependency chain.
const LOW_PACKAGE_COUNT: usize = 8;
/// Exposes closure-walk scaling within a microbenchmark iteration budget.
const HIGH_PACKAGE_COUNT: usize = 64;
/// Represents several binaries sharing one parsed workspace lockfile.
const CLOSURE_COUNT: usize = 16;

criterion_group!(benches, lockfile_closure);
criterion_main!(benches);

fn lockfile_closure(c: &mut Criterion) {
    // Identifiers name the application workload independently of its implementation partition.
    let mut group = c.benchmark_group("cargo_release_plan_algorithms/lockfile_closure");
    for (name, package_count) in [("low", LOW_PACKAGE_COUNT), ("high", HIGH_PACKAGE_COUNT)] {
        let lockfile = lockfile_input(package_count);
        group.bench_function(name, |b| {
            b.iter(|| {
                black_box(benchmark_lockfile_closures(
                    black_box(&lockfile),
                    black_box("root"),
                    black_box("1.0.0"),
                    black_box(CLOSURE_COUNT),
                ))
            });
        });
    }
    group.finish();
}

fn lockfile_input(package_count: usize) -> String {
    let mut text = String::from(
        "version = 4\n\n[[package]]\nname = \"root\"\nversion = \"1.0.0\"\n\
         dependencies = [\"dependency-0\"]\n",
    );
    for index in 0..package_count {
        writeln!(text, "\n[[package]]").expect("writing to String");
        writeln!(text, "name = \"dependency-{index}\"").expect("writing to String");
        writeln!(text, "version = \"1.0.0\"").expect("writing to String");
        writeln!(text, "source = \"registry+https://example.invalid/index\"")
            .expect("writing to String");
        if let Some(next) = index.checked_add(1).filter(|next| *next < package_count) {
            writeln!(text, "dependencies = [\"dependency-{next}\"]").expect("writing to String");
        }
    }
    text
}
