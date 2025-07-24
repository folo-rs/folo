//! Intermediate types for building a [`Run`].
//!
//! These generally do not need to be named or directly referenced in user code - they only exist
//! as intermediate steps in a call chain.
//!
//! # Builder Order Requirements
//!
//! The builder API enforces a specific order of method calls through the type system. This ensures
//! that all required configuration is provided and prevents invalid configurations:
//!
//! 1. **Start**: [`Run::builder()`] returns [`RunBuilderBasic`]
//! 2. **Optional**: Call [`groups()`](RunBuilderBasic::groups) to set thread groups
//! 3. **Thread State**: Call [`prepare_thread_fn()`](RunBuilderBasic::prepare_thread_fn) to configure per-thread state
//! 4. **Iteration State**: Call [`prepare_iter_fn()`](RunBuilderWithThreadState::prepare_iter_fn) to configure per-iteration state  
//! 5. **Measurement**: Call [`measure_wrapper_fns()`](RunBuilderWithIterState::measure_wrapper_fns) to configure measurement
//! 6. **Required**: Call [`iter_fn()`](RunBuilderWithWrapperState::iter_fn) to set the benchmark function
//! 7. **Finish**: Call [`build()`](RunBuilderFinal::build) to create the [`Run`]
//!
//! Each step is optional except for `iter_fn()` and `build()`. However, if you skip a step
//! you cannot go back to it afterwards.
//!
//! # Examples
//!
//! Minimal usage (only required steps):
//! ```
//! use par_bench::Run;
//!
//! let run = Run::builder().iter_fn(|_| { /* benchmark work */ }).build();
//! ```
//!
//! With thread preparation:
//! ```
//! use par_bench::Run;
//!
//! let run = Run::builder()
//!     .prepare_thread_fn(|_meta| "thread_state")
//!     .iter_fn(|_unit: ()| {
//!         // Thread state is used internally
//!         std::hint::black_box(42);
//!     })
//!     .build();
//! ```
//!
//! Full configuration:
//! ```
//! use new_zealand::nz;
//! use par_bench::Run;
//!
//! let run = Run::builder()
//!     .groups(nz!(2))
//!     .prepare_thread_fn(|_meta| "thread_state")
//!     .prepare_iter_fn(|_meta, state| state.to_string())
//!     .measure_wrapper_fns(
//!         |_meta, _state| std::time::Instant::now(),
//!         |start| start.elapsed(),
//!     )
//!     .iter_fn(|iter_state: String| {
//!         // Use the per-iteration string state
//!         std::hint::black_box(iter_state.len());
//!     })
//!     .build();
//! ```

#![allow(
    clippy::ignored_unit_patterns,
    reason = "builder pattern uses intentional unit patterns for no-op defaults"
)]
#![allow(
    clippy::type_complexity,
    reason = "builder pattern uses complex function types for flexibility"
)]

use std::num::NonZero;
use std::sync::Arc;

use new_zealand::nz;

use crate::{Run, RunMeta};

/// The first stage of preparing a benchmark run, with all type parameters unknown.
#[derive(Debug)]
#[must_use]
pub struct RunBuilderBasic {
    groups: NonZero<usize>,
}

/// The second stage of preparing a benchmark run, with the thread state type parameter known.
#[derive(derive_more::Debug)]
#[must_use]
pub struct RunBuilderWithThreadState<ThreadState>
where
    ThreadState: 'static,
{
    groups: NonZero<usize>,

    #[debug(ignore)]
    prepare_thread_fn: Arc<dyn Fn(&RunMeta) -> ThreadState + Send + Sync + 'static>,
}

/// The third stage of preparing a benchmark run, with the iteration state type parameter known.
#[derive(derive_more::Debug)]
#[must_use]
pub struct RunBuilderWithIterState<ThreadState, IterState>
where
    ThreadState: 'static,
{
    groups: NonZero<usize>,

    #[debug(ignore)]
    prepare_thread_fn: Arc<dyn Fn(&RunMeta) -> ThreadState + Send + Sync + 'static>,
    #[debug(ignore)]
    prepare_iter_fn: Arc<dyn Fn(&RunMeta, &ThreadState) -> IterState + Send + Sync + 'static>,
}

