#![expect(missing_docs, reason = "benchmarks")]

use std::hint::black_box;
use std::time::Instant;

use all_the_time::Session as TimeSession;
use alloc_tracker::{Allocator, Session as AllocSession};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use vicinal::Pool;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn entrypoint(c: &mut Criterion) {
    let allocs = AllocSession::new();
    let times = TimeSession::new();

    let mut g = c.benchmark_group("spawn");

    let spawn_single_alloc = allocs.operation("spawn_single");
    let spawn_single_time = times.operation("spawn_single");

    g.bench_function("spawn_single", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let start = Instant::now();
            let _alloc_span = spawn_single_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_single_time.measure_thread().iterations(iterations);

            for _ in 0..iterations {
                let handle = scheduler.spawn(|| black_box(42));
                black_box(block_on(handle));
            }

            start.elapsed()
        });
    });

    let spawn_100_alloc = allocs.operation("spawn_100");
    let spawn_100_time = times.operation("spawn_100");

    g.bench_function("spawn_100", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let mut handles = Vec::with_capacity(100);

            let start = Instant::now();
            let _alloc_span = spawn_100_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_100_time.measure_thread().iterations(iterations);

            for _ in 0..iterations {
                for i in 0..100 {
                    handles.push(scheduler.spawn(move || black_box(i)));
                }

                #[allow(
                    clippy::iter_with_drain,
                    reason = "we reuse the vector in the next iteration"
                )]
                for handle in handles.drain(..) {
                    black_box(block_on(handle));
                }
            }

            start.elapsed()
        });
    });

    let spawn_urgent_single_alloc = allocs.operation("spawn_urgent_single");
    let spawn_urgent_single_time = times.operation("spawn_urgent_single");

    g.bench_function("spawn_urgent_single", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let start = Instant::now();
            let _alloc_span = spawn_urgent_single_alloc
                .measure_process()
                .iterations(iterations);
            let _time_span = spawn_urgent_single_time
                .measure_thread()
                .iterations(iterations);

            for _ in 0..iterations {
                let handle = scheduler.spawn_urgent(|| black_box(42));
                black_box(block_on(handle));
            }

            start.elapsed()
        });
    });

    let spawn_only_alloc = allocs.operation("spawn_only");
    let spawn_only_time = times.operation("spawn_only");

    g.bench_function("spawn_only", |b| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        b.iter_custom(|iterations| {
            let capacity = usize::try_from(iterations).expect("iterations fits in usize");
            let mut handles = Vec::with_capacity(capacity);

            let start = Instant::now();
            let _alloc_span = spawn_only_alloc.measure_process().iterations(iterations);
            let _time_span = spawn_only_time.measure_thread().iterations(iterations);

            for _ in 0..iterations {
                handles.push(scheduler.spawn(|| black_box(42)));
            }

            let elapsed = start.elapsed();

            // Await outside measured section to avoid measuring await overhead.
            for handle in handles {
                black_box(block_on(handle));
            }

            elapsed
        });
    });

    g.finish();
}

criterion_group!(benches, entrypoint);
criterion_main!(benches);
