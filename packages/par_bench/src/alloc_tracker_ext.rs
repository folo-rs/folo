//! Extension trait for allocation tracking in benchmark runs.
//!
//! This module provides an extension trait that adds allocation tracking capabilities
//! to the `Run` builder types when the `alloc_tracker` feature is enabled.

/// Extension trait for allocation tracking in benchmark runs.
///
/// This trait adds the `measure_allocs` method to `Run` builder types, providing
/// a convenient way to track memory allocations during benchmark execution.
///
/// # Examples
///
/// ```
/// use alloc_tracker::{Allocator, Session};
/// use many_cpus::ProcessorSet;
/// use par_bench::{AllocTrackerExt, Run, ThreadPool};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// # fn main() {
/// let allocs = Session::new();
/// let mut pool = ThreadPool::new(&ProcessorSet::default());
///
/// let run = Run::new()
///     .measure_allocs(&allocs, "my_operation")
///     .iter(|_| {
///         let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
///     });
///
/// let results = run.execute_on(&mut pool, 1000);
/// allocs.print_to_stdout();
/// # }
/// ```
pub trait AllocTrackerExt<'a, ThreadState> {
    /// The type returned when allocation tracking is configured.
    type Output;

    /// Configures allocation tracking for the benchmark run.
    ///
    /// This method creates a measurement wrapper that tracks memory allocations
    /// using the provided `alloc_tracker` session and operation name.
    ///
    /// The measurement wrapper will:
    /// 1. Start tracking allocations before the benchmark iterations begin
    /// 2. Stop tracking and record the results after all iterations complete
    /// 3. Return a `Report` containing the allocation data for each thread
    ///
    /// # Parameters
    ///
    /// * `session` - The allocation tracking session to use
    /// * `operation_name` - The name to assign to this operation in the tracking results
    ///
    /// # Examples
    ///
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    /// use many_cpus::ProcessorSet;
    /// use par_bench::{AllocTrackerExt, Run, ThreadPool};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// # fn main() {
    /// let allocs = Session::new();
    /// let mut pool = ThreadPool::new(&ProcessorSet::default());
    ///
    /// let run = Run::new()
    ///     .measure_allocs(&allocs, "vector_creation")
    ///     .iter(|_| {
    ///         let _data = vec![1, 2, 3, 4, 5];
    ///     });
    ///
    /// let results = run.execute_on(&mut pool, 100);
    ///
    /// // Access allocation reports from results
    /// for report in results.measure_outputs() {
    ///     println!("Thread allocation report available");
    /// }
    ///
    /// // Print overall session statistics
    /// allocs.print_to_stdout();
    /// # }
    /// ```
    fn measure_allocs(
        self,
        session: &'a alloc_tracker::Session,
        operation_name: &'a str,
    ) -> Self::Output;
}

impl<'a> AllocTrackerExt<'a, ()> for crate::configure::RunInitial {
    type Output = crate::configure::RunWithWrapperState<
        'a,
        (),
        (),
        alloc_tracker::ThreadSpan,
        alloc_tracker::Report,
    >;

    fn measure_allocs(
        self,
        session: &'a alloc_tracker::Session,
        operation_name: &'a str,
    ) -> Self::Output {
        self.measure_wrapper(
            move |args| {
                session
                    .operation(operation_name)
                    .measure_thread()
                    .iterations(args.meta().iterations())
            },
            |span| {
                // Drop the span to record the measurements, then get the report
                drop(span);
                session.to_report()
            },
        )
    }
}

impl<'a, ThreadState> AllocTrackerExt<'a, ThreadState>
    for crate::configure::RunWithThreadState<'a, ThreadState>
{
    type Output = crate::configure::RunWithWrapperState<
        'a,
        ThreadState,
        (),
        alloc_tracker::ThreadSpan,
        alloc_tracker::Report,
    >;

    fn measure_allocs(
        self,
        session: &'a alloc_tracker::Session,
        operation_name: &'a str,
    ) -> Self::Output {
        self.measure_wrapper(
            move |args| {
                session
                    .operation(operation_name)
                    .measure_thread()
                    .iterations(args.meta().iterations())
            },
            |span| {
                // Drop the span to record the measurements, then get the report
                drop(span);
                session.to_report()
            },
        )
    }
}

impl<'a, ThreadState, IterState> AllocTrackerExt<'a, ThreadState>
    for crate::configure::RunWithIterState<'a, ThreadState, IterState>
{
    type Output = crate::configure::RunWithWrapperState<
        'a,
        ThreadState,
        IterState,
        alloc_tracker::ThreadSpan,
        alloc_tracker::Report,
    >;

    fn measure_allocs(
        self,
        session: &'a alloc_tracker::Session,
        operation_name: &'a str,
    ) -> Self::Output {
        self.measure_wrapper(
            move |args| {
                session
                    .operation(operation_name)
                    .measure_thread()
                    .iterations(args.meta().iterations())
            },
            |span| {
                // Drop the span to record the measurements, then get the report
                drop(span);
                session.to_report()
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use alloc_tracker::Session;
    use many_cpus::ProcessorSet;

    use super::AllocTrackerExt;
    use crate::{Run, ThreadPool};

    #[test]
    fn module_loads() {
        // Basic test to ensure the module compiles and loads correctly under Miri.
        // This test doesn't require any OS functionality.
        let session = Session::new();
        // New sessions should be empty initially.
        assert!(session.is_empty());
    }

    #[test]
    #[cfg(not(miri))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn measure_allocs_basic_usage() {
        let allocs = Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::single());

        let results = Run::new()
            .measure_allocs(&allocs, "test_operation")
            .iter(|_| {
                // Allocate some memory to generate allocation activity
                let _data = [1, 2, 3, 4, 5].to_vec();
            })
            .execute_on(&mut pool, 10);

        // Verify that we got results back
        assert!(results.measure_outputs().count() > 0);

        // Verify that the session recorded the operation
        let report = allocs.to_report();
        assert!(!report.is_empty());
    }

    #[test]
    #[cfg(not(miri))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn measure_allocs_with_thread_state() {
        let allocs = Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::single());

        let results = Run::new()
            .prepare_thread(|_| String::from("thread_data"))
            .measure_allocs(&allocs, "test_with_state")
            .iter(|args| {
                // Use thread state and allocate memory
                let _combined = format!("{}_allocated", args.thread_state());
            })
            .execute_on(&mut pool, 5);

        // Verify that we got results back
        assert!(results.measure_outputs().count() > 0);

        // Verify that the session recorded the operation
        let report = allocs.to_report();
        assert!(!report.is_empty());
    }
}