/// The fourth stage of preparing a benchmark run, with the wrapper state type parameter known.
#[derive(derive_more::Debug)]
#[must_use]
pub struct RunBuilderWithWrapperState<ThreadState, IterState, MeasureWrapperState, MeasureOutput>
where
    ThreadState: 'static,
    MeasureOutput: Send + 'static,
{
    groups: NonZero<usize>,

    #[debug(ignore)]
    prepare_thread_fn: Arc<dyn Fn(&RunMeta) -> ThreadState + Send + Sync + 'static>,
    #[debug(ignore)]
    prepare_iter_fn: Arc<dyn Fn(&RunMeta, &ThreadState) -> IterState + Send + Sync + 'static>,

    #[debug(ignore)]
    measure_wrapper_begin_fn:
        Arc<dyn Fn(&RunMeta, &ThreadState) -> MeasureWrapperState + Send + Sync + 'static>,
    #[debug(ignore)]
    measure_wrapper_end_fn:
        Arc<dyn Fn(MeasureWrapperState) -> MeasureOutput + Send + Sync + 'static>,
}

/// The final state of preparing a benchmark run, with all type parameters known.
#[derive(derive_more::Debug)]
#[must_use]
pub struct RunBuilderFinal<ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>
where
    ThreadState: 'static,
    MeasureOutput: Send + 'static,
{
    pub(crate) groups: NonZero<usize>,

    #[debug(ignore)]
    pub(crate) prepare_thread_fn: Arc<dyn Fn(&RunMeta) -> ThreadState + Send + Sync + 'static>,
    #[debug(ignore)]
    pub(crate) prepare_iter_fn:
        Arc<dyn Fn(&RunMeta, &ThreadState) -> IterState + Send + Sync + 'static>,

    #[debug(ignore)]
    pub(crate) measure_wrapper_begin_fn:
        Arc<dyn Fn(&RunMeta, &ThreadState) -> MeasureWrapperState + Send + Sync + 'static>,
    #[debug(ignore)]
    pub(crate) measure_wrapper_end_fn:
        Arc<dyn Fn(MeasureWrapperState) -> MeasureOutput + Send + Sync + 'static>,

    #[debug(ignore)]
    pub(crate) iter_fn: Arc<dyn Fn(IterState) -> CleanupState + Send + Sync + 'static>,
}

impl RunBuilderBasic {
    pub(crate) fn new() -> Self {
        Self { groups: nz!(1) }
    }

    /// Divides the threads used for benchmarking into `n` equal groups. Defaults to 1 group.
    ///
    /// The callbacks will be informed of which group they are executing for, and the total number
    /// of groups, via a [`RunMeta`] parameter.
    ///
    /// Attempting to execute a run of a thread pool that is not evenly divisible by `n` will
    /// result in a panic.
    pub fn groups(self, n: NonZero<usize>) -> Self {
        Self { groups: n }
    }

    /// Sets the callback used to prepare thread-scoped state for each thread used for benchmarking.
    ///
    /// The thread-scoped state is later passed by `&mut` exclusive reference to the
    /// "prepare iteration" callback when preparing each iteration.
    ///
    /// **Builder Order**: This method can only be called on [`RunBuilderBasic`]. After calling this,
    /// you must use methods from [`RunBuilderWithThreadState`] for subsequent configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use par_bench::Run;
    ///
    /// let run = Run::builder()
    ///     .prepare_thread_fn(|_meta| "thread_state")
    ///     .iter_fn(|_unit: ()| {
    ///         // Thread state is used internally; iter_fn gets unit type by default
    ///         std::hint::black_box(42);
    ///     })
    ///     .build();
    /// ```
    pub fn prepare_thread_fn<F, ThreadState>(self, f: F) -> RunBuilderWithThreadState<ThreadState>
    where
        F: Fn(&RunMeta) -> ThreadState + Send + Sync + 'static,
    {
        RunBuilderWithThreadState {
            groups: self.groups,
            prepare_thread_fn: Arc::new(f),
        }
    }

    /// Sets the callback used to prepare iteration-scoped state for each benchmark iteration.
    ///
    /// Sets the callback used to prepare iteration-scoped state for each benchmark iteration.
    ///
    /// This callback receives a `&mut` exclusive reference to the thread-scoped state.
    ///
    /// Every iteration is prepared before any iteration is executed.
    ///
    /// **Builder Order**: This method can only be called on a [`RunBuilderBasic`] (no thread state).
    /// After calling this, you must use methods from [`RunBuilderWithIterState`] for subsequent configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use par_bench::Run;
    ///
    /// let run = Run::builder()
    ///     .prepare_iter_fn(|_meta, _unit_state| vec![1, 2, 3])
    ///     .iter_fn(|iter_vec: Vec<i32>| {
    ///         // Use the per-iteration vector
    ///         std::hint::black_box(iter_vec.len());
    ///     })
    ///     .build();
    /// ```
    ///
    /// If you wish to specify a thread preparation function to provide state for each
    /// thread or iteration, do both before calling this method.
    pub fn prepare_iter_fn<F, IterState>(self, f: F) -> RunBuilderWithIterState<(), IterState>
    where
        F: Fn(&RunMeta, &()) -> IterState + Send + Sync + 'static,
    {
        RunBuilderWithIterState {
            groups: self.groups,
            prepare_thread_fn: Arc::new(|_| ()),
            prepare_iter_fn: Arc::new(f),
        }
    }

