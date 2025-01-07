use std::num::NonZeroUsize;

use criterion::{criterion_group, criterion_main, Criterion};
use folo_hw::ProcessorSet;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

const TWO_PROCESSORS: NonZeroUsize = NonZeroUsize::new(2).unwrap();

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("cross_memory_region_traffic");

    let all_processors = ProcessorSet::all();

    let far_processor_pair = ProcessorSet::builder()
        .performance_processors_only()
        .different_memory_regions()
        .take(TWO_PROCESSORS)
        .expect("must have two processors in different memory regions for this benchmark");

    let near_processor_pair = ProcessorSet::builder()
        .performance_processors_only()
        .same_memory_region()
        .take(TWO_PROCESSORS)
        .expect("must have two processors in the same memory region for this benchmark");

    group.finish();
}
