//! Benchmarking Windows PAL internal logic via private API that bypasses the
//! public API and allows operations to be performed without (full) caching.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[allow(
    clippy::needless_pass_by_ref_mut,
    reason = "spurious error on non-Windows"
)]
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
    use std::sync::Arc;
    use std::time::Duration;

    use benchmark_utils::{ThreadPool, bench_on_threadpool};
    use criterion::Criterion;
    use many_cpus::ProcessorSet;
    use many_cpus::pal::BUILD_TARGET_PLATFORM;
    use new_zealand::nz;
    use windows::Win32::System::SystemInformation::GROUP_AFFINITY;

    pub(crate) fn entrypoint(c: &mut Criterion) {
        let mut group = c.benchmark_group("Pal_Windows");

        // The results are quite jittery. Give it some time to stabilize.
        group.measurement_time(Duration::from_secs(30));

        group.bench_function("current_thread_processors", |b| {
            b.iter(|| black_box(BUILD_TARGET_PLATFORM.__private_current_thread_processors()));
        });

        group.bench_function("get_all_processors", |b| {
            b.iter(|| BUILD_TARGET_PLATFORM.__private_get_all_processors());
        });

        group.bench_function("affinity_mask_to_processor_id_1", |b| {
            let mask = GROUP_AFFINITY {
                Group: 0,
                Mask: 1,
                ..Default::default()
            };

            b.iter(|| {
                black_box(BUILD_TARGET_PLATFORM.__private_affinity_mask_to_processor_id(&mask))
            });
        });

        group.bench_function("affinity_mask_to_processor_id_16", |b| {
            let mask = GROUP_AFFINITY {
                Group: 0,
                Mask: 0xFF,
                ..Default::default()
            };

            b.iter(|| {
                black_box(BUILD_TARGET_PLATFORM.__private_affinity_mask_to_processor_id(&mask))
            });
        });

        group.bench_function("pin_thread_to_default_set", |b| {
            let default_processor_set = Arc::new(ProcessorSet::default());
            let one_processor = ProcessorSet::builder().take(nz!(1)).unwrap();
            let one_thread = ThreadPool::new(&one_processor);

            b.iter_custom({
                |iters| {
                    bench_on_threadpool(&one_thread, iters, || (), {
                        let default_processor_set = Arc::clone(&default_processor_set);

                        move |()| {
                            default_processor_set.pin_current_thread_to();
                        }
                    })
                }
            });
        });

        group.finish();
    }
}