    /// Sets the callbacks used to wrap the measured part of a benchmark run, encompassing the
    /// execution (but not the preparation) of every iteration.
    ///
    /// The `f_begin` callback is called before the first iteration of each thread, and the
    /// `f_end` callback is called after the last iteration of each thread. Any return value
    /// from the former is passed to the latter.
    ///
    /// If you wish to specify a thread or iteration preparation function to provide state for each
    /// iteration, do it before calling this method.
    pub fn measure_wrapper_fns<FBegin, FEnd, MeasureWrapperState, MeasureOutput>(
        self,
        f_begin: FBegin,
        f_end: FEnd,
    ) -> RunBuilderWithWrapperState<(), (), MeasureWrapperState, MeasureOutput>
    where
        FBegin: Fn(&RunMeta, &()) -> MeasureWrapperState + Send + Sync + 'static,
        FEnd: Fn(MeasureWrapperState) -> MeasureOutput + Send + Sync + 'static,
        MeasureOutput: Send + 'static,
    {
        RunBuilderWithWrapperState {
            groups: self.groups,
            prepare_thread_fn: Arc::new(|_| ()),
            prepare_iter_fn: Arc::new(|_, _| ()),
            measure_wrapper_begin_fn: Arc::new(f_begin),
            measure_wrapper_end_fn: Arc::new(f_end),
        }
    }

    /// Sets the callback used to execute the actual benchmark iterations.
    ///
    /// This callback may return a value to drop after the measured part of the benchmark
    /// run (to exclude any cleanup logic from measurement).
    ///
    /// This must be the last step in preparing a benchmark run. After this,
    /// you can only call `build()` on the builder.
    ///
    /// If you wish to specify a thread or iteration preparation function to provide state for each
    /// thread or iteration, or a specify a measurement wrapper function, do all of these before
    /// calling this method.
    pub fn iter_fn<F, CleanupState>(self, f: F) -> RunBuilderFinal<(), (), (), (), CleanupState>
    where
        F: Fn(()) -> CleanupState + Send + Sync + 'static,
    {
        RunBuilderFinal {
            groups: self.groups,
            prepare_thread_fn: Arc::new(|_| ()),
            prepare_iter_fn: Arc::new(|_, _| ()),
            measure_wrapper_begin_fn: Arc::new(|_, _| ()),
            measure_wrapper_end_fn: Arc::new(|_| ()),
            iter_fn: Arc::new(f),
        }
    }
}

