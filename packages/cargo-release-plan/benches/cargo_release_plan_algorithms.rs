//! Wall-clock benchmarks for deterministic in-process release-plan algorithms.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::fmt::Write as _;
use std::hint::black_box;

use cargo_release_plan::__private::{benchmark_lockfile_closure, benchmark_patch_rendering};
use criterion::{Criterion, criterion_group, criterion_main};

/// Keeps the low case above trivial fixed-cost behavior.
const LOW_LINE_COUNT: usize = 16;
/// Exposes scaling while keeping local benchmark smoke runs practical.
const HIGH_LINE_COUNT: usize = 2_048;
/// Produces distributed edits instead of one contiguous replacement.
const CHANGED_LINE_INTERVAL: usize = 8;

/// Keeps the low lockfile case representative of a dependency chain.
const LOW_PACKAGE_COUNT: usize = 8;
/// Exposes closure-walk scaling without measuring process or filesystem work.
const HIGH_PACKAGE_COUNT: usize = 512;

criterion_group!(benches, patch_rendering, lockfile_closure);
criterion_main!(benches);

fn patch_rendering(c: &mut Criterion) {
    let mut group = c.benchmark_group("cargo_release_plan_algorithms/patch_rendering");

    for (name, line_count) in [("low", LOW_LINE_COUNT), ("high", HIGH_LINE_COUNT)] {
        let (old, new) = patch_inputs(line_count);
        group.bench_function(name, |b| {
            b.iter(|| black_box(benchmark_patch_rendering(black_box(&old), black_box(&new))));
        });
    }

    group.finish();
}

fn lockfile_closure(c: &mut Criterion) {
    let mut group = c.benchmark_group("cargo_release_plan_algorithms/lockfile_closure");

    for (name, package_count) in [("low", LOW_PACKAGE_COUNT), ("high", HIGH_PACKAGE_COUNT)] {
        let lockfile = lockfile_input(package_count);
        group.bench_function(name, |b| {
            b.iter(|| {
                black_box(benchmark_lockfile_closure(
                    black_box(&lockfile),
                    black_box("root"),
                    black_box("1.0.0"),
                ))
            });
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
