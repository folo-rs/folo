//! In-memory ZIP creation costs, independent of filesystem and process setup.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use crp_native::benchmark_archive;

::testing::set_allocator!();

// Contrast a small input with one spanning many streaming buffers; neither is a size limit.
const LOW_BYTES: usize = 4 * 1024;
const HIGH_BYTES: usize = 256 * 1024;

criterion_group!(benches, archiving);
criterion_main!(benches);

fn archiving(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("crp_native_archive/deflate");
    for (name, size) in [("low", LOW_BYTES), ("high", HIGH_BYTES)] {
        // Repeat nonuniform bytes to exercise compression without random fixture setup.
        let input: Vec<_> = (0..=u8::MAX).cycle().take(size).collect();
        group.bench_function(name, |bencher| {
            bencher.iter(|| {
                black_box(
                    benchmark_archive(black_box(&input))
                        .expect("in-memory archive inputs and output are valid"),
                )
            });
        });
    }
    group.finish();
}
