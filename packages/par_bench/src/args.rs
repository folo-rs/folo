//! Argument wrappers passed to closures used in a `Run`.
//!
//! You generally do not need to reference these types directly.

use crate::RunMeta;

/// Arguments provided to the `prepare_thread_fn` closure of a `Run`.
#[derive(Debug)]
pub struct PrepareThread<'a> {
    meta: &'a RunMeta,
}

impl<'a> PrepareThread<'a> {
    pub(crate) fn new(meta: &'a RunMeta) -> Self {
        Self { meta }
    }

    /// Returns the `RunMeta` for the current run.
    #[must_use]
    pub fn meta(&self) -> &'a RunMeta {
        self.meta
    }
}

/// Arguments provided to the `prepare_iter_fn` closure of a `Run`.
#[derive(Debug)]
pub struct PrepareIter<'a, ThreadState> {
    meta: &'a RunMeta,
    thread_state: &'a ThreadState,
}

impl<'a, ThreadState> PrepareIter<'a, ThreadState> {
    pub(crate) fn new(meta: &'a RunMeta, thread_state: &'a ThreadState) -> Self {
        Self { meta, thread_state }
    }

    /// Returns the `RunMeta` for the current run.
    #[must_use]
    pub fn meta(&self) -> &'a RunMeta {
        self.meta
    }

    /// Returns the thread state for the current thread.
    #[must_use]
    pub fn thread_state(&self) -> &'a ThreadState {
        self.thread_state
    }
}

/// Arguments provided to the `measure_wrapper_begin_fn` closure of a `Run`.
#[derive(Debug)]
pub struct MeasureWrapperBegin<'a, ThreadState> {
    meta: &'a RunMeta,
    thread_state: &'a ThreadState,
}

impl<'a, ThreadState> MeasureWrapperBegin<'a, ThreadState> {
    pub(crate) fn new(meta: &'a RunMeta, thread_state: &'a ThreadState) -> Self {
        Self { meta, thread_state }
    }

    /// Returns the `RunMeta` for the current run.
    #[must_use]
    pub fn meta(&self) -> &'a RunMeta {
        self.meta
    }

    /// Returns the thread state for the current thread.
    #[must_use]
    pub fn thread_state(&self) -> &'a ThreadState {
        self.thread_state
    }
}

/// Arguments provided to the `iter_fn` closure of a `Run`.
#[derive(Debug)]
pub struct Iter<'a, ThreadState, IterState> {
    meta: &'a RunMeta,
    thread_state: &'a ThreadState,
    #[expect(clippy::struct_field_names, reason = "this is fine")]
    iter_state: Option<IterState>,
}

impl<'a, ThreadState, IterState> Iter<'a, ThreadState, IterState> {
    pub(crate) fn new(
        meta: &'a RunMeta,
        thread_state: &'a ThreadState,
        iter_state: IterState,
    ) -> Self {
        Self {
            meta,
            thread_state,
            iter_state: Some(iter_state),
        }
    }

    /// Returns the `RunMeta` for the current run.
    #[must_use]
    pub fn meta(&self) -> &'a RunMeta {
        self.meta
    }

    /// Returns the thread state for the current thread.
    #[must_use]
    pub fn thread_state(&self) -> &'a ThreadState {
        self.thread_state
    }

    /// Returns the iteration state for the current iteration, consuming it.
    /// 
    /// # Panics
    /// 
    /// Panics if the iteration state has already been consumed.
    #[must_use]
    pub fn take_iter_state(&mut self) -> IterState {
        self.iter_state
            .take()
            .expect("can only take iter_state once")
    }

    /// Returns a reference to the iteration state for the current iteration.
    /// 
    /// # Panics
    /// 
    /// Panics if the iteration state has already been consumed.
    #[must_use]
    pub fn iter_state(&self) -> &IterState {
        self.iter_state
            .as_ref()
            .expect("iter_state already consumed")
    }
}
