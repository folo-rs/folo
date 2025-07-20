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

use crate::Run;

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
    prepare_thread_fn: Arc<dyn Fn(&GroupInfo) -> ThreadState + Send + Sync>,
}

/// The third stage of preparing a benchmark run, with the iteration state type parameter known.
#[derive(derive_more::Debug)]
#[must_use]
pub struct RunBuilderWithIterState<ThreadState, IterState>
where
    ThreadState: 'static,
    IterState: 'static,
{
    groups: NonZero<usize>,

    #[debug(ignore)]
    prepare_thread_fn: Arc<dyn Fn(&GroupInfo) -> ThreadState + Send + Sync>,
    #[debug(ignore)]
    prepare_iter_fn: Arc<dyn Fn(&GroupInfo, &mut ThreadState) -> IterState + Send + Sync>,
}

/// The fourth stage of preparing a benchmark run, with the wrapper state type parameter known.
#[derive(derive_more::Debug)]
#[must_use]
pub struct RunBuilderWithWrapperState<ThreadState, IterState, MeasureWrapperState>
where
    ThreadState: 'static,
    IterState: 'static,
    MeasureWrapperState: 'static,
{
    groups: NonZero<usize>,

    #[debug(ignore)]
    prepare_thread_fn: Arc<dyn Fn(&GroupInfo) -> ThreadState + Send + Sync>,
    #[debug(ignore)]
    prepare_iter_fn: Arc<dyn Fn(&GroupInfo, &mut ThreadState) -> IterState + Send + Sync>,

    #[debug(ignore)]
    measure_wrapper_begin_fn: Arc<dyn Fn(&GroupInfo) -> MeasureWrapperState + Send + Sync>,
    #[debug(ignore)]
    measure_wrapper_end_fn: Arc<dyn Fn(MeasureWrapperState) + Send + Sync>,
}

/// The final state of preparing a benchmark run, with all type parameters known.
#[derive(derive_more::Debug)]
#[must_use]
pub struct RunBuilderFinal<ThreadState, IterState, MeasureWrapperState, CleanupState>
where
    ThreadState: 'static,
    IterState: 'static,
    MeasureWrapperState: 'static,
    CleanupState: 'static,
{
    pub(crate) groups: NonZero<usize>,

    #[debug(ignore)]
    pub(crate) prepare_thread_fn: Arc<dyn Fn(&GroupInfo) -> ThreadState + Send + Sync>,
    #[debug(ignore)]
    pub(crate) prepare_iter_fn:
        Arc<dyn Fn(&GroupInfo, &mut ThreadState) -> IterState + Send + Sync>,

    #[debug(ignore)]
    pub(crate) measure_wrapper_begin_fn:
        Arc<dyn Fn(&GroupInfo) -> MeasureWrapperState + Send + Sync>,
    #[debug(ignore)]
    pub(crate) measure_wrapper_end_fn: Arc<dyn Fn(MeasureWrapperState) + Send + Sync>,

    #[debug(ignore)]
    pub(crate) iter_fn: Arc<dyn Fn(IterState) -> CleanupState + Send + Sync>,
}

/// Informs a benchmark run callback which thread group it is currently executing for,
/// out of how many total.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupInfo {
    /// The index of the current thread group, starting from 0.
    index: usize,

    /// The total number of thread groups in the run.
    count: NonZero<usize>,
}

impl GroupInfo {
    /// Creates a new `GroupInfo` with the specified index and total count.
    pub(crate) fn new(index: usize, count: NonZero<usize>) -> Self {
        Self { index, count }
    }

    /// Returns the index of the current thread group, starting from 0.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns the total number of thread groups in the run.
    #[must_use]
    pub fn count(&self) -> NonZero<usize> {
        self.count
    }
}

impl RunBuilderBasic {
    pub(crate) fn new() -> Self {
        Self { groups: nz!(1) }
    }

    /// Divides the threads used for benchmarking into `n` equal groups. Defaults to 1 group.
    ///
    /// The callbacks will be informed of which group they are executing for, and the total number
    /// of groups, via a [`GroupInfo`] parameter.
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
    pub fn prepare_thread_fn<F, ThreadState>(self, f: F) -> RunBuilderWithThreadState<ThreadState>
    where
        F: Fn(&GroupInfo) -> ThreadState + Send + Sync + 'static,
    {
        RunBuilderWithThreadState {
            groups: self.groups,
            prepare_thread_fn: Arc::new(f),
        }
    }

