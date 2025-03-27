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

    let mut total_elapsed_nanos: u128 = 0;

    for rx in result_rxs.drain(..) {
        let elapsed = rx.recv().unwrap();
        total_elapsed_nanos = total_elapsed_nanos.saturating_add(elapsed.as_nanos());
    }

    calculate_average_duration(thread_count, total_elapsed_nanos)
}

#[cfg_attr(test, mutants::skip)] // Difficult to simulate time and therefore set expectations.
fn calculate_average_duration(thread_count: usize, total_elapsed_nanos: u128) -> Duration {
    Duration::from_nanos((total_elapsed_nanos / thread_count as u128) as u64)
}

/// For the A/B benchmarking, identifiers whether a worker is the A or the B.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    use std::{
        num::NonZero,
        sync::atomic::{self, AtomicBool},
    };

    use many_cpus::ProcessorSet;

    use super::*;

    #[test]
    fn bench_all() {
        let prepare_called_at_least_once = Arc::new(AtomicBool::new(false));
        let iter_called_at_least_once = Arc::new(AtomicBool::new(false));

        _ = bench_on_every_processor(
            1,
            {
                let prepare_called_at_least_once = Arc::clone(&prepare_called_at_least_once);
                move || {
                    prepare_called_at_least_once.store(true, atomic::Ordering::SeqCst);
                }
            },
            {
                let iter_called_at_least_once = Arc::clone(&iter_called_at_least_once);
                move |_| {
                    iter_called_at_least_once.store(true, atomic::Ordering::SeqCst);
                }
            },
        );

        assert!(prepare_called_at_least_once.load(atomic::Ordering::SeqCst));
        assert!(iter_called_at_least_once.load(atomic::Ordering::SeqCst));
    }

    #[test]
    fn bench_ab_two() {
        let two_processors = ProcessorSet::builder().take(NonZero::new(2).unwrap());

        let Some(two_processors) = two_processors else {
            eprintln!("need at least two processors to run this test");
            return;
        };

        let pool = ThreadPool::new(two_processors);

        let a_prepare_called_at_least_once = Arc::new(AtomicBool::new(false));
        let a_iter_called_at_least_once = Arc::new(AtomicBool::new(false));
        let b_prepare_called_at_least_once = Arc::new(AtomicBool::new(false));
        let b_iter_called_at_least_once = Arc::new(AtomicBool::new(false));

        _ = bench_on_threadpool_ab(
            &pool,
            1,
            {
                let a_prepare_called_at_least_once = Arc::clone(&a_prepare_called_at_least_once);
                let b_prepare_called_at_least_once = Arc::clone(&b_prepare_called_at_least_once);
                move |ab| match ab {
                    AbWorker::A => {
                        a_prepare_called_at_least_once.store(true, atomic::Ordering::SeqCst)
                    }
                    AbWorker::B => {
                        b_prepare_called_at_least_once.store(true, atomic::Ordering::SeqCst)
                    }
                }
            },
            {
                let a_iter_called_at_least_once = Arc::clone(&a_iter_called_at_least_once);
                let b_iter_called_at_least_once = Arc::clone(&b_iter_called_at_least_once);
                move |ab, _| match ab {
                    AbWorker::A => {
                        a_iter_called_at_least_once.store(true, atomic::Ordering::SeqCst)
                    }
                    AbWorker::B => {
                        b_iter_called_at_least_once.store(true, atomic::Ordering::SeqCst)
                    }
                }
            },
        );

        assert!(a_prepare_called_at_least_once.load(atomic::Ordering::SeqCst));
        assert!(a_iter_called_at_least_once.load(atomic::Ordering::SeqCst));
        assert!(b_prepare_called_at_least_once.load(atomic::Ordering::SeqCst));
        assert!(b_iter_called_at_least_once.load(atomic::Ordering::SeqCst));
    }
}
