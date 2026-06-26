//! A true end-to-end test that runs the real `cargo bench` against a real
//! Criterion benchmark and harvests the output the genuine engine writes.
//!
//! Every other integration test drives `run` against a mock engine through the
//! `run_with_overrides` entry, pinning an absolute target root and launching the
//! mock from the workspace-root current directory. That is fast and hermetic, but
//! it cannot exercise the production `resolve_target_root`, real cargo argument
//! passing, Criterion's actual on-disk layout, or the working directory cargo
//! gives a benchmark binary. This test closes that gap with the smallest possible
//! real benchmark.
//!
//! It is deliberately heavyweight: it compiles Criterion and runs an actual
//! benchmark process, so it is gated off under Miri (no real subprocesses) and
//! under coverage (`cargo llvm-cov` shares one instrumented target directory and
//! the extra build cost is not worth instrumenting). The benchmark itself is
//! shrunk to the smallest Criterion permits — a 10-sample run with sub-second
//! warm-up and measurement windows, reduced bootstrap resampling, and Criterion
//! taken with `default-features = false` (dropping plotters and rayon) — and the
//! fixture builds unoptimized (see [`FIXTURE_CARGO_TOML`]) so the build stays
//! quick.
#![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
#![allow(
    clippy::float_cmp,
    reason = "the wall-time assertion only checks the sign"
)]

use std::path::{Path, PathBuf};

use cargo_bench_history::{Cli, Command, MetricKind, Run, RunOutcome, run};
use cargo_bench_history_core::codec;
use serial_test::serial;
use testing::CwdGuard;

/// Parses CLI arguments into the typed [`Command`], exactly as the binary does.
fn command_from(args: &[&str]) -> Command {
    Cli::from_args(&["cargo-bench-history"], args)
        .unwrap()
        .into_command()
}

/// Collects every `.json` file under `dir` (recursively) into `out`.
fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "json") {
            out.push(path);
        }
    }
}

/// A minimal standalone crate carrying a single Criterion benchmark. The empty
/// `[workspace]` table stops cargo from walking up into the folo workspace, and
/// `default-features = false` keeps the Criterion build fast while retaining the
/// `cargo_bench_support` feature `cargo bench` requires.
///
/// The `[profile.bench]` override builds Criterion and its dependency graph
/// unoptimized, since this fixture is its own workspace with its own `target/`
/// and shares no artifacts with folo. The build is the dominant cost, and the
/// test only checks that a positive wall time is harvested, never the magnitude
/// of the measurement — so optimization buys nothing here. `codegen-units` is
/// left at the default; raising it multiplies object files and slows linking
/// (especially under Windows real-time antivirus). The unoptimized build makes
/// Criterion's bootstrap analysis slow in turn, which the reduced `nresamples`
/// in the benchmark below compensates for.
const FIXTURE_CARGO_TOML: &str = r#"[package]
name = "bench_history_e2e_fixture"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[dev-dependencies]
criterion = { version = "0.8.2", default-features = false, features = ["cargo_bench_support"] }

[[bench]]
name = "tiny"
harness = false

[profile.bench]
opt-level = 0
debug = false
"#;

/// The benchmark itself: one case, `e2e/add`, with the smallest run Criterion
/// permits so the measurement completes in well under a second. `nresamples` is
/// cut far below Criterion's 100,000 default so the bootstrap analysis stays fast
/// even though the fixture builds unoptimized (see [`FIXTURE_CARGO_TOML`]); the
/// harvested wall-time point estimate does not depend on the resample count.
const FIXTURE_BENCH: &str = r#"use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};

fn tiny(c: &mut Criterion) {
    let mut group = c.benchmark_group("e2e");
    group
        .warm_up_time(Duration::from_millis(50))
        .measurement_time(Duration::from_millis(100))
        .sample_size(10);
    group.bench_function("add", |b| b.iter(|| black_box(1_u64) + black_box(2_u64)));
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().nresamples(1000);
    targets = tiny
}
criterion_main!(benches);
"#;

/// Storage points at a `store/` directory inside the fixture, keeping the test
/// hermetic; the explicit project `id` avoids depending on the tempdir basename.
const FIXTURE_CONFIG: &str = "[project]\nid = \"e2e\"\n\n[storage.local]\npath = \"./store\"\n";

/// Drives the production `run` against a real `cargo bench` + Criterion build and
/// asserts the genuine wall-time output is harvested and stored.
#[tokio::test]
#[serial]
#[cfg_attr(miri, ignore)] // Spawns a real `cargo bench`, which Miri cannot run.
#[cfg_attr(coverage_nightly, ignore)] // Heavy real build; llvm-cov shares one target dir.
async fn run_against_real_criterion_bench_stores_wall_time() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();

    std::fs::write(root.join("Cargo.toml"), FIXTURE_CARGO_TOML).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("lib.rs"), "").unwrap();
    std::fs::create_dir_all(root.join("benches")).unwrap();
    std::fs::write(root.join("benches").join("tiny.rs"), FIXTURE_BENCH).unwrap();
    std::fs::create_dir_all(root.join(".cargo")).unwrap();
    std::fs::write(
        root.join(".cargo").join("bench_history.toml"),
        FIXTURE_CONFIG,
    )
    .unwrap();

    // Drive the production entry (no overrides) with the fixture as the current
    // directory, so the real `resolve_target_root` injects `<root>/target` and
    // `cargo bench` builds and runs Criterion there. Restore the directory before
    // assertions and before the tempdir is dropped (required on Windows).
    let outcome = {
        let _cwd = CwdGuard::enter(root);
        run(&command_from(&["run"])).await.unwrap()
    };

    let RunOutcome::Completed { message } = outcome else {
        panic!("expected a completed run, got {outcome:?}");
    };
    assert!(
        message.contains("Stored 1"),
        "the real Criterion case should be stored: {message}"
    );

    let mut files = Vec::new();
    collect_json_files(&root.join("store"), &mut files);
    assert_eq!(
        files.len(),
        1,
        "expected exactly one stored object, found {files:?}"
    );

    let raw = std::fs::read(&files[0]).unwrap();
    let json = String::from_utf8(codec::decompress(&raw).unwrap()).unwrap();
    let set = Run::from_json(&json).unwrap();
    assert_eq!(set.results.len(), 1, "one harvested benchmark case");
    let record = &set.results[0];
    assert_eq!(record.id.qualified(), "e2e/add");

    let metric = record
        .metrics
        .iter()
        .find(|metric| metric.kind == MetricKind::WallTime)
        .unwrap();
    assert!(
        metric.value > 0.0,
        "a real measurement must be positive, got {}",
        metric.value
    );
}