impl<ThreadState> RunBuilderWithThreadState<ThreadState>
where
    ThreadState: 'static,
{
    /// Divides the threads used for benchmarking into `n` equal groups. Defaults to 1 group.
    ///
    /// The callbacks will be informed of which group they are executing for, and the total number
    /// of groups, via a [`RunMeta`] parameter.
    ///
    /// Attempting to execute a run of a thread pool that is not evenly divisible by `n` will
    /// result in a panic.
    pub fn groups(self, n: NonZero<usize>) -> Self {
        Self { groups: n, ..self }
    }

    /// Sets the callback used to prepare iteration-scoped state for each benchmark iteration.
    ///
    /// This callback receives a `&mut` exclusive reference to the thread-scoped state.
    ///
    /// Every iteration is prepared before any iteration is executed.
    ///
    /// **Builder Order**: This method can only be called after [`prepare_thread_fn()`](RunBuilderBasic::prepare_thread_fn).
    /// After calling this, you must use methods from [`RunBuilderWithIterState`] for subsequent configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use par_bench::Run;
    ///
    /// let run = Run::builder()
    ///     .prepare_thread_fn(|_meta| vec![1, 2, 3])
    ///     .prepare_iter_fn(|_meta, vec| vec.clone())
    ///     .iter_fn(|iter_vec: Vec<i32>| {
    ///         // Use the per-iteration vector
    ///         std::hint::black_box(iter_vec.len());
    ///     })
    ///     .build();
    /// ```
    pub fn prepare_iter_fn<F, IterState>(
        self,
        f: F,
    ) -> RunBuilderWithIterState<ThreadState, IterState>
    where
        F: Fn(&RunMeta, &ThreadState) -> IterState + Send + Sync + 'static,
    {
        RunBuilderWithIterState {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: Arc::new(f),
        }
    }

    /// Sets the callbacks used to wrap the measured part of a benchmark run, encompassing the
    /// execution (but not the preparation) of every iteration.
    ///
    /// The `f_begin` callback is called before the first iteration of each thread, and the
    /// `f_end` callback is called after the last iteration of each thread. Any return value
    /// from the former is passed to the latter.
    ///
    /// If you wish to specify an iteration preparation function to provide state for each
    /// iteration, do it before calling this method.
    pub fn measure_wrapper_fns<FBegin, FEnd, MeasureWrapperState, MeasureOutput>(
        self,
        f_begin: FBegin,
        f_end: FEnd,
    ) -> RunBuilderWithWrapperState<ThreadState, (), MeasureWrapperState, MeasureOutput>
    where
        FBegin: Fn(&RunMeta, &ThreadState) -> MeasureWrapperState + Send + Sync + 'static,
        FEnd: Fn(MeasureWrapperState) -> MeasureOutput + Send + Sync + 'static,
        MeasureOutput: Send + 'static,
    {
        RunBuilderWithWrapperState {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: Arc::new(|_, _| ()),
            measure_wrapper_begin_fn: Arc::new(f_begin),
            measure_wrapper_end_fn: Arc::new(f_end),
        }
    }

    /// Sets the callback used to execute the actual benchmark iterations.
    ///
    /// This callback may return a value to drop after the measured part of the benchmark
    /// run (to exclude any cleanup logic from measurement).
    ///
    /// **Builder Order**: This is the final required step in preparing a benchmark run. After this,
    /// you can only call [`build()`](RunBuilderFinal::build) on the builder.
    ///
    /// Must be called after:
    /// 1. [`prepare_thread_fn()`](RunBuilderBasic::prepare_thread_fn) (optional but recommended)
    /// 2. Any optional measure wrapper configuration
    ///
    /// # Examples
    ///
    /// ```
    /// use par_bench::Run;
    ///
    /// let run = Run::builder()
    ///     .prepare_thread_fn(|_meta| vec![1, 2, 3])
    ///     .iter_fn(|_unit_state| {
    ///         // Execute the benchmark iteration
    ///         std::hint::black_box(42);
    ///         // Return value to drop after measurement
    ///         "cleanup_data".to_string()
    ///     })
    ///     .build();
    /// ```
    ///
    /// If you wish to specify an iteration preparation function to provide state for each
    /// iteration, do it before calling this method.
    pub fn iter_fn<F, CleanupState>(
        self,
        f: F,
    ) -> RunBuilderFinal<ThreadState, (), (), (), CleanupState>
    where
        F: Fn(()) -> CleanupState + Send + Sync + 'static,
    {
        RunBuilderFinal {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: Arc::new(|_, _| ()),
            measure_wrapper_begin_fn: Arc::new(|_, _| ()),
            measure_wrapper_end_fn: Arc::new(|_| ()),
            iter_fn: Arc::new(f),
        }
    }
}

impl<ThreadState, IterState> RunBuilderWithIterState<ThreadState, IterState>
where
    ThreadState: 'static,
{
    /// Divides the threads used for benchmarking into `n` equal groups. Defaults to 1 group.
    ///
    /// The callbacks will be informed of which group they are executing for, and the total number
    /// of groups, via a [`RunMeta`] parameter.
    ///
    /// Attempting to execute a run of a thread pool that is not evenly divisible by `n` will
    /// result in a panic.
    pub fn groups(self, n: NonZero<usize>) -> Self {
        Self { groups: n, ..self }
    }

    /// Sets the callbacks used to wrap the measured part of a benchmark run, encompassing the
    /// execution (but not the preparation) of every iteration.
    ///
    /// The `f_begin` callback is called before the first iteration of each thread, and the
    /// `f_end` callback is called after the last iteration of each thread. Any return value
    /// from the former is passed to the latter.
    pub fn measure_wrapper_fns<FBegin, FEnd, MeasureWrapperState, MeasureOutput>(
        self,
        f_begin: FBegin,
        f_end: FEnd,
    ) -> RunBuilderWithWrapperState<ThreadState, IterState, MeasureWrapperState, MeasureOutput>
    where
        FBegin: Fn(&RunMeta, &ThreadState) -> MeasureWrapperState + Send + Sync + 'static,
        FEnd: Fn(MeasureWrapperState) -> MeasureOutput + Send + Sync + 'static,
        MeasureOutput: Send + 'static,
    {
        RunBuilderWithWrapperState {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: self.prepare_iter_fn,
            measure_wrapper_begin_fn: Arc::new(f_begin),
            measure_wrapper_end_fn: Arc::new(f_end),
        }
    }

    /// Sets the callback used to execute the actual benchmark iterations.
    ///
    /// This callback receives the iteration-scoped state and may return a value to drop
    /// after the measured part of the benchmark run (to exclude any cleanup logic
    /// from measurement).
    ///
    /// This must be the last step in preparing a benchmark run. After this,
    /// you can only call `build()` on the builder.
    ///
    /// If you wish to specify a thread wrapper function pair, do it before calling this method.
    pub fn iter_fn<F, CleanupState>(
        self,
        f: F,
    ) -> RunBuilderFinal<ThreadState, IterState, (), (), CleanupState>
    where
        F: Fn(IterState) -> CleanupState + Send + Sync + 'static,
    {
        RunBuilderFinal {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: self.prepare_iter_fn,
            measure_wrapper_begin_fn: Arc::new(|_, _| ()),
            measure_wrapper_end_fn: Arc::new(|_| ()),
            iter_fn: Arc::new(f),
        }
    }
}

