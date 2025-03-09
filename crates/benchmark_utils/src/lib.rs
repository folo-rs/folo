use std::{
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};

use many_cpus::ProcessorSet;

/// Executes a benchmark in parallel on every processor, returning the
/// average duration taken by each started thread.
///
/// `iter_fn` is called any number of times per thread, with the iteration count determined
/// by the Criterion benchmark infrastructure.
///
/// The return value of the `prepare_fn` is provided via shared reference to `iter_fn`. Creation
/// of this value is not part of the timed block. It is dropped after the timed block.
pub fn bench_on_every_processor<P, D, F>(iters: u64, prepare_fn: P, iter_fn: F) -> Duration
where
    P: Fn() -> D + Send + Clone + 'static,
    F: Fn(&D) + Send + Clone + 'static,
{
    let processors = ProcessorSet::all();

    // All threads will wait on this before starting, so they start together.
    let barrier = Arc::new(Barrier::new(processors.len()));

    let threads = processors.spawn_threads({
        let barrier = barrier.clone();
        move |_| {
            let data = prepare_fn();

            barrier.wait();

            let start = Instant::now();

            for _ in 0..iters {
                iter_fn(&data);
            }

            let elapsed = start.elapsed();

            drop(data);

            elapsed
        }
    });

    let mut total_elapsed_nanos = 0;

    let thread_count = threads.len();

    for thread in threads {
        let elapsed = thread.join().unwrap();
        total_elapsed_nanos += elapsed.as_nanos();
    }

    Duration::from_nanos((total_elapsed_nanos / thread_count as u128) as u64)
}
