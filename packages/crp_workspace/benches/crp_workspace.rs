//! In-process workspace observation benchmarks.

#![allow(
    missing_docs,
    reason = "Benchmark entry points are maintainer tooling."
)]

use std::fmt::Write as _;
use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use crp_diag::{Discard, Verbose};
use crp_workspace::git::decode_blob_batch;
use crp_workspace::lockfile::benchmark_lockfile_closures;
use crp_workspace::manifest_document::ManifestDocuments;

::testing::set_allocator!();

/// Keeps the low lockfile case representative of a dependency chain.
const LOW_PACKAGE_COUNT: usize = 8;
/// Exposes closure-walk scaling within a microbenchmark iteration budget.
const HIGH_PACKAGE_COUNT: usize = 64;
/// Represents several binaries sharing one parsed workspace lockfile.
const CLOSURE_COUNT: usize = 16;

criterion_group!(
    benches,
    lockfile_closure,
    historical_blob_batch,
    manifest_reuse
);
criterion_main!(benches);

fn manifest_reuse(c: &mut Criterion) {
    let mut group = c.benchmark_group("crp_workspace/manifest_reuse");
    for (name, count) in [("low", LOW_PACKAGE_COUNT), ("high", HIGH_PACKAGE_COUNT)] {
        let mut text = String::from("[workspace.dependencies]\n");
        for index in 0..count {
            writeln!(
                text,
                "member_{index} = {{ version = '=1.0.0', path = 'member_{index}' }}"
            )
            .expect("writing to a String cannot fail");
        }
        let mut documents = ManifestDocuments::default();
        let path = Path::new("Cargo.toml");
        let verbose = Verbose::new(false, &Discard);
        documents
            .parse(path, &text, verbose)
            .expect("generated TOML is valid");
        group.bench_function(name, |b| {
            b.iter(|| {
                black_box(
                    documents
                        .parse(path, black_box(&text), verbose)
                        .expect("the cached document has already been parsed"),
                )
            });
        });
    }
    group.finish();
}

fn historical_blob_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("crp_workspace/historical_blob_batch");
    for (name, count) in [("low", LOW_PACKAGE_COUNT), ("high", HIGH_PACKAGE_COUNT)] {
        let ids: Vec<_> = (0..count).map(|index| format!("{index:040x}")).collect();
        let ids: Vec<_> = ids.iter().map(String::as_str).collect();
        // A compact manifest-shaped blob keeps the measured operation focused on framing
        // and allocation across workspace sizes, not TOML interpretation or subprocess latency.
        let body = b"[package]\nname = 'member'\nversion = '1.0.0'\n";
        let mut output = Vec::new();
        for id in &ids {
            output.extend_from_slice(format!("{id} blob {}\n", body.len()).as_bytes());
            output.extend_from_slice(body);
            output.push(b'\n');
        }
        group.bench_function(name, |b| {
            b.iter(|| {
                black_box(
                    decode_blob_batch(black_box(&ids), black_box(&output))
                        .expect("the fixture contains one complete blob per requested identity"),
                )
            });
        });
    }
    group.finish();
}

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
