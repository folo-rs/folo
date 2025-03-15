use std::{hint::black_box, num::NonZero};

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::{HardwareTracker, ProcessorSet};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("HardwareTracker");

    group.bench_function("current_processor_unpinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::with_current(|tracker| {
                // We cannot return a reference to the processor itself but this is close enough.
                tracker.current_processor().id()
            }));
        })
    });

    group.bench_function("current_processor_id_unpinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::with_current(|tracker| {
                tracker.current_processor_id()
            }));
        })
    });

    group.bench_function("current_memory_region_id_unpinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::with_current(|tracker| {
                tracker.current_memory_region_id()
            }));
        })
    });

    // Now we pin the current thread and do the whole thing again!
    let one_processor = ProcessorSet::builder()
        .performance_processors_only()
        .take(NonZero::new(1).unwrap())
        .unwrap();

    one_processor.pin_current_thread_to();

    group.bench_function("current_processor_pinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::with_current(|tracker| {
                // We cannot return a reference to the processor itself but this is close enough.
                tracker.current_processor().id()
            }));
        })
    });

    group.bench_function("current_processor_id_pinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::with_current(|tracker| {
                tracker.current_processor_id()
            }));
        })
    });

    group.bench_function("current_memory_region_id_pinned", |b| {
        b.iter(|| {
            black_box(HardwareTracker::with_current(|tracker| {
                tracker.current_memory_region_id()
            }));
        })
    });

    // Don't forget to unpin the thread to avoid affecting future benchmarks!
    ProcessorSet::all().pin_current_thread_to();

    group.finish();
}
