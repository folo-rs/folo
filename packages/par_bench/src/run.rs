use std::marker::PhantomData;

use crate::configure::RunInitial;

/// A benchmark run will execute a specific number of multithreaded iterations on every
/// thread of a [`crate::ThreadPool`].
///
/// A `Run` must first be configured, after which it can be executed. The run logic separates
/// preparation (unmeasured) from execution (measured) phases.
///
/// # Execution phases
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
/// use many_cpus::SystemHardware;
/// use par_bench::{Run, ThreadPool};
///
/// # fn main() {
/// let mut pool = ThreadPool::new(&SystemHardware::current().processors());
/// let counter = Arc::new(AtomicU64::new(0));
///
/// let run = Run::new()
///     .prepare_thread({
///         let counter = Arc::clone(&counter);
///         move |_| Arc::clone(&counter)
///     })
///     .prepare_iter(|args| Arc::clone(args.thread_state()))
///     .iter(|mut args| {
///         args.iter_state().fetch_add(1, Ordering::Relaxed);
///     });
///
/// let results = run.execute_on(&mut pool, 1000);
/// println!("Executed in: {:?}", results.mean_duration());
/// # }
/// ```
///
/// With measurement wrapper for custom metrics:
/// ```
/// use std::time::Instant;
///
/// use many_cpus::SystemHardware;
/// use par_bench::{Run, ThreadPool};
///
/// # fn main() {
/// let mut pool = ThreadPool::new(&SystemHardware::current().processors());
///
/// let run = Run::new()
///     .measure_wrapper(|_| Instant::now(), |start| start.elapsed())
///     .iter(|_| {
///         // Simulate some work
///         std::hint::black_box((0..100).sum::<i32>());
///     });
///
/// let results = run.execute_on(&mut pool, 1000);
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
    /// 3. Optionally set thread preparation with [`prepare_thread()`](crate::configure::RunInitial::prepare_thread)
    /// 4. Optionally set iteration preparation with [`prepare_iter()`](crate::configure::RunWithThreadState::prepare_iter)
    /// 5. Optionally set measurement wrappers with [`measure_wrapper()`](crate::configure::RunWithIterState::measure_wrapper)
    /// 6. **Required**: Set the benchmark function with [`iter()`](crate::configure::RunWithWrapperState::iter)
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
    /// let run = Run::new().iter(|_| {
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
