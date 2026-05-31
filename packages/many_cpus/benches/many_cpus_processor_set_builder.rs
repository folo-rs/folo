//! Benchmarking operations on the `ProcessorSetBuilder` type.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus::SystemHardware;
use new_zealand::nz;
use par_bench::{Run, ThreadPool};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let hw = SystemHardware::current();

    let one_processor_set = hw.processors().to_builder().take(nz!(1)).unwrap();
    let mut one_thread = ThreadPool::new(&one_processor_set);

    let mut two_threads = hw
        .processors()
        .to_builder()
        .take(nz!(2))
        .map(|x| ThreadPool::new(&x));

    let mut group = c.benchmark_group("ProcessorSetBuilder");

    // Results from this are really unstable for whatever reason. Give it more time to stabilize.
    group.measurement_time(Duration::from_secs(30));

    // Single-threaded benchmarks using Run pattern for consistent overhead.
    Run::new()
        .iter(|_| {
            black_box(hw.processors().to_builder().take_all().unwrap());
        })
        .execute_criterion_on(&mut one_thread, &mut group, "all_st");

    Run::new()
        .iter(|_| {
            black_box(hw.processors().to_builder().take(nz!(1)).unwrap());
        })
        .execute_criterion_on(&mut one_thread, &mut group, "one_st");

    Run::new()
        .iter(|_| {
            black_box(
                hw.processors()
                    .to_builder()
                    .filter(|p| p.id() % 2 == 0)
                    .take_all()
                    .unwrap(),
            );
        })
        .execute_criterion_on(&mut one_thread, &mut group, "only_evens_st");

    // Two-processor benchmarks for comparison.
    if let Some(ref mut thread_pool) = two_threads {
        Run::new()
            .iter(|_| {
                black_box(hw.processors().to_builder().take_all().unwrap());
            })
            .execute_criterion_on(thread_pool, &mut group, "all_mt");

        Run::new()
            .iter(|_| {
                black_box(hw.processors().to_builder().take(nz!(1)).unwrap());
            })
            .execute_criterion_on(thread_pool, &mut group, "one_mt");

        Run::new()
            .iter(|_| {
                black_box(
                    hw.processors()
                        .to_builder()
                        .filter(|p| p.id() % 2 == 0)
                        .take_all()
                        .unwrap(),
                );
            })
            .execute_criterion_on(thread_pool, &mut group, "only_evens_mt");
    }

    group.finish();
}
