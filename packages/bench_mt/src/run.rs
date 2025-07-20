use std::iter;
use std::num::NonZero;
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use num::Integer;

use crate::{GroupInfo, RunBuilderBasic, RunBuilderFinal, ThreadPool};

/// A benchmark run will execute a specific number of multithreaded iterations on a [`ThreadPool`].
///
/// Use `Run::builder()` to prepare a run and configure the callbacks to use during the run,
/// then call `Run::execute_on()` to execute it on a specific thread pool.
#[must_use]
#[derive(Debug)]
pub struct Run<ThreadState = (), IterState = (), MeasureWrapperState = (), CleanupState = ()>
where
    ThreadState: 'static,
    IterState: 'static,
    MeasureWrapperState: 'static,
    CleanupState: 'static,
{
    // This type is just a wrapper around the final builder type, for better UX.
    inner: RunBuilderFinal<ThreadState, IterState, MeasureWrapperState, CleanupState>,
}

impl Run {
    /// Returns a new run builder that can be used to prepare a benchmark run.
    pub fn builder() -> RunBuilderBasic {
        RunBuilderBasic::new()
    }
}

impl<ThreadState, IterState, MeasureWrapperState, CleanupState>
    Run<ThreadState, IterState, MeasureWrapperState, CleanupState>
{
    pub(crate) fn new(
        inner: RunBuilderFinal<ThreadState, IterState, MeasureWrapperState, CleanupState>,
    ) -> Self {
        Run::<ThreadState, IterState, MeasureWrapperState, CleanupState> { inner }
    }

    /// Executes the benchmark run on the specified thread pool for a specific number of iterations.
    ///
    /// # Panics
    ///
    /// Panics if the thread pool's processor count is not divisible by the number of groups
    /// specified in the benchmark run builder.
    pub fn execute_on(self, pool: &ThreadPool, iterations: u64) -> RunStats {
        let (threads_per_group, remainder) =
            pool.thread_count().get().div_rem(&self.inner.groups.get());

        assert!(
            remainder == 0,
            "thread pool processor count must be divisible by the number of groups"
        );

        let group_count = self.inner.groups;

        let threads_per_group = NonZero::new(threads_per_group)
            .expect("guarded by NonZero thread count as well as remainder check above");

        let group_indexes = (0..self.inner.groups.get())
            .flat_map(|group_index| iter::repeat_n(group_index, threads_per_group.get()))
            .collect::<Vec<_>>();

        // Every thread takes an item and that becomes its group index.
        let group_indexes = Arc::new(Mutex::new(group_indexes));

        // All threads will wait on this before starting, so they start together.
        let start = Arc::new(Barrier::new(pool.thread_count().get()));

        let (result_txs, result_rxs): (Vec<_>, Vec<_>) =
            iter::repeat_with(oneshot::channel::<Duration>)
                .take(pool.thread_count().get())
                .unzip();

        // Every thread takes one and reports its result back via this.
        let result_txs = Arc::new(Mutex::new(result_txs));

        // Break the callbacks out of `self` so we do not send `self` to the pool.
        let prepare_thread_fn = self.inner.prepare_thread_fn;
        let prepare_iter_fn = self.inner.prepare_iter_fn;
        let iter_fn = self.inner.iter_fn;
        let measure_wrapper_begin_fn = self.inner.measure_wrapper_begin_fn;
        let measure_wrapper_end_fn = self.inner.measure_wrapper_end_fn;

        pool.enqueue_task({
            let start = Arc::clone(&start);
            let group_indexes = Arc::clone(&group_indexes);
            let result_txs = Arc::clone(&result_txs);

            move || {
                let group_index = group_indexes.lock().unwrap().pop().unwrap();
                let result_tx = result_txs.lock().unwrap().pop().unwrap();

                let group_info = GroupInfo {
                    index: group_index,
                    count: group_count,
                };

                let mut thread_state = prepare_thread_fn(&group_info);

                let iterations_usize = usize::try_from(iterations)
                    .expect("iteration count that exceeds virtual memory size is impossible to execute as state would not fit in memory");

                let iter_state = iter::repeat_with(|| {
                    prepare_iter_fn(&group_info, &mut thread_state)
                }).take(iterations_usize).collect::<Vec<_>>();

                let mut cleanup_state = Vec::with_capacity(iterations_usize);

                let measure_state = measure_wrapper_begin_fn(&group_info);

                start.wait();

                let start_time = Instant::now();

                for iter_state in iter_state {
                    cleanup_state.push(iter_fn(iter_state));
                }

                let elapsed = start_time.elapsed();

                measure_wrapper_end_fn(measure_state);

                drop(cleanup_state);
                drop(thread_state);

                result_tx.send(elapsed).unwrap();
            }
        });

        let mut total_elapsed_nanos: u128 = 0;

        for rx in result_rxs {
            let elapsed = rx.recv().unwrap();
            total_elapsed_nanos = total_elapsed_nanos.saturating_add(elapsed.as_nanos());
        }

        RunStats {
            mean_duration: calculate_mean_duration(pool.thread_count(), total_elapsed_nanos),
        }
    }
}

