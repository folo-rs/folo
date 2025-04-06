//! Benchmarking operations exposed by the `HardwareTracker` struct.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use folo_utils::nz;
use many_cpus::{HardwareTracker, ProcessorSet};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("HardwareTracker");

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
    ProcessorSet::all().pin_current_thread_to();

    group.finish();
}
