//! Benchmarking operations exposed by the `HardwareTracker` struct.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::{HardwareTracker, ProcessorSet};
use new_zealand::nz;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("HardwareTracker");

    // Results from this are really unstable for whatever reason. Give it more time to stabilize.
    group.measurement_time(Duration::from_secs(30));

    group.bench_function("current_processor_unpinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::with_current_processor(|p| {
                // We cannot return a reference to the processor itself but this is close enough.
                p.id()
            }));
        });
    });

    group.bench_function("current_processor_id_unpinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::current_processor_id());
        });
    });

    group.bench_function("current_memory_region_id_unpinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::current_memory_region_id());
        });
    });

    // Now we pin the current thread and do the whole thing again!
    let one_processor = ProcessorSet::builder()
        .performance_processors_only()
        .take(nz!(1))
        .unwrap();

    one_processor.pin_current_thread_to();

    group.bench_function("current_processor_pinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::with_current_processor(|p| {
                // We cannot return a reference to the processor itself but this is close enough.
                p.id()
            }));
        });
    });

    group.bench_function("current_processor_id_pinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::current_processor_id());
        });
    });

    group.bench_function("current_memory_region_id_pinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::current_memory_region_id());
        });
    });

    // Don't forget to unpin the thread to avoid affecting future benchmarks!
    ProcessorSet::builder()
        .ignoring_resource_quota()
        .take_all()
        .unwrap()
        .pin_current_thread_to();

    group.finish();
}
