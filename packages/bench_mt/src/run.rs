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
        Self { inner }
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

                let group_info = GroupInfo::new(group_index, group_count);

                let mut thread_state = prepare_thread_fn(&group_info);

                let iterations_usize = usize::try_from(iterations)
                    .expect("iteration count that exceeds virtual memory size is impossible to execute as state would not fit in memory");

                let iter_state = iter::repeat_with(|| {
                    prepare_iter_fn(&group_info, &mut thread_state)
                }).take(iterations_usize).collect::<Vec<_>>();

                let mut cleanup_state = Vec::with_capacity(iterations_usize);

                start.wait();

                let measure_state = measure_wrapper_begin_fn(&group_info);

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
    #[cfg_attr(test, mutants::skip)] // Real timing logic in tests is not desirable.
    pub fn mean_duration(&self) -> Duration {
        self.mean_duration
    }
}

#[cfg(test)]
#[cfg(not(miri))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "test code with known array bounds")]

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

    #[test]
    fn two_processors_two_groups_one_thread_per_group() {
        let Some(pool) = TWO_THREADS.as_ref() else {
            println!(
                "Skipping test two_processors_two_groups_one_thread_per_group: not enough processors"
            );
            return; // Skip test if not enough processors.
        };

        let group_info_seen = Arc::new(Mutex::new(Vec::new()));

        _ = Run::builder()
            .groups(nz!(2))
            .prepare_thread_fn({
                let group_info_seen = Arc::clone(&group_info_seen);
                move |group_info| {
                    group_info_seen.lock().unwrap().push(*group_info);
                }
            })
            .iter_fn(|()| ())
            .build()
            .execute_on(pool, 1);

        let mut seen = group_info_seen.lock().unwrap();
        seen.sort_by_key(|info| info.index());

        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].index(), 0);
        assert_eq!(seen[0].count().get(), 2);
        assert_eq!(seen[1].index(), 1);
        assert_eq!(seen[1].count().get(), 2);
    }

    #[test]
    fn four_processors_two_groups_two_threads_per_group() {
        let Some(pool) = FOUR_THREADS.as_ref() else {
            println!(
                "Skipping test four_processors_two_groups_two_threads_per_group: not enough processors"
            );
            return; // Skip test if not enough processors.
        };

        let group_info_seen = Arc::new(Mutex::new(Vec::new()));

        _ = Run::builder()
            .groups(nz!(2))
            .prepare_thread_fn({
                let group_info_seen = Arc::clone(&group_info_seen);
                move |group_info| {
                    group_info_seen.lock().unwrap().push(*group_info);
                }
            })
            .iter_fn(|()| ())
            .build()
            .execute_on(pool, 1);

        let mut seen = group_info_seen.lock().unwrap();
        seen.sort_by_key(|info| info.index());

        assert_eq!(seen.len(), 4);
        // Two threads should be in group 0.
        assert_eq!(seen[0].index(), 0);
        assert_eq!(seen[0].count().get(), 2);
        assert_eq!(seen[1].index(), 0);
        assert_eq!(seen[1].count().get(), 2);
        // Two threads should be in group 1.
        assert_eq!(seen[2].index(), 1);
        assert_eq!(seen[2].count().get(), 2);
        assert_eq!(seen[3].index(), 1);
        assert_eq!(seen[3].count().get(), 2);
    }

    #[test]
    fn three_processors_three_groups_one_thread_per_group() {
        let Some(pool) = THREE_THREADS.as_ref() else {
            println!(
                "Skipping test three_processors_three_groups_one_thread_per_group: not enough processors"
            );
            return; // Skip test if not enough processors.
        };

        let group_info_seen = Arc::new(Mutex::new(Vec::new()));

        _ = Run::builder()
            .groups(nz!(3))
            .prepare_thread_fn({
                let group_info_seen = Arc::clone(&group_info_seen);
                move |group_info| {
                    group_info_seen.lock().unwrap().push(*group_info);
                }
            })
            .iter_fn(|()| ())
            .build()
            .execute_on(pool, 1);

        let mut seen = group_info_seen.lock().unwrap();
        seen.sort_by_key(|info| info.index());

        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].index(), 0);
        assert_eq!(seen[0].count().get(), 3);
        assert_eq!(seen[1].index(), 1);
        assert_eq!(seen[1].count().get(), 3);
        assert_eq!(seen[2].index(), 2);
        assert_eq!(seen[2].count().get(), 3);
    }

    #[test]
    #[should_panic]
    fn four_processors_three_groups_panics() {
        let Some(pool) = FOUR_THREADS.as_ref() else {
            println!("Skipping test four_processors_three_groups_panics: not enough processors");
            panic!("Skip test if not enough processors by panicking");
        };

        _ = Run::builder()
            .groups(nz!(3))
            .iter_fn(|()| ())
            .build()
            .execute_on(pool, 1);
    }

    #[test]
    #[should_panic]
    fn one_processor_two_groups_panics() {
        _ = Run::builder()
            .groups(nz!(2))
            .iter_fn(|()| ())
            .build()
            .execute_on(&ONE_THREAD, 1);
    }

    #[test]
    fn state_flow_from_thread_to_iteration_to_cleanup() {
        let cleanup_states = Arc::new(Mutex::new(Vec::new()));

        _ = Run::builder()
            .prepare_thread_fn(|_| "thread_state".to_string())
            .prepare_iter_fn(|_, thread_state| format!("{thread_state}_iter"))
            .iter_fn({
                let cleanup_states = Arc::clone(&cleanup_states);
                move |iter_state| {
                    let cleanup_state = format!("{iter_state}_cleanup");
                    cleanup_states.lock().unwrap().push(cleanup_state.clone());
                    cleanup_state
                }
            })
            .build()
            .execute_on(&ONE_THREAD, 2);

        let states = cleanup_states.lock().unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0], "thread_state_iter_cleanup");
        assert_eq!(states[1], "thread_state_iter_cleanup");
    }

    #[test]
    fn measurement_wrapper_called_before_and_after_timed_execution() {
        let events = Arc::new(Mutex::new(Vec::new()));

        _ = Run::builder()
            .measure_wrapper_fns(
                {
                    let events = Arc::clone(&events);
                    move |_| {
                        events.lock().unwrap().push("begin".to_string());
                        "wrapper_state".to_string()
                    }
                },
                {
                    let events = Arc::clone(&events);
                    move |wrapper_state| {
                        events.lock().unwrap().push(format!("end_{wrapper_state}"));
                    }
                },
            )
            .iter_fn({
                let events = Arc::clone(&events);
                move |()| {
                    events.lock().unwrap().push("iteration".to_string());
                }
            })
            .build()
            .execute_on(&ONE_THREAD, 1);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], "begin");
        assert_eq!(events[1], "iteration");
        assert_eq!(events[2], "end_wrapper_state");
    }

    #[test]
    fn cleanup_executed_after_measurement_wrapper_end() {
        let events = Arc::new(Mutex::new(Vec::new()));

        _ = Run::builder()
            .measure_wrapper_fns(|_| "wrapper_state".to_string(), {
                let events = Arc::clone(&events);
                move |_| {
                    events.lock().unwrap().push("wrapper_end".to_string());
                }
            })
            .iter_fn({
                let events = Arc::clone(&events);
                move |()| {
                    struct CleanupTracker {
                        events: Arc<Mutex<Vec<String>>>,
                    }

                    impl Drop for CleanupTracker {
                        fn drop(&mut self) {
                            self.events.lock().unwrap().push("cleanup".to_string());
                        }
                    }

                    CleanupTracker {
                        events: Arc::clone(&events),
                    }
                }
            })
            .build()
            .execute_on(&ONE_THREAD, 1);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], "wrapper_end");
        assert_eq!(events[1], "cleanup");
    }

    fn test_call_counts(
        pool: &ThreadPool,
        groups: NonZero<usize>,
    ) {
        const ITERATIONS: u64 = 3;
        let expected_threads = pool.thread_count().get();
        
        let thread_prepare_count = Arc::new(AtomicU64::new(0));
        let iter_prepare_count = Arc::new(AtomicU64::new(0));
        let wrapper_begin_count = Arc::new(AtomicU64::new(0));
        let wrapper_end_count = Arc::new(AtomicU64::new(0));
        let iter_count = Arc::new(AtomicU64::new(0));

        _ = Run::builder()
            .groups(groups)
            .prepare_thread_fn({
                let thread_prepare_count = Arc::clone(&thread_prepare_count);
                move |_| {
                    thread_prepare_count.fetch_add(1, atomic::Ordering::Relaxed);
                }
            })
            .prepare_iter_fn({
                let iter_prepare_count = Arc::clone(&iter_prepare_count);
                move |_, ()| {
                    iter_prepare_count.fetch_add(1, atomic::Ordering::Relaxed);
                }
            })
            .measure_wrapper_fns(
                {
                    let wrapper_begin_count = Arc::clone(&wrapper_begin_count);
                    move |_| {
                        wrapper_begin_count.fetch_add(1, atomic::Ordering::Relaxed);
                    }
                },
                {
                    let wrapper_end_count = Arc::clone(&wrapper_end_count);
                    move |()| {
                        wrapper_end_count.fetch_add(1, atomic::Ordering::Relaxed);
                    }
                },
            )
            .iter_fn({
                let iter_count = Arc::clone(&iter_count);
                move |()| {
                    iter_count.fetch_add(1, atomic::Ordering::Relaxed);
                }
            })
            .build()
            .execute_on(pool, ITERATIONS);

        let expected_total_iterations = ITERATIONS
            .checked_mul(expected_threads as u64)
            .unwrap();

        assert_eq!(
            thread_prepare_count.load(atomic::Ordering::Relaxed),
            expected_threads as u64
        );
        assert_eq!(
            iter_prepare_count.load(atomic::Ordering::Relaxed),
            expected_total_iterations
        );
        assert_eq!(
            wrapper_begin_count.load(atomic::Ordering::Relaxed),
            expected_threads as u64
        );
        assert_eq!(
            wrapper_end_count.load(atomic::Ordering::Relaxed),
            expected_threads as u64
        );
        assert_eq!(
            iter_count.load(atomic::Ordering::Relaxed),
            expected_total_iterations
        );
    }

    #[test]
    fn call_counts_one_processor() {
        test_call_counts(&ONE_THREAD, nz!(1));
    }

    #[test]
    fn call_counts_four_processors_one_group() {
        let Some(pool) = FOUR_THREADS.as_ref() else {
            println!("Skipping test call_counts_four_processors_one_group: not enough processors");
            return; // Skip test if not enough processors.
        };
        test_call_counts(pool, nz!(1));
    }

    #[test]
    fn call_counts_two_processors_two_groups() {
        let Some(pool) = TWO_THREADS.as_ref() else {
            println!("Skipping test call_counts_two_processors_two_groups: not enough processors");
            return; // Skip test if not enough processors.
        };
        test_call_counts(pool, nz!(2));
    }

    #[test]
    fn call_counts_four_processors_two_groups() {
        let Some(pool) = FOUR_THREADS.as_ref() else {
            println!("Skipping test call_counts_four_processors_two_groups: not enough processors");
            return; // Skip test if not enough processors.
        };
        test_call_counts(pool, nz!(2));
    }
}