impl<ThreadState, IterState, MeasureWrapperState, MeasureOutput>
    RunBuilderWithWrapperState<ThreadState, IterState, MeasureWrapperState, MeasureOutput>
where
    ThreadState: 'static,
    MeasureOutput: Send + 'static,
{
    /// Divides the threads used for benchmarking into `n` equal groups. Defaults to 1 group.
    ///
    /// The callbacks will be informed of which group they are executing for, and the total number
    /// of groups, via a [`RunMeta`] parameter.
    ///
    /// Attempting to execute a run of a thread pool that is not evenly divisible by `n` will
    /// result in a panic.
    pub fn groups(self, n: NonZero<usize>) -> Self {
        Self { groups: n, ..self }
    }

    /// Sets the callback used to execute the actual benchmark iterations.
    ///
    /// This callback receives the iteration-scoped state and may return a value to drop
    /// after the measured part of the benchmark run (to exclude any cleanup logic
    /// from measurement).
    ///
    /// This must be the last step in preparing a benchmark run. After this,
    /// you can only call `build()` on the builder.
    pub fn iter_fn<F, CleanupState>(
        self,
        f: F,
    ) -> RunBuilderFinal<ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>
    where
        F: Fn(IterState) -> CleanupState + Send + Sync + 'static,
    {
        RunBuilderFinal {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: self.prepare_iter_fn,
            measure_wrapper_begin_fn: self.measure_wrapper_begin_fn,
            measure_wrapper_end_fn: self.measure_wrapper_end_fn,
            iter_fn: Arc::new(f),
        }
    }
}

impl<ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>
    RunBuilderFinal<ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>
where
    ThreadState: 'static,
    MeasureOutput: Send + 'static,
{
    /// Completes the preparation of a benchmark run and returns a [`Run`] instance that can be
    /// used to execute the run on a specific thread pool.
    ///
    /// **Builder Order**: This is the final step that can only be called after [`Self::iter_fn()`] has been configured.
    /// The returned [`Run`] is ready for execution via [`Run::execute_on()`](crate::Run::execute_on).
    ///
    /// # Examples
    ///
    /// ```
    /// use par_bench::Run;
    ///
    /// let run = Run::builder()
    ///     .prepare_thread_fn(|_meta| vec![1, 2, 3])
    ///     .iter_fn(|_unit_state| {
    ///         std::hint::black_box(42);
    ///     })
    ///     .build(); // Final step - creates executable Run
    ///
    /// // Run is now ready for execution
    /// # use par_bench::ThreadPool;
    /// # use many_cpus::ProcessorSet;
    /// # use new_zealand::nz;
    /// # if let Some(processors) = ProcessorSet::builder().take(nz!(1)) {
    /// #     let pool = ThreadPool::new(&processors);
    /// #     let _result = run.execute_on(&pool, 100);
    /// # }
    /// ```
    pub fn build(
        self,
    ) -> Run<ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState> {
        // If we need to, we can do some validation here in the future. Right now,
        // we have no need for this - everything is guarded via the type system.

        Run::<ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>::new(self)
    }
}

// In this file, we have unit tests for ensuring that the different stages are callable
// in the correct manner. However, we do not have tests here that validate the correctness
// of the run execution itself, as that is done in the `run` module.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_does_not_panic() {
        drop(Run::builder().iter_fn(|()| ()).build());
    }
}