    /// Sets the callback used to prepare iteration-scoped state for each benchmark iteration.
    ///
    /// This callback receives a `&mut` exclusive reference to the thread-scoped state.
    ///
    /// Every iteration is prepared before any iteration is executed.
    ///
    /// If you wish to specify a thread preparation function to provide state for each
    /// thread or iteration, do both before calling this method.
    pub fn prepare_iter_fn<F, IterState>(self, f: F) -> RunBuilderWithIterState<(), IterState>
    where
        F: Fn(&GroupInfo, &mut ()) -> IterState + Send + Sync + 'static,
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
    pub fn measure_wrapper_fns<FBegin, FEnd, MeasureWrapperState>(
        self,
        f_begin: FBegin,
        f_end: FEnd,
    ) -> RunBuilderWithWrapperState<(), (), MeasureWrapperState>
    where
        FBegin: Fn(&GroupInfo) -> MeasureWrapperState + Send + Sync + 'static,
        FEnd: Fn(MeasureWrapperState) + Send + Sync + 'static,
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
    pub fn iter_fn<F, CleanupState>(self, f: F) -> RunBuilderFinal<(), (), (), CleanupState>
    where
        F: Fn(()) -> CleanupState + Send + Sync + 'static,
    {
        RunBuilderFinal {
            groups: self.groups,
            prepare_thread_fn: Arc::new(|_| ()),
            prepare_iter_fn: Arc::new(|_, _| ()),
            measure_wrapper_begin_fn: Arc::new(|_| ()),
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
    /// of groups, via a [`GroupInfo`] parameter.
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
    pub fn prepare_iter_fn<F, IterState>(
        self,
        f: F,
    ) -> RunBuilderWithIterState<ThreadState, IterState>
    where
        F: Fn(&GroupInfo, &mut ThreadState) -> IterState + Send + Sync + 'static,
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
    pub fn measure_wrapper_fns<FBegin, FEnd, MeasureWrapperState>(
        self,
        f_begin: FBegin,
        f_end: FEnd,
    ) -> RunBuilderWithWrapperState<ThreadState, (), MeasureWrapperState>
    where
        FBegin: Fn(&GroupInfo) -> MeasureWrapperState + Send + Sync + 'static,
        FEnd: Fn(MeasureWrapperState) + Send + Sync + 'static,
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
    /// This must be the last step in preparing a benchmark run. After this,
    /// you can only call `build()` on the builder.
    ///
    /// If you wish to specify an iteration preparation function to provide state for each
    /// iteration, do it before calling this method.
    pub fn iter_fn<F, CleanupState>(
        self,
        f: F,
    ) -> RunBuilderFinal<ThreadState, (), (), CleanupState>
    where
        F: Fn(()) -> CleanupState + Send + Sync + 'static,
    {
        RunBuilderFinal {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: Arc::new(|_, _| ()),
            measure_wrapper_begin_fn: Arc::new(|_| ()),
            measure_wrapper_end_fn: Arc::new(|_| ()),
            iter_fn: Arc::new(f),
        }
    }
}

impl<ThreadState, IterState> RunBuilderWithIterState<ThreadState, IterState>
where
    ThreadState: 'static,
    IterState: 'static,
{
    /// Divides the threads used for benchmarking into `n` equal groups. Defaults to 1 group.
    ///
    /// The callbacks will be informed of which group they are executing for, and the total number
    /// of groups, via a [`GroupInfo`] parameter.
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
    pub fn measure_wrapper_fns<FBegin, FEnd, MeasureWrapperState>(
        self,
        f_begin: FBegin,
        f_end: FEnd,
    ) -> RunBuilderWithWrapperState<ThreadState, IterState, MeasureWrapperState>
    where
        FBegin: Fn(&GroupInfo) -> MeasureWrapperState + Send + Sync + 'static,
        FEnd: Fn(MeasureWrapperState) + Send + Sync + 'static,
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
    ) -> RunBuilderFinal<ThreadState, IterState, (), CleanupState>
    where
        F: Fn(IterState) -> CleanupState + Send + Sync + 'static,
    {
        RunBuilderFinal {
            groups: self.groups,
            prepare_thread_fn: self.prepare_thread_fn,
            prepare_iter_fn: self.prepare_iter_fn,
            measure_wrapper_begin_fn: Arc::new(|_| ()),
            measure_wrapper_end_fn: Arc::new(|_| ()),
            iter_fn: Arc::new(f),
        }
    }
}

impl<ThreadState, IterState, MeasureWrapperState>
    RunBuilderWithWrapperState<ThreadState, IterState, MeasureWrapperState>
where
    ThreadState: 'static,
    IterState: 'static,
    MeasureWrapperState: 'static,
{
    /// Divides the threads used for benchmarking into `n` equal groups. Defaults to 1 group.
    ///
    /// The callbacks will be informed of which group they are executing for, and the total number
    /// of groups, via a [`GroupInfo`] parameter.
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
    ) -> RunBuilderFinal<ThreadState, IterState, MeasureWrapperState, CleanupState>
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

impl<ThreadState, IterState, MeasureWrapperState, CleanupState>
    RunBuilderFinal<ThreadState, IterState, MeasureWrapperState, CleanupState>
where
    ThreadState: 'static,
    IterState: 'static,
    MeasureWrapperState: 'static,
    CleanupState: 'static,
{
    /// Completes the preparation of a benchmark run and returns a [`Run`] instance that can be
    /// used to execute the run on a specific thread pool.
    pub fn build(self) -> Run<ThreadState, IterState, MeasureWrapperState, CleanupState> {
        // If we need to, we can do some validation here in the future. Right now,
        // we have no need for this - everything is guarded via the type system.

        Run::<ThreadState, IterState, MeasureWrapperState, CleanupState>::new(self)
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
