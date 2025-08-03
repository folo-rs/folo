//! Extension trait for processor time tracking in benchmark runs.
//!
//! This module provides an extension trait that adds processor time tracking capabilities
//! to the `Run` builder types when the `all_the_time` feature is enabled.

/// Extension trait for processor time tracking in benchmark runs.
///
/// This trait adds the `measure_processor_time` method to `Run` builder types, providing
/// a convenient way to track processor time during benchmark execution.
///
/// # Examples
///
/// ```
/// use all_the_time::Session;
/// use many_cpus::ProcessorSet;
/// use par_bench::{AllTheTimeExt, Run, ThreadPool};
///
/// # fn main() {
/// let processor_time = Session::new();
/// let mut pool = ThreadPool::new(&ProcessorSet::default());
///
/// let run = Run::new()
///     .measure_processor_time(&processor_time, "my_operation")
///     .iter(|_| {
///         // Perform processor-intensive work
///         let mut sum = 0;
///         for i in 0..1000 {
///             sum += i * i;
///         }
///         std::hint::black_box(sum);
///     });
///
/// let results = run.execute_on(&mut pool, 1000);
/// processor_time.print_to_stdout();
/// # }
/// ```
pub trait AllTheTimeExt<'a, ThreadState> {
    /// The type returned when processor time tracking is configured.
    type Output;

    /// Configures processor time tracking for the benchmark run.
    ///
    /// This method creates a measurement wrapper that tracks processor time
    /// using the provided `all_the_time` session and operation name.
    ///
    /// The measurement wrapper will:
    /// 1. Start tracking processor time before the benchmark iterations begin
    /// 2. Stop tracking and record the results after all iterations complete
    /// 3. Return a `Report` containing the processor time data for each thread
    ///
    /// # Parameters
    ///
    /// * `session` - The processor time tracking session to use
    /// * `operation_name` - The name to assign to this operation in the tracking results
    ///
    /// # Examples
    ///
    /// ```
    /// use all_the_time::Session;
    /// use many_cpus::ProcessorSet;
    /// use par_bench::{AllTheTimeExt, Run, ThreadPool};
    ///
    /// # fn main() {
    /// let processor_time = Session::new();
    /// let mut pool = ThreadPool::new(&ProcessorSet::default());
    ///
    /// let run = Run::new()
    ///     .measure_processor_time(&processor_time, "cpu_intensive_work")
    ///     .iter(|_| {
    ///         // Perform CPU-intensive calculations
    ///         let mut result = 1_u64;
    ///         for i in 1_u64..=100 {
    ///             result = result.wrapping_mul(i % 1000);
    ///         }
    ///         std::hint::black_box(result);
    ///     });
    ///
    /// let results = run.execute_on(&mut pool, 100);
    ///
    /// // Access processor time reports from results
    /// for report in results.measure_outputs() {
    ///     println!("Thread processor time report available");
    /// }
    ///
    /// // Print overall session statistics
    /// processor_time.print_to_stdout();
    /// # }
    /// ```
    fn measure_processor_time(
        self,
        session: &'a all_the_time::Session,
        operation_name: &'a str,
    ) -> Self::Output;
}

impl<'a> AllTheTimeExt<'a, ()> for crate::configure::RunInitial {
    type Output = crate::configure::RunWithWrapperState<
        'a,
        (),
        (),
        all_the_time::ThreadSpan,
        all_the_time::Report,
    >;

    fn measure_processor_time(
        self,
        session: &'a all_the_time::Session,
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

impl<'a, ThreadState> AllTheTimeExt<'a, ThreadState>
    for crate::configure::RunWithThreadState<'a, ThreadState>
{
    type Output = crate::configure::RunWithWrapperState<
        'a,
        ThreadState,
        (),
        all_the_time::ThreadSpan,
        all_the_time::Report,
    >;

    fn measure_processor_time(
        self,
        session: &'a all_the_time::Session,
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

impl<'a, ThreadState, IterState> AllTheTimeExt<'a, ThreadState>
    for crate::configure::RunWithIterState<'a, ThreadState, IterState>
{
    type Output = crate::configure::RunWithWrapperState<
        'a,
        ThreadState,
        IterState,
        all_the_time::ThreadSpan,
        all_the_time::Report,
    >;

    fn measure_processor_time(
        self,
        session: &'a all_the_time::Session,
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
    use all_the_time::Session;
    use many_cpus::ProcessorSet;

    use super::AllTheTimeExt;
    use crate::{Run, ThreadPool};

    #[test]
    fn module_loads() {
        // Basic test to ensure the module compiles and loads correctly under Miri.
        // This test does not require any OS functionality.
        let session = Session::new();
        // New sessions should be empty initially.
        assert!(session.is_empty());
    }

    #[test]
    #[cfg(not(miri))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn measure_processor_time_basic_usage() {
        let processor_time = Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::single());

        let results = Run::new()
            .measure_processor_time(&processor_time, "test_operation")
            .iter(|_| {
                // Perform some CPU-intensive work to generate processor time activity
                let mut sum = 0;
                for i in 0..1000 {
                    sum += i * i;
                }
                std::hint::black_box(sum);
            })
            .execute_on(&mut pool, 10);

        // Verify that we got results back
        assert!(results.measure_outputs().count() > 0);

        // Verify that the session recorded the operation
        let report = processor_time.to_report();
        assert!(!report.is_empty());
    }

    #[test]
    #[cfg(not(miri))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn measure_processor_time_with_thread_state() {
        let processor_time = Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::single());

        let results = Run::new()
            .prepare_thread(|_| String::from("thread_data"))
            .measure_processor_time(&processor_time, "test_with_state")
            .iter(|args| {
                // Use thread state and perform CPU work
                let thread_data = args.thread_state();
                let mut result = thread_data.len();
                for i in 0..100 {
                    result = result.wrapping_mul(i).wrapping_add(thread_data.len());
                }
                std::hint::black_box(result);
            })
            .execute_on(&mut pool, 5);

        // Verify that we got results back
        assert!(results.measure_outputs().count() > 0);

        // Verify that the session recorded the operation
        let report = processor_time.to_report();
        assert!(!report.is_empty());
    }
}
