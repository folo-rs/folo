use std::{
    sync::{Arc, Barrier, Mutex},
    time::{Duration, Instant},
    vec,
};

mod threadpool;
pub use threadpool::*;

/// Executes a benchmark in parallel on every processor, returning the
/// average duration taken by each started thread.
///
/// `iter_fn` is called any number of times per thread, with the iteration count determined
/// by the Criterion benchmark infrastructure.
///
/// The return value of the `prepare_fn` is provided via shared reference to `iter_fn`. Creation
/// of this value is not part of the timed block. It is dropped after the timed block.
///
/// NB! Every batch will be executed on a fresh thread.
/// If you want pre-warmed threads, use `bench_on_threadpool()` instead.
pub fn bench_on_every_processor<P, D, F>(iters: u64, prepare_fn: P, iter_fn: F) -> Duration
where
    P: Fn() -> D + Send + Clone + 'static,
    F: Fn(&D) + Send + Clone + 'static,
{
    let pool = ThreadPool::all();

    bench_on_threadpool(&pool, iters, prepare_fn, iter_fn)
}

/// Executes a benchmark in parallel on every thread in a thread pool, returning the
/// average duration taken by each started thread.
///
/// `iter_fn` is called any number of times per thread, with the iteration count determined
/// by the Criterion benchmark infrastructure.
///
/// The return value of the `prepare_fn` is provided via shared reference to `iter_fn`. Creation
/// of this value is not part of the timed block. It is dropped after the timed block.
pub fn bench_on_threadpool<P, D, F>(
    thread_pool: &ThreadPool,
    iters: u64,
    prepare_fn: P,
    iter_fn: F,
) -> Duration
where
    P: Fn() -> D + Send + Clone + 'static,
    F: Fn(&D) + Send + Clone + 'static,
{
    let thread_count = thread_pool.thread_count();

    // All threads will wait on this before starting, so they start together.
    let start = Arc::new(Barrier::new(thread_count));

    let (result_txs, mut result_rxs): (Vec<_>, Vec<_>) =
        (0..thread_count).map(|_| oneshot::channel()).unzip();

    let result_txs = Arc::new(Mutex::new(result_txs));

    thread_pool.enqueue_task({
        let start = Arc::clone(&start);
        let result_txs = Arc::clone(&result_txs);

        move || {
            let result_tx = result_txs.lock().unwrap().pop().unwrap();

            let data = prepare_fn();

            start.wait();

            let start = Instant::now();

            for _ in 0..iters {
                iter_fn(&data);
            }

            let elapsed = start.elapsed();

            drop(data);

            result_tx.send(elapsed).unwrap();
        }
    });

    let mut total_elapsed_nanos = 0;

    for rx in result_rxs.drain(..) {
        let elapsed = rx.recv().unwrap();
        total_elapsed_nanos += elapsed.as_nanos();
    }

    Duration::from_nanos((total_elapsed_nanos / thread_count as u128) as u64)
}

/// For the A/B benchmarking, identifiers whether a worker is the A or the B.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AbWorker {
    A,
    B,
}

/// Executes a benchmark in parallel on every thread in a thread pool, returning the
/// average duration taken by each started thread.
///
/// `iter_fn` is called any number of times per thread, with the iteration count determined
/// by the Criterion benchmark infrastructure.
///
/// The return value of the `prepare_fn` is provided via shared reference to `iter_fn`. Creation
/// of this value is not part of the timed block. It is dropped after the timed block.
///
/// Threads are divided in two, with `prepare_fn` and `iter_fn` each provided either A/B to
/// distinguish one half from the other.
pub fn bench_on_threadpool_ab<P, D, F>(
    thread_pool: &ThreadPool,
    iters: u64,
    prepare_fn: P,
    iter_fn: F,
) -> Duration
where
    P: Fn(AbWorker) -> D + Send + Clone + 'static,
    F: Fn(AbWorker, &D) + Send + Clone + 'static,
{
    assert!(thread_pool.thread_count() >= 2);
    assert!(thread_pool.thread_count() % 2 == 0);

    let thread_count = thread_pool.thread_count();
    let half_thread_count = thread_count / 2;

    let worker_types = Arc::new(Mutex::new(
        vec![AbWorker::A; half_thread_count]
            .into_iter()
            .chain(vec![AbWorker::B; half_thread_count])
            .collect::<Vec<_>>(),
    ));

    bench_on_threadpool(
        thread_pool,
        iters,
        {
            let worker_types = Arc::clone(&worker_types);
            move || {
                let worker_type = worker_types.lock().unwrap().pop().unwrap();
                let data = prepare_fn(worker_type);
                (worker_type, data)
            }
        },
        move |(worker_type, data)| {
            let worker_type = *worker_type;
            iter_fn(worker_type, data);
        },
    )
}

#[cfg(not(miri))] // Miri cannot deal with hardware inspection APIs this uses.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_smoke_test() {
        _ = bench_on_every_processor(1, || (), |_| {});
    }
}
