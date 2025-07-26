use std::marker::PhantomData;

use crate::configure::RunInitial;

/// A benchmark run will execute a specific number of multithreaded iterations on every
/// thread of a [`crate::ThreadPool`].
///
/// A `Run` must first be configured, after which it can be executed. The run logic separates
/// preparation (unmeasured) from execution (measured) phases.
///
/// # Execution Phases
///
/// 1. **Thread Preparation**: Each thread executes the thread preparation callback once
/// 2. **Iteration Preparation**: Each thread prepares state for every iteration (unmeasured)  
/// 3. **Measurement Begin**: Measurement wrapper begin callback is called per thread
/// 4. **Iteration Execution**: All iterations are executed (measured)
/// 5. **Measurement End**: Measurement wrapper end callback is called per thread
/// 6. **Cleanup**: All cleanup state is dropped (unmeasured)
///
/// # Examples
///
/// Basic usage with atomic counter:
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicU64, Ordering};
///
/// use par_bench::{Run, ThreadPool};
///
/// # fn main() {
/// let pool = ThreadPool::default();
/// let counter = Arc::new(AtomicU64::new(0));
///
/// let run = Run::new()
///     .prepare_thread_fn({
///         let counter = Arc::clone(&counter);
///         move |_meta| Arc::clone(&counter)
///     })
///     .prepare_iter_fn(|_meta, counter| Arc::clone(counter))
///     .iter_fn(|counter: Arc<AtomicU64>| {
///         counter.fetch_add(1, Ordering::Relaxed);
///     });
///
/// let results = run.execute_on(&pool, 1000);
/// println!("Executed in: {:?}", results.mean_duration());
/// # }
/// ```
///
/// With measurement wrapper for custom metrics:
/// ```
/// use std::time::Instant;
///
/// use par_bench::{Run, ThreadPool};
///
/// # fn main() {
/// let pool = ThreadPool::default();
///
/// let run = Run::new()
///     .measure_wrapper_fns(|_meta, _state| Instant::now(), |start| start.elapsed())
///     .iter_fn(|_| {
///         // Simulate some work
///         std::hint::black_box((0..100).sum::<i32>());
///     });
///
/// let results = run.execute_on(&pool, 1000);
///
/// // Access per-thread measurement data
/// for elapsed in results.measure_outputs() {
///     println!("Thread execution time: {:?}", elapsed);
/// }
/// # }
/// ```
#[derive(Debug)]
pub struct Run {
    _no_construct: PhantomData<()>,
}

impl Run {
    /// Creates a new benchmark run and starts the process of configuring it.
    ///
    /// The type uses a fluent API with enforced ordering to configure the different callback
    /// functions that will be executed during the benchmark run. The type system ensures that
    /// methods are called in the correct order and that all required configuration is provided.
    ///
    /// # Order of operations
    ///
    /// 1. Start with `Run::new()`, which gives you an object you can use to configure the run.
    /// 2. Optionally configure thread groups with [`groups()`](crate::configure::RunInitial::groups)
    /// 3. Optionally set thread preparation with [`prepare_thread_fn()`](crate::configure::RunInitial::prepare_thread_fn)
    /// 4. Optionally set iteration preparation with [`prepare_iter_fn()`](crate::configure::RunWithThreadState::prepare_iter_fn)
    /// 5. Optionally set measurement wrappers with [`measure_wrapper_fns()`](crate::configure::RunWithIterState::measure_wrapper_fns)
    /// 6. **Required**: Set the benchmark function with [`iter_fn()`](crate::configure::RunWithWrapperState::iter_fn)
    /// 7. **Required**: Execute the run with either [`execute_on()`][crate::ConfiguredRun::execute_on]
    ///    or [`execute_criterion_on()`][crate::ConfiguredRun::execute_criterion_on].
    ///
    /// You can skip optional steps but cannot go back in the sequence.
    ///
    /// # Examples
    ///
    /// ```
    /// use par_bench::Run;
    ///
    /// let run = Run::new().iter_fn(|_| {
    ///     // Benchmark work goes here
    ///     std::hint::black_box(42 * 42);
    /// });
    /// ```
    #[expect(
        clippy::new_ret_no_self,
        reason = "builder-style configuration pattern, intentional"
    )]
    pub fn new() -> RunInitial {
        RunInitial::new()
    }
}
