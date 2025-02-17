use std::{
    hint::black_box,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    },
    thread,
};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use folo_state::region_local;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("region_local");

    group.bench_function("get", |b| {
        b.iter(|| {
            region_local!(static VALUE: u32 = 99942);

            black_box(VALUE.get());
        })
    });

    group.bench_function("set", |b| {
        b.iter(|| {
            region_local!(static VALUE: u32 = 99942);

            VALUE.set(black_box(566));
        })
    });

    group.finish();

    let mut group = c.benchmark_group("region_local_par");

    // Two threads perform "get" in a loop.
    // Both threads work until both have hit the target iteration count.
    group.bench_function("par_get", |b| {
        region_local!(static VALUE: u32 = 99942);

        b.iter_batched(
            || {
                ParallelRun::prepare(
                    || _ = black_box(VALUE.get()),
                    || _ = black_box(VALUE.get()),
                    10_000,
                )
            },
            |run| run.start(),
            BatchSize::PerIteration,
        );
    });

    // One thread performs "get" in a loop, another performs "set" in a loop.
    // Both threads work until both have hit the target iteration count.
    group.bench_function("par_get_set", |b| {
        region_local!(static VALUE: u32 = 99942);

        b.iter_batched(
            || {
                ParallelRun::prepare(
                    || _ = black_box(VALUE.get()),
                    || VALUE.set(black_box(566)),
                    10_000,
                )
            },
            |run| run.start(),
            BatchSize::PerIteration,
        );
    });

    // TODO: Get & get+set from different memory regions.
    // TODO: Get & get+set from pinned processors.

    group.finish();
}

struct ParallelRun {
    start_barrier: Arc<Barrier>,
    done_barrier: Arc<Barrier>,

    threads: Vec<thread::JoinHandle<()>>,
}

impl ParallelRun {
    fn prepare<A, B>(mut a: A, mut b: B, target_iterations: usize) -> Self
    where
        A: FnMut() + Send + 'static,
        B: FnMut() + Send + 'static,
    {
        // We update iteration counts every BATCH_SIZE iterations (because updating those
        // counts can be slow, so we do not want to apply this overhead to every iteration).
        const BATCH_SIZE: usize = 100;

        let iterations_completed_a = Arc::new(AtomicUsize::new(0));
        let iterations_completed_b = Arc::new(AtomicUsize::new(0));

        // a + b + prepare()
        let ready_barrier = Arc::new(Barrier::new(3));
        // a + b + start()
        let start_barrier = Arc::new(Barrier::new(3));
        // a + b + start()
        let done_barrier = Arc::new(Barrier::new(3));

        let thread_a = thread::spawn({
            let ready_barrier = Arc::clone(&ready_barrier);
            let start_barrier = Arc::clone(&start_barrier);
            let done_barrier = Arc::clone(&done_barrier);
            let iterations_completed_a = Arc::clone(&iterations_completed_a);
            let iterations_completed_b = Arc::clone(&iterations_completed_b);

            move || {
                ready_barrier.wait();
                start_barrier.wait();

                let mut iterations_completed = 0;
                loop {
                    a();

                    iterations_completed += 1;

                    if iterations_completed % BATCH_SIZE == 0 {
                        iterations_completed_a.fetch_add(BATCH_SIZE, Ordering::Relaxed);

                        if iterations_completed >= target_iterations
                            && iterations_completed_b.load(Ordering::Relaxed) >= target_iterations
                        {
                            break;
                        }
                    }
                }

                done_barrier.wait();
            }
        });

        let thread_b = thread::spawn({
            let ready_barrier = Arc::clone(&ready_barrier);
            let start_barrier = Arc::clone(&start_barrier);
            let done_barrier = Arc::clone(&done_barrier);
            let iterations_completed_a = Arc::clone(&iterations_completed_a);
            let iterations_completed_b = Arc::clone(&iterations_completed_b);

            move || {
                ready_barrier.wait();
                start_barrier.wait();

                let mut iterations_completed = 0;
                loop {
                    b();

                    iterations_completed += 1;

                    if iterations_completed % BATCH_SIZE == 0 {
                        iterations_completed_b.fetch_add(BATCH_SIZE, Ordering::Relaxed);

                        if iterations_completed >= target_iterations
                            && iterations_completed_a.load(Ordering::Relaxed) >= target_iterations
                        {
                            break;
                        }
                    }
                }

                done_barrier.wait();
            }
        });

        ready_barrier.wait();

        Self {
            start_barrier,
            done_barrier,
            threads: vec![thread_a, thread_b],
        }
    }

    fn start(&self) {
        self.start_barrier.wait();
        self.done_barrier.wait();
    }
}

impl Drop for ParallelRun {
    fn drop(&mut self) {
        for thread in self.threads.drain(..) {
            thread.join().unwrap();
        }
    }
}
