//! Full-lifecycle benchmarks for the `alloc_tracker` memory allocation tracker.
//!
//! Where `alloc_tracker_tracking_overhead` measures only the per-span record
//! path, this benchmark covers every remaining lifecycle stage — session and
//! operation setup, report snapshotting, merging, human rendering (which
//! resolves the recorded spans into per-iteration figures and sorts operations —
//! the work that regressed in issue #328), and JSON output. Each stage is
//! measured for all three cost dimensions at once:
//!
//! * **wall-clock time** — Criterion, the harness.
//! * **memory allocations** — `alloc_tracker` itself, measuring its own lifecycle.
//! * **processor time** — `all_the_time` (a dev-dependency; this crate and
//!   `all_the_time` form an allowed dev-dependency-only cycle).
//!
//! The rendering stage is scaled by operation count so that any regression back
//! to super-linear or retained-span work surfaces immediately instead of hiding
//! behind a cheap "slope-only" summary path.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::fmt::Write as _;
use std::hint::black_box;
use std::time::{Duration, Instant};

use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Report, Session};
use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// Number of spans folded into each operation of the report fixtures.
///
/// Streaming statistics fold every span into O(1) accumulators, so this affects
/// only the fixture's record cost, not the rendering cost — which is exactly the
/// invariant the rendering benchmark locks in.
const SPANS_PER_OP: usize = 8;

fn entrypoint(c: &mut Criterion) {
    // The measuring sessions capture this benchmark's own allocation and
    // processor-time cost and emit their JSON on drop at the end of `entrypoint`.
    let alloc = Session::new();
    let time = TimeSession::new();

    setup(c, &alloc, &time);
    snapshot(c, &alloc, &time);
    merge(c, &alloc, &time);
    render(c, &alloc, &time);
    write(c, &alloc, &time);
}

/// Wraps the measured `body` in an allocation span and a processor-time span so
/// every Criterion sample also feeds the two side-channel measuring sessions.
fn measured<F: FnMut()>(
    iters: u64,
    alloc_op: &alloc_tracker::Operation,
    time_op: &all_the_time::Operation,
    mut body: F,
) -> Duration {
    let start = Instant::now();
    {
        let _alloc_span = alloc_op.measure_thread().iterations(iters);
        let _time_span = time_op.measure_thread().iterations(iters);

        for _ in 0..iters {
            body();
        }
    }
    start.elapsed()
}

/// Builds a report fixture with `num_ops` operations, each carrying
/// `SPANS_PER_OP` recorded spans, so downstream stages have realistic streaming
/// state to work over.
fn build_report(num_ops: usize) -> Report {
    let session = Session::new().no_stdout().no_file();
    for op_idx in 0..num_ops {
        let op = session.operation(format!("op_{op_idx}"));
        for _ in 0..SPANS_PER_OP {
            let _span = op.measure_thread().iterations(10);
            black_box((0..10_u32).collect::<Vec<_>>());
        }
    }
    session.to_report()
}

fn setup(c: &mut Criterion, alloc: &Session, time: &TimeSession) {
    let mut group = c.benchmark_group("alloc_tracker_report_lifecycle/setup");
    let alloc_op = alloc.operation("alloc_tracker_report_lifecycle/setup/session_100_ops");
    let time_op = time.operation("alloc_tracker_report_lifecycle/setup/session_100_ops");

    group.bench_function("session_100_ops", |b| {
        b.iter_custom(|iters| {
            measured(iters, &alloc_op, &time_op, || {
                let session = Session::new().no_stdout().no_file();
                for op_idx in 0..100 {
                    black_box(session.operation(format!("op_{op_idx}")));
                }
                black_box(&session);
            })
        });
    });

    group.finish();
}

fn snapshot(c: &mut Criterion, alloc: &Session, time: &TimeSession) {
    let mut group = c.benchmark_group("alloc_tracker_report_lifecycle/snapshot");
    let alloc_op = alloc.operation("alloc_tracker_report_lifecycle/snapshot/ops_100");
    let time_op = time.operation("alloc_tracker_report_lifecycle/snapshot/ops_100");

    // A populated session snapshotted repeatedly; `to_report` takes `&self`.
    let session = Session::new().no_stdout().no_file();
    for op_idx in 0..100 {
        let op = session.operation(format!("op_{op_idx}"));
        for _ in 0..SPANS_PER_OP {
            let _span = op.measure_thread().iterations(10);
            black_box((0..10_u32).collect::<Vec<_>>());
        }
    }

    group.bench_function("ops_100", |b| {
        b.iter_custom(|iters| {
            measured(iters, &alloc_op, &time_op, || {
                black_box(session.to_report());
            })
        });
    });

    group.finish();
}

fn merge(c: &mut Criterion, alloc: &Session, time: &TimeSession) {
    let mut group = c.benchmark_group("alloc_tracker_report_lifecycle/merge");
    let alloc_op = alloc.operation("alloc_tracker_report_lifecycle/merge/ops_100");
    let time_op = time.operation("alloc_tracker_report_lifecycle/merge/ops_100");

    let left = build_report(100);
    let right = build_report(100);

    group.bench_function("ops_100", |b| {
        b.iter_custom(|iters| {
            measured(iters, &alloc_op, &time_op, || {
                black_box(Report::merge(black_box(&left), black_box(&right)));
            })
        });
    });

    group.finish();
}

fn render(c: &mut Criterion, alloc: &Session, time: &TimeSession) {
    let mut group = c.benchmark_group("alloc_tracker_report_lifecycle/render");

    // Scaling render by operation count is the core regression guard: rendering
    // resolves per-iteration figures and sorts operations, which streaming makes
    // O(operations), so the 10x jump in operation count must show a roughly 10x
    // (not super-linear) jump in cost.
    for num_ops in [10_usize, 100] {
        let report = build_report(num_ops);
        let name = format!("ops_{num_ops}");
        let alloc_op = alloc.operation(format!("alloc_tracker_report_lifecycle/render/{name}"));
        let time_op = time.operation(format!("alloc_tracker_report_lifecycle/render/{name}"));

        group.bench_function(&name, |b| {
            b.iter_custom(|iters| {
                measured(iters, &alloc_op, &time_op, || {
                    let mut rendered = String::new();
                    write!(rendered, "{report}").expect("writing to a String cannot fail");
                    black_box(rendered);
                })
            });
        });
    }

    group.finish();
}

fn write(c: &mut Criterion, alloc: &Session, time: &TimeSession) {
    let mut group = c.benchmark_group("alloc_tracker_report_lifecycle/write");
    let alloc_op = alloc.operation("alloc_tracker_report_lifecycle/write/ops_25");
    let time_op = time.operation("alloc_tracker_report_lifecycle/write/ops_25");

    // JSON output is I/O-bound, so keep the operation count modest.
    let report = build_report(25);
    let directory = tempfile::tempdir().expect("creating a temp directory cannot fail in a bench");

    group.bench_function("ops_25", |b| {
        b.iter_custom(|iters| {
            measured(iters, &alloc_op, &time_op, || {
                report.write_to_directory(directory.path());
            })
        });
    });

    group.finish();
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
