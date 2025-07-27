//! Intermediate stages of configuring a benchmark run.
//!
//! You generally do not need to reference these types, they are just parts of a call chain.

#![allow(
    clippy::ignored_unit_patterns,
    reason = "builder pattern uses intentional unit patterns for no-op defaults"
)]
#![allow(
    clippy::type_complexity,
    reason = "builder pattern uses complex function types for flexibility"
)]
#![allow(
    clippy::iter_not_returning_iterator,
    reason = "this is a different kind of iter(), iterators do not have a monopoly"
)]

use std::num::NonZero;

use new_zealand::nz;

use crate::{ConfiguredRun, args};

/// The first stage of configuring a benchmark run, with all type parameters unknown.
#[derive(Debug)]
#[must_use]
pub struct RunInitial {
    groups: NonZero<usize>,
}

/// The second stage of configuring a benchmark run, with the thread state type parameter known.
#[derive(derive_more::Debug)]
#[must_use]
pub struct RunWithThreadState<'a, ThreadState> {
    groups: NonZero<usize>,

    #[debug(ignore)]
    prepare_thread_fn: Box<dyn Fn(args::PrepareThread<'_>) -> ThreadState + Send + Sync + 'a>,
}

/// The third stage of configuring a benchmark run, with the iteration state type parameter known.
#[derive(derive_more::Debug)]
#[must_use]
pub struct RunWithIterState<'a, ThreadState, IterState> {
    groups: NonZero<usize>,

    #[debug(ignore)]
    prepare_thread_fn: Box<dyn Fn(args::PrepareThread<'_>) -> ThreadState + Send + Sync + 'a>,
    #[debug(ignore)]
    prepare_iter_fn:
        Box<dyn Fn(args::PrepareIter<'_, ThreadState>) -> IterState + Send + Sync + 'a>,
}

/// The fourth stage of configuring a benchmark run, with the wrapper state type parameter known.
#[derive(derive_more::Debug)]
#[must_use]
pub struct RunWithWrapperState<'a, ThreadState, IterState, MeasureWrapperState, MeasureOutput>
where
    MeasureOutput: Send + 'static,
{
    groups: NonZero<usize>,

    #[debug(ignore)]
    prepare_thread_fn: Box<dyn Fn(args::PrepareThread<'_>) -> ThreadState + Send + Sync + 'a>,
    #[debug(ignore)]
    prepare_iter_fn:
        Box<dyn Fn(args::PrepareIter<'_, ThreadState>) -> IterState + Send + Sync + 'a>,

    #[debug(ignore)]
    measure_wrapper_begin_fn: Box<
        dyn Fn(args::MeasureWrapperBegin<'_, ThreadState>) -> MeasureWrapperState
            + Send
            + Sync
            + 'a,
    >,
    #[debug(ignore)]
    measure_wrapper_end_fn: Box<dyn Fn(MeasureWrapperState) -> MeasureOutput + Send + Sync + 'a>,
}

impl RunInitial {
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
    /// The thread-scoped state is later passed by shared reference to the
    /// "prepare iteration" callback when preparing each iteration.
    ///
    /// **Builder Order**: This method can only be called on [`RunInitial`]. After calling this,
    /// you must use methods from [`RunWithThreadState`] for subsequent configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use par_bench::Run;
    ///
    /// let run = Run::new()
    ///     .prepare_thread_fn(|_meta| "thread_state")
    ///     .iter_fn(|_unit: ()| {
    ///         // Thread state is used internally; iter_fn gets unit type by default
    ///         std::hint::black_box(42);
    ///     });
    /// ```
    pub fn prepare_thread<'a, F, ThreadState>(self, f: F) -> RunWithThreadState<'a, ThreadState>
    where
        F: Fn(args::PrepareThread<'_>) -> ThreadState + Send + Sync + 'a,
    {
        RunWithThreadState {
            groups: self.groups,
            prepare_thread_fn: Box::new(f),
        }
    }

    /// Sets the callback used to prepare iteration-scoped state for each benchmark iteration.
    ///
    /// Sets the callback used to prepare iteration-scoped state for each benchmark iteration.
    ///
    /// This callback receives a shared reference to the thread-scoped state.
    ///
    /// Every iteration is prepared before any iteration is executed.
    ///
    /// **Builder Order**: This method can only be called on a [`RunInitial`] (no thread state).
    /// After calling this, you must use methods from [`RunWithIterState`] for subsequent configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use par_bench::Run;
    ///
    /// let run = Run::new()
    ///     .prepare_iter_fn(|_meta, _unit_state| vec![1, 2, 3])
    ///     .iter_fn(|iter_vec: Vec<i32>| {
    ///         // Use the per-iteration vector
    ///         std::hint::black_box(iter_vec.len());
    ///     });
    /// ```
    ///
    /// If you wish to specify a thread preparation function to provide state for each
    /// thread or iteration, do both before calling this method.
    pub fn prepare_iter<'a, F, IterState>(self, f: F) -> RunWithIterState<'a, (), IterState>
    where
        F: Fn(args::PrepareIter<'_, ()>) -> IterState + Send + Sync + 'a,
    {
        RunWithIterState {
            groups: self.groups,
            prepare_thread_fn: Box::new(|_| ()),
            prepare_iter_fn: Box::new(f),
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
    pub fn measure_wrapper<'a, FBegin, FEnd, MeasureWrapperState, MeasureOutput>(
        self,
        f_begin: FBegin,
        f_end: FEnd,
    ) -> RunWithWrapperState<'a, (), (), MeasureWrapperState, MeasureOutput>
    where
        FBegin: Fn(args::MeasureWrapperBegin<'_, ()>) -> MeasureWrapperState + Send + Sync + 'a,
        FEnd: Fn(MeasureWrapperState) -> MeasureOutput + Send + Sync + 'a,
        MeasureOutput: Send + 'static,
    {
        RunWithWrapperState {
            groups: self.groups,
            prepare_thread_fn: Box::new(|_| ()),
            prepare_iter_fn: Box::new(|_| ()),
            measure_wrapper_begin_fn: Box::new(f_begin),
            measure_wrapper_end_fn: Box::new(f_end),
        }
    }

    /// Sets the callback used to execute the actual benchmark iterations.
    ///
    /// This callback may return a value to drop after the measured part of the benchmark
    /// run (to exclude any cleanup logic from measurement).
    ///
    /// This must be the last step in preparing a benchmark run. After this,
    /// the only action permitted is to execute the run.
    ///
    /// If you wish to specify a thread or iteration preparation function to provide state for each
    /// thread or iteration, or a specify a measurement wrapper function, do all of these before
    /// calling this method.
    pub fn iter<'a, F, CleanupState>(self, f: F) -> ConfiguredRun<'a, (), (), (), (), CleanupState>
    where
        F: Fn(args::Iter<'_, (), ()>) -> CleanupState + Send + Sync + 'a,
    {
        ConfiguredRun {
            groups: self.groups,
            prepare_thread_fn: Box::new(|_| ()),
            prepare_iter_fn: Box::new(|_| ()),
            measure_wrapper_begin_fn: Box::new(|_| ()),
            measure_wrapper_end_fn: Box::new(|_| ()),
            iter_fn: Box::new(f),
        }
    }
}

impl<'a, ThreadState> RunWithThreadState<'a, ThreadState> {
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
    /// This callback receives a shared reference to the thread-scoped state.
    ///
    /// Every iteration is prepared before any iteration is executed.
    ///
    /// **Builder Order**: This method can only be called after [`prepare_thread_fn()`](RunInitial::prepare_thread_fn).
    /// After calling this, you must use methods from [`RunWithIterState`] for subsequent configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use par_bench::Run;
    ///
    /// let run = Run::new()
    ///     .prepare_thread_fn(|_meta| vec![1, 2, 3])
    ///     .prepare_iter_fn(|_meta, vec| vec.clone())
    ///     .iter_fn(|iter_vec: Vec<i32>| {
    ///         // Use the per-iteration vector
    ///         std::hint::black_box(iter_vec.len());
    ///     });
    /// ```
    pub fn prepare_iter<F, IterState>(self, f: F) -> RunWithIterState<'a, ThreadState, IterState>
    where
        F: Fn(args::PrepareIter<'_, ThreadState>) -> IterState + Send + Sync + 'a,
    {
        RunWithIterState {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: Box::new(f),
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
    pub fn measure_wrapper<FBegin, FEnd, MeasureWrapperState, MeasureOutput>(
        self,
        f_begin: FBegin,
        f_end: FEnd,
    ) -> RunWithWrapperState<'a, ThreadState, (), MeasureWrapperState, MeasureOutput>
    where
        FBegin: Fn(args::MeasureWrapperBegin<'_, ThreadState>) -> MeasureWrapperState
            + Send
            + Sync
            + 'a,
        FEnd: Fn(MeasureWrapperState) -> MeasureOutput + Send + Sync + 'a,
        MeasureOutput: Send + 'static,
    {
        RunWithWrapperState {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: Box::new(|_| ()),
            measure_wrapper_begin_fn: Box::new(f_begin),
            measure_wrapper_end_fn: Box::new(f_end),
        }
    }

    /// Sets the callback used to execute the actual benchmark iterations.
    ///
    /// This callback may return a value to drop after the measured part of the benchmark
    /// run (to exclude any cleanup logic from measurement).
    ///
    /// **Builder Order**: This is the final required step in preparing a benchmark run. After this,
    /// you can call [`execute_on()`](crate::ConfiguredRun::execute_on) to run the benchmark.
    ///
    /// Must be called after:
    /// 1. [`prepare_thread_fn()`](RunInitial::prepare_thread_fn) (optional but recommended)
    /// 2. Any optional measure wrapper configuration
    ///
    /// # Examples
    ///
    /// ```
    /// use par_bench::Run;
    ///
    /// let run = Run::new()
    ///     .prepare_thread_fn(|_meta| vec![1, 2, 3])
    ///     .iter_fn(|_unit_state| {
    ///         // Execute the benchmark iteration
    ///         std::hint::black_box(42);
    ///         // Return value to drop after measurement
    ///         "cleanup_data".to_string()
    ///     });
    /// ```
    ///
    /// If you wish to specify an iteration preparation function to provide state for each
    /// iteration, do it before calling this method.
    pub fn iter<F, CleanupState>(
        self,
        f: F,
    ) -> ConfiguredRun<'a, ThreadState, (), (), (), CleanupState>
    where
        F: Fn(args::Iter<'_, ThreadState, ()>) -> CleanupState + Send + Sync + 'a,
    {
        ConfiguredRun {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: Box::new(|_| ()),
            measure_wrapper_begin_fn: Box::new(|_| ()),
            measure_wrapper_end_fn: Box::new(|_| ()),
            iter_fn: Box::new(f),
        }
    }
}

impl<'a, ThreadState, IterState> RunWithIterState<'a, ThreadState, IterState> {
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
    pub fn measure_wrapper<FBegin, FEnd, MeasureWrapperState, MeasureOutput>(
        self,
        f_begin: FBegin,
        f_end: FEnd,
    ) -> RunWithWrapperState<'a, ThreadState, IterState, MeasureWrapperState, MeasureOutput>
    where
        FBegin: Fn(args::MeasureWrapperBegin<'_, ThreadState>) -> MeasureWrapperState
            + Send
            + Sync
            + 'a,
        FEnd: Fn(MeasureWrapperState) -> MeasureOutput + Send + Sync + 'a,
        MeasureOutput: Send + 'static,
    {
        RunWithWrapperState {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: self.prepare_iter_fn,
            measure_wrapper_begin_fn: Box::new(f_begin),
            measure_wrapper_end_fn: Box::new(f_end),
        }
    }

    /// Sets the callback used to execute the actual benchmark iterations.
    ///
    /// This callback receives the iteration-scoped state and may return a value to drop
    /// after the measured part of the benchmark run (to exclude any cleanup logic
    /// from measurement).
    ///
    /// This must be the last step in preparing a benchmark run. After this,
    /// the only action permitted is to execute the run.
    ///
    /// If you wish to specify a thread wrapper function pair, do it before calling this method.
    pub fn iter<F, CleanupState>(
        self,
        f: F,
    ) -> ConfiguredRun<'a, ThreadState, IterState, (), (), CleanupState>
    where
        F: Fn(args::Iter<'_, ThreadState, IterState>) -> CleanupState + Send + Sync + 'a,
    {
        ConfiguredRun {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: self.prepare_iter_fn,
            measure_wrapper_begin_fn: Box::new(|_| ()),
            measure_wrapper_end_fn: Box::new(|_| ()),
            iter_fn: Box::new(f),
        }
    }
}

impl<'a, ThreadState, IterState, MeasureWrapperState, MeasureOutput>
    RunWithWrapperState<'a, ThreadState, IterState, MeasureWrapperState, MeasureOutput>
where
    MeasureOutput: Send,
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
    /// the only action permitted is to execute the run.
    pub fn iter<F, CleanupState>(
        self,
        f: F,
    ) -> ConfiguredRun<'a, ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>
    where
        F: Fn(args::Iter<'_, ThreadState, IterState>) -> CleanupState + Send + Sync + 'a,
    {
        ConfiguredRun {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: self.prepare_iter_fn,
            measure_wrapper_begin_fn: self.measure_wrapper_begin_fn,
            measure_wrapper_end_fn: self.measure_wrapper_end_fn,
            iter_fn: Box::new(f),
        }
    }
}