#[cfg_attr(test, mutants::skip)] // Difficult to simulate time and therefore set expectations.
fn calculate_mean_duration(thread_count: NonZero<usize>, total_elapsed_nanos: u128) -> Duration {
    let total_elapsed_nanos_per_thread = total_elapsed_nanos
        .checked_div(thread_count.get() as u128)
        .expect("thread count is NonZero, so division by zero is impossible");

    Duration::from_nanos(
        total_elapsed_nanos_per_thread
            .try_into()
            .expect("overflowing u64 is unrealistic when using a real clock"),
    )
}

/// The result of executing a benchmark run, carrying data suitable for ingestion
/// into a benchmark framework.
#[derive(Debug)]
#[must_use = "the benchmarking framework will typically need this information for its results"]
pub struct RunStats {
    mean_duration: Duration,
}

impl RunStats {
    /// Returns the mean duration of the run.
    #[must_use]
    pub fn mean_duration(&self) -> Duration {
        self.mean_duration
    }
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;
    use std::sync::atomic::{self, AtomicU64};

    use many_cpus::ProcessorSet;
    use new_zealand::nz;

    use super::*;

    static ONE_PROCESSOR: LazyLock<ProcessorSet> =
        LazyLock::new(|| ProcessorSet::builder().take(nz!(1)).unwrap());

    static ONE_THREAD: LazyLock<ThreadPool> = LazyLock::new(|| ThreadPool::new(&ONE_PROCESSOR));

    static TWO_PROCESSORS: LazyLock<Option<ProcessorSet>> =
        LazyLock::new(|| ProcessorSet::builder().take(nz!(2)));

    static TWO_THREADS: LazyLock<Option<ThreadPool>> =
        LazyLock::new(|| TWO_PROCESSORS.as_ref().map(ThreadPool::new));

    static THREE_PROCESSORS: LazyLock<Option<ProcessorSet>> =
        LazyLock::new(|| ProcessorSet::builder().take(nz!(3)));

    static THREE_THREADS: LazyLock<Option<ThreadPool>> =
        LazyLock::new(|| THREE_PROCESSORS.as_ref().map(ThreadPool::new));

    static FOUR_PROCESSORS: LazyLock<Option<ProcessorSet>> =
        LazyLock::new(|| ProcessorSet::builder().take(nz!(4)));

    static FOUR_THREADS: LazyLock<Option<ThreadPool>> =
        LazyLock::new(|| FOUR_PROCESSORS.as_ref().map(ThreadPool::new));

    #[test]
    fn single_iteration_minimal() {
        let iteration_count = Arc::new(AtomicU64::new(0));

        _ = Run::builder()
            .iter_fn({
                let iteration_count = Arc::clone(&iteration_count);

                move |()| {
                    iteration_count.fetch_add(1, atomic::Ordering::Relaxed);
                }
            })
            .build()
            .execute_on(&ONE_THREAD, 1);

        assert_eq!(iteration_count.load(atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn multiple_iterations_minimal() {
        let iteration_count = Arc::new(AtomicU64::new(0));

        _ = Run::builder()
            .iter_fn({
                let iteration_count = Arc::clone(&iteration_count);

                move |()| {
                    iteration_count.fetch_add(1, atomic::Ordering::Relaxed);
                }
            })
            .build()
            .execute_on(&ONE_THREAD, 9999);

        assert_eq!(iteration_count.load(atomic::Ordering::Relaxed), 9999);
    }

    // TODO: 2 processors and 2 groups gives one in each group
    // TODO: 4 processors and 2 groups gives two in each group
    // TODO: 3 processors and 3 groups gives one in each group
    // TODO: 4 processors and 3 groups is a panic
    // TODO: 1 processor and 2 groups is a panic

    // TODO: state is correctly passed from thread state, to iteration state, to cleanup
    // TODO: measurement wrapper is correctly called before and after the timed part of the run
    // TODO: cleanup is executed after the timed part of the run (after measurement wrapper end)

    // TODO: thread prepare, iteration prepare, measurement wrapper and iteration body are
    // called the expected number of times when targeting 1 processor
    // TODO: thread prepare, iteration prepare, measurement wrapper and iteration body are
    // called the expected number of times when targeting 4 processors and 1 group
    // TODO: thread prepare, iteration prepare, measurement wrapper and iteration body are
    // called the expected number of times when targeting 2 processors and 2 groups
    // TODO: thread prepare, iteration prepare, measurement wrapper and iteration body are
    // called the expected number of times when targeting 4 processors and 2 groups
}
