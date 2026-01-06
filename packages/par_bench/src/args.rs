//! Argument wrappers passed to closures used in a `Run`.
//!
//! You generally do not need to reference these types directly.

use crate::RunMeta;

/// Arguments provided to the `prepare_thread()` closure of a `Run`.
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
    #[inline]
    pub fn meta(&self) -> &'a RunMeta {
        self.meta
    }
}

/// Arguments provided to the `prepare_iter()` closure of a `Run`.
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
    #[inline]
    pub fn meta(&self) -> &'a RunMeta {
        self.meta
    }

    /// Returns the thread state for the current thread.
    #[must_use]
    #[inline]
    pub fn thread_state(&self) -> &'a ThreadState {
        self.thread_state
    }
}

/// Arguments provided to the `measure_wrapper_begin()` closure of a `Run`.
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
    #[inline]
    pub fn meta(&self) -> &'a RunMeta {
        self.meta
    }

    /// Returns the thread state for the current thread.
    #[must_use]
    #[inline]
    pub fn thread_state(&self) -> &'a ThreadState {
        self.thread_state
    }
}

/// Arguments provided to the `iter()` closure of a `Run`.
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
    #[inline]
    pub fn meta(&self) -> &'a RunMeta {
        self.meta
    }

    /// Returns the thread state for the current thread.
    #[must_use]
    #[inline]
    pub fn thread_state(&self) -> &'a ThreadState {
        self.thread_state
    }

    /// Returns the iteration state for the current iteration, consuming it.
    ///
    /// # Panics
    ///
    /// Panics if the iteration state has already been consumed.
    #[must_use]
    #[inline]
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
    #[inline]
    pub fn iter_state(&self) -> &IterState {
        self.iter_state
            .as_ref()
            .expect("iter_state already consumed")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::{Arc, Mutex};

    use many_cpus::SystemHardware;
    use new_zealand::nz;

    use crate::{Run, ThreadPool};

    #[test]
    #[cfg_attr(miri, ignore)]
    fn prepare_iter_meta_returns_correct_run_meta() {
        let mut pool = ThreadPool::new(
            SystemHardware::current()
                .processors()
                .to_builder()
                .take(nz!(1))
                .unwrap(),
        );
        let observed_meta = Arc::new(Mutex::new(None));

        let _result = Run::new()
            .prepare_iter({
                let observed_meta = Arc::clone(&observed_meta);
                move |args| {
                    *observed_meta.lock().unwrap() = Some(*args.meta());
                }
            })
            .iter(|_| ())
            .execute_on(&mut pool, 42);

        let meta = observed_meta.lock().unwrap().unwrap();
        assert_eq!(meta.iterations(), 42);
        assert_eq!(meta.group_count().get(), 1);
        assert_eq!(meta.group_index(), 0);
        assert_eq!(meta.thread_count().get(), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn measure_wrapper_begin_thread_state_returns_correct_state() {
        let mut pool = ThreadPool::new(
            SystemHardware::current()
                .processors()
                .to_builder()
                .take(nz!(1))
                .unwrap(),
        );
        let observed_thread_state = Arc::new(Mutex::new(None));

        let _result = Run::new()
            .prepare_thread(|_| "my_thread_state".to_string())
            .measure_wrapper(
                {
                    let observed = Arc::clone(&observed_thread_state);
                    move |args| {
                        *observed.lock().unwrap() = Some(args.thread_state().clone());
                    }
                },
                |()| (),
            )
            .iter(|_| ())
            .execute_on(&mut pool, 1);

        let thread_state = observed_thread_state.lock().unwrap().clone().unwrap();
        assert_eq!(thread_state, "my_thread_state");
    }
}
