//! Benchmarking Windows PAL internal logic via private API that bypasses the
//! public API and allows operations to be performed without (full) caching.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    #[cfg(windows)]
    windows::entrypoint(c);

    #[cfg(not(windows))]
    {
        _ = c;
    }
}

#[cfg(windows)]
mod windows {
    use std::hint::black_box;

    use criterion::Criterion;
    use many_cpus::pal::BUILD_TARGET_PLATFORM;

    pub(crate) fn entrypoint(c: &mut Criterion) {
        let mut group = c.benchmark_group("Pal_Windows");

        group.bench_function("current_thread_processors", |b| {
            b.iter(|| black_box(BUILD_TARGET_PLATFORM.__private_current_thread_processors()));
        });

        group.finish();
    }
}
