use std::hint::black_box;
use std::iter;
use std::num::NonZero;
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use num::Integer;

use crate::builder::{RunBuilderBasic, RunBuilderFinal};
use crate::{RunMeta, ThreadPool};

/// A benchmark run will execute a specific number of multithreaded iterations on a [`ThreadPool`].
///
/// Use `Run::builder()` to prepare a run and configure the callbacks to use during the run,
/// then call `Run::execute_on()` to execute it on a specific thread pool.
#[must_use]
#[derive(Debug)]
pub struct Run<
    ThreadState = (),
    IterState = (),
    MeasureWrapperState = (),
    MeasureOutput = (),
    CleanupState = (),
> where
    ThreadState: 'static,
    MeasureOutput: Send + 'static,
{
    // This type is just a wrapper around the final builder type, for better UX.
    inner:
        RunBuilderFinal<ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>,
}

impl Run {
    /// Returns a new run builder that can be used to prepare a benchmark run.
    pub fn builder() -> RunBuilderBasic {
        RunBuilderBasic::new()
    }
}

impl<ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>
    Run<ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>
where
    MeasureOutput: Send + 'static,
{
    pub(crate) fn new(
        inner: RunBuilderFinal<
            ThreadState,
            IterState,
            MeasureWrapperState,
            MeasureOutput,
            CleanupState,
        >,
    ) -> Self {
        Self { inner }
    }

    /// Executes the benchmark run on the specified thread pool for a specific number of iterations.
    ///
    /// # Panics
    ///
    /// Panics if the thread pool's processor count is not divisible by the number of groups
    /// specified in the benchmark run builder.
    pub fn execute_on(self, pool: &ThreadPool, iterations: u64) -> RunSummary<MeasureOutput> {
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

        // Break the callbacks out of `self` so we do not send `self` to the pool.
        let prepare_thread_fn = self.inner.prepare_thread_fn;
        let prepare_iter_fn = self.inner.prepare_iter_fn;
        let iter_fn = self.inner.iter_fn;
        let measure_wrapper_begin_fn = self.inner.measure_wrapper_begin_fn;
        let measure_wrapper_end_fn = self.inner.measure_wrapper_end_fn;

        let results = pool.execute_task({
            let start = Arc::clone(&start);
            let group_indexes = Arc::clone(&group_indexes);

            move || {
                let group_index = group_indexes.lock().unwrap().pop().unwrap();

                let meta = RunMeta::new(group_index, group_count, iterations);

                let thread_state = prepare_thread_fn(&meta);

                let iterations_usize = usize::try_from(iterations)
                    .expect("iteration count that exceeds virtual memory size is impossible to execute as state would not fit in memory");

                let iter_state = iter::repeat_with(|| {
                    prepare_iter_fn(&meta, &thread_state)
                }).take(iterations_usize).collect::<Vec<_>>();

                let mut cleanup_state = Vec::with_capacity(iterations_usize);

                start.wait();

                let measure_state = measure_wrapper_begin_fn(&meta, &thread_state);

                let start_time = Instant::now();

                for iter_state in iter_state {
                    cleanup_state.push(black_box(iter_fn(black_box(iter_state))));
                }

                let elapsed = start_time.elapsed();

                let measure_output = measure_wrapper_end_fn(measure_state);

                drop(cleanup_state);
                drop(thread_state);

                (elapsed, measure_output)
            }
        });

        let mut total_elapsed_nanos: u128 = 0;
        let mut measure_outputs = Vec::with_capacity(pool.thread_count().get());

        for (elapsed, measure_output) in results {
            total_elapsed_nanos = total_elapsed_nanos.saturating_add(elapsed.as_nanos());
            measure_outputs.push(measure_output);
        }

        RunSummary {
            mean_duration: calculate_mean_duration(pool.thread_count(), total_elapsed_nanos),
            measure_output: measure_outputs.into_boxed_slice(),
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
pub struct RunSummary<MeasureOutput> {
    mean_duration: Duration,

    measure_output: Box<[MeasureOutput]>,
}

impl<MeasureOutput> RunSummary<MeasureOutput> {
    /// Returns the mean duration of the run.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Real timing logic in tests is not desirable.
    pub fn mean_duration(&self) -> Duration {
        self.mean_duration
    }

    /// Returns the output of the measurement wrapper used for the run.
    ///
    /// This will iterate over one measurement output per thread that was involved in the run.
    pub fn measure_outputs(&self) -> impl Iterator<Item = &MeasureOutput> {
        self.measure_output.iter()
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

    static ONE_PROCESSOR: LazyLock<ProcessorSet> =
        LazyLock::new(|| ProcessorSet::builder().take(nz!(1)).unwrap());

    static TWO_PROCESSORS: LazyLock<Option<ProcessorSet>> =
        LazyLock::new(|| ProcessorSet::builder().take(nz!(2)));

    static THREE_PROCESSORS: LazyLock<Option<ProcessorSet>> =
        LazyLock::new(|| ProcessorSet::builder().take(nz!(3)));

    static FOUR_PROCESSORS: LazyLock<Option<ProcessorSet>> =
        LazyLock::new(|| ProcessorSet::builder().take(nz!(4)));

    use super::*;

    #[test]
    fn single_iteration_minimal() {
        let processors = ProcessorSet::builder().take(nz!(1)).unwrap();
        let pool = ThreadPool::new(&processors);
        let iteration_count = Arc::new(AtomicU64::new(0));

        let _result = Run::builder()
            .iter_fn({
                let iteration_count = Arc::clone(&iteration_count);

                move |()| {
                    iteration_count.fetch_add(1, atomic::Ordering::Relaxed);
                }
            })
            .build()
            .execute_on(&pool, 1);

        assert_eq!(iteration_count.load(atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn multiple_iterations_minimal() {
        let processors = ProcessorSet::builder().take(nz!(1)).unwrap();
        let pool = ThreadPool::new(&processors);
        let iteration_count = Arc::new(AtomicU64::new(0));

        let _result = Run::builder()
            .iter_fn({
                let iteration_count = Arc::clone(&iteration_count);

                move |()| {
                    iteration_count.fetch_add(1, atomic::Ordering::Relaxed);
                }
            })
            .build()
            .execute_on(&pool, 9999);

        assert_eq!(iteration_count.load(atomic::Ordering::Relaxed), 9999);
    }

    #[test]
    fn two_processors_two_groups_one_thread_per_group() {
        let Some(processors) = TWO_PROCESSORS.as_ref() else {
            println!(
                "Skipping test two_processors_two_groups_one_thread_per_group: not enough processors"
            );
            return; // Skip test if not enough processors.
        };
        let pool = ThreadPool::new(processors);

        let group_info_seen = Arc::new(Mutex::new(Vec::new()));

        let _result = Run::builder()
            .groups(nz!(2))
            .prepare_thread_fn({
                let group_info_seen = Arc::clone(&group_info_seen);
                move |group_info| {
                    group_info_seen.lock().unwrap().push(*group_info);
                }
            })
            .iter_fn(|()| ())
            .build()
            .execute_on(&pool, 1);

        let mut seen = group_info_seen.lock().unwrap();
        seen.sort_by_key(RunMeta::group_index);

        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].group_index(), 0);
        assert_eq!(seen[0].group_count().get(), 2);
        assert_eq!(seen[1].group_index(), 1);
        assert_eq!(seen[1].group_count().get(), 2);
    }

    #[test]
    fn four_processors_two_groups_two_threads_per_group() {
        let Some(processors) = FOUR_PROCESSORS.as_ref() else {
            println!(
                "Skipping test four_processors_two_groups_two_threads_per_group: not enough processors"
            );
            return; // Skip test if not enough processors.
        };
        let pool = ThreadPool::new(processors);

        let group_info_seen = Arc::new(Mutex::new(Vec::new()));

        let _result = Run::builder()
            .groups(nz!(2))
            .prepare_thread_fn({
                let group_info_seen = Arc::clone(&group_info_seen);
                move |group_info| {
                    group_info_seen.lock().unwrap().push(*group_info);
                }
            })
            .iter_fn(|()| ())
            .build()
            .execute_on(&pool, 1);

        let mut seen = group_info_seen.lock().unwrap();
        seen.sort_by_key(RunMeta::group_index);

        assert_eq!(seen.len(), 4);
        // Two threads should be in group 0.
        assert_eq!(seen[0].group_index(), 0);
        assert_eq!(seen[0].group_count().get(), 2);
        assert_eq!(seen[1].group_index(), 0);
        assert_eq!(seen[1].group_count().get(), 2);
        // Two threads should be in group 1.
        assert_eq!(seen[2].group_index(), 1);
        assert_eq!(seen[2].group_count().get(), 2);
        assert_eq!(seen[3].group_index(), 1);
        assert_eq!(seen[3].group_count().get(), 2);
    }

    #[test]
    fn three_processors_three_groups_one_thread_per_group() {
        let Some(processors) = THREE_PROCESSORS.as_ref() else {
            println!(
                "Skipping test three_processors_three_groups_one_thread_per_group: not enough processors"
            );
            return; // Skip test if not enough processors.
        };
        let pool = ThreadPool::new(processors);

        let group_info_seen = Arc::new(Mutex::new(Vec::new()));

        let _result = Run::builder()
            .groups(nz!(3))
            .prepare_thread_fn({
                let group_info_seen = Arc::clone(&group_info_seen);
                move |group_info| {
                    group_info_seen.lock().unwrap().push(*group_info);
                }
            })
            .iter_fn(|()| ())
            .build()
            .execute_on(&pool, 1);

        let mut seen = group_info_seen.lock().unwrap();
        seen.sort_by_key(RunMeta::group_index);

        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].group_index(), 0);
        assert_eq!(seen[0].group_count().get(), 3);
        assert_eq!(seen[1].group_index(), 1);
        assert_eq!(seen[1].group_count().get(), 3);
        assert_eq!(seen[2].group_index(), 2);
        assert_eq!(seen[2].group_count().get(), 3);
    }

    #[test]
    #[should_panic]
    fn four_processors_three_groups_panics() {
        let Some(processors) = FOUR_PROCESSORS.as_ref() else {
            println!("Skipping test four_processors_three_groups_panics: not enough processors");
            panic!("Skip test if not enough processors by panicking");
        };
        let pool = ThreadPool::new(processors);

        let _result = Run::builder()
            .groups(nz!(3))
            .iter_fn(|()| ())
            .build()
            .execute_on(&pool, 1);
    }

    #[test]
    #[should_panic]
    fn one_processor_two_groups_panics() {
        let pool = ThreadPool::new(&ONE_PROCESSOR);

        let _result = Run::builder()
            .groups(nz!(2))
            .iter_fn(|()| ())
            .build()
            .execute_on(&pool, 1);
    }

    #[test]
    fn state_flow_from_thread_to_iteration_to_cleanup() {
        let pool = ThreadPool::new(&ONE_PROCESSOR);
        let cleanup_states = Arc::new(Mutex::new(Vec::new()));

        let _result = Run::builder()
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
            .execute_on(&pool, 2);

        let states = cleanup_states.lock().unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0], "thread_state_iter_cleanup");
        assert_eq!(states[1], "thread_state_iter_cleanup");
    }

    #[test]
    fn measurement_wrapper_called_before_and_after_timed_execution() {
        let pool = ThreadPool::new(&ONE_PROCESSOR);
        let events = Arc::new(Mutex::new(Vec::new()));

        let result = Run::builder()
            .measure_wrapper_fns(
                {
                    let events = Arc::clone(&events);
                    move |_, ()| {
                        events.lock().unwrap().push("begin".to_string());
                        "wrapper_state".to_string()
                    }
                },
                {
                    let events = Arc::clone(&events);
                    move |wrapper_state| {
                        events.lock().unwrap().push(format!("end_{wrapper_state}"));
                        format!("output_{wrapper_state}")
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
            .execute_on(&pool, 1);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], "begin");
        assert_eq!(events[1], "iteration");
        assert_eq!(events[2], "end_wrapper_state");

        // Verify that the measure output is correctly captured and available.
        let outputs: Vec<_> = result.measure_outputs().collect();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0], "output_wrapper_state");
    }

    #[test]
    fn measure_output_threaded_through_logic() {
        let Some(processors) = TWO_PROCESSORS.as_ref() else {
            println!("Skipping test measure_output_threaded_through_logic: not enough processors");
            return; // Skip test if not enough processors.
        };
        let pool = ThreadPool::new(processors);

        let result = Run::builder()
            .groups(nz!(2))
            .measure_wrapper_fns(
                |group_info, ()| format!("group_{}", group_info.group_index()),
                |state| format!("{state}_output"),
            )
            .iter_fn(|()| ())
            .build()
            .execute_on(&pool, 1);

        let mut outputs: Vec<_> = result.measure_outputs().collect();
        outputs.sort();

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0], "group_0_output");
        assert_eq!(outputs[1], "group_1_output");
    }

    #[test]
    fn cleanup_executed_after_measurement_wrapper_end() {
        let pool = ThreadPool::new(&ONE_PROCESSOR);
        let events = Arc::new(Mutex::new(Vec::new()));

        let _result = Run::builder()
            .measure_wrapper_fns(|_, ()| "wrapper_state".to_string(), {
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
            .execute_on(&pool, 1);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], "wrapper_end");
        assert_eq!(events[1], "cleanup");
    }

    fn test_call_counts(pool: &ThreadPool, groups: NonZero<usize>) {
        const ITERATIONS: u64 = 3;
        let expected_threads = pool.thread_count().get();

        let thread_prepare_count = Arc::new(AtomicU64::new(0));
        let iter_prepare_count = Arc::new(AtomicU64::new(0));
        let wrapper_begin_count = Arc::new(AtomicU64::new(0));
        let wrapper_end_count = Arc::new(AtomicU64::new(0));
        let iter_count = Arc::new(AtomicU64::new(0));

        let _result = Run::builder()
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
                    move |_, ()| {
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

        let expected_total_iterations = ITERATIONS.checked_mul(expected_threads as u64).unwrap();

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
        let pool = ThreadPool::new(&ONE_PROCESSOR);
        test_call_counts(&pool, nz!(1));
    }

    #[test]
    fn call_counts_four_processors_one_group() {
        let Some(processors) = FOUR_PROCESSORS.as_ref() else {
            println!("Skipping test call_counts_four_processors_one_group: not enough processors");
            return; // Skip test if not enough processors.
        };
        let pool = ThreadPool::new(processors);
        test_call_counts(&pool, nz!(1));
    }

    #[test]
    fn call_counts_two_processors_two_groups() {
        let Some(processors) = TWO_PROCESSORS.as_ref() else {
            println!("Skipping test call_counts_two_processors_two_groups: not enough processors");
            return; // Skip test if not enough processors.
        };
        let pool = ThreadPool::new(processors);
        test_call_counts(&pool, nz!(2));
    }

    #[test]
    fn call_counts_four_processors_two_groups() {
        let Some(processors) = FOUR_PROCESSORS.as_ref() else {
            println!("Skipping test call_counts_four_processors_two_groups: not enough processors");
            return; // Skip test if not enough processors.
        };
        let pool = ThreadPool::new(processors);
        test_call_counts(&pool, nz!(2));
    }
}
