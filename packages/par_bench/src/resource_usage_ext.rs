//! Extension trait for combined resource usage tracking in benchmark runs.
//!
//! This module provides an extension trait that adds combined allocation and processor time
//! tracking capabilities to the `Run` builder types when either the `alloc_tracker` or
//! `all_the_time` features are enabled.

/// Extension trait for combined resource usage tracking in benchmark runs.
///
/// This trait adds the `measure_resource_usage` method to `Run` builder types, providing
/// a convenient way to track multiple types of resource usage during benchmark execution.
/// The available measurement types depend on which features are enabled.
///
/// # Examples
///
/// ```
/// use all_the_time::Session as TimeSession;
/// # #[cfg(all(feature = "alloc_tracker", feature = "all_the_time"))]
/// # fn example() {
/// use alloc_tracker::{Allocator, Session as AllocSession};
/// use many_cpus::ProcessorSet;
/// use par_bench::{ResourceUsageExt, Run, ThreadPool};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// let allocs = AllocSession::new();
/// let processor_time = TimeSession::new();
/// let mut pool = ThreadPool::new(&ProcessorSet::default());
///
/// let run = Run::new()
///     .measure_resource_usage("my_operation", |measure| {
///         measure.allocs(&allocs).processor_time(&processor_time)
///     })
///     .iter(|_| {
///         let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
///
///         // Perform processor-intensive work
///         let mut sum = 0_u64;
///         for i in 0_u64..1000 {
///             sum = sum.wrapping_add(i.wrapping_mul(i));
///         }
///         std::hint::black_box(sum);
///     });
///
/// let results = run.execute_on(&mut pool, 1000);
///
/// // Access the combined resource usage data
/// for output in results.measure_outputs() {
///     println!("Resource usage data collected");
/// }
/// # }
/// # #[cfg(all(feature = "alloc_tracker", feature = "all_the_time"))]
/// # example();
/// ```
pub trait ResourceUsageExt<'a, ThreadState> {
    /// The type returned when resource usage tracking is configured.
    type Output;

    /// Configures resource usage tracking for the benchmark run.
    ///
    /// This method creates a measurement wrapper that tracks various types of resource usage
    /// based on the enabled features and the configuration provided in the callback.
    ///
    /// The callback receives a [`ResourceUsageMeasureBuilder`] that can be used to configure
    /// which types of measurements should be taken during the benchmark run.
    ///
    /// # Parameters
    ///
    /// * `operation_name` - The name to assign to this operation in all tracking results
    /// * `configure` - A callback that configures which resource measurements to track
    ///
    /// # Examples
    ///
    /// With allocation tracking only:
    /// ```
    /// # #[cfg(feature = "alloc_tracker")]
    /// # fn example() {
    /// use alloc_tracker::{Allocator, Session};
    /// use many_cpus::ProcessorSet;
    /// use par_bench::{ResourceUsageExt, Run, ThreadPool};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let allocs = Session::new();
    /// let mut pool = ThreadPool::new(&ProcessorSet::default());
    ///
    /// let run = Run::new()
    ///     .measure_resource_usage("vector_creation", |measure| measure.allocs(&allocs))
    ///     .iter(|_| {
    ///         let _data = vec![1, 2, 3, 4, 5];
    ///     });
    ///
    /// let results = run.execute_on(&mut pool, 100);
    /// # }
    /// # #[cfg(feature = "alloc_tracker")]
    /// # example();
    /// ```
    ///
    /// With processor time tracking only:
    /// ```
    /// # #[cfg(feature = "all_the_time")]
    /// # fn example() {
    /// use all_the_time::Session;
    /// use many_cpus::ProcessorSet;
    /// use par_bench::{ResourceUsageExt, Run, ThreadPool};
    ///
    /// let processor_time = Session::new();
    /// let mut pool = ThreadPool::new(&ProcessorSet::default());
    ///
    /// let run = Run::new()
    ///     .measure_resource_usage("cpu_work", |measure| {
    ///         measure.processor_time(&processor_time)
    ///     })
    ///     .iter(|_| {
    ///         let mut sum = 0_u64;
    ///         for i in 0_u64..1000 {
    ///             sum = sum.wrapping_add(i);
    ///         }
    ///         std::hint::black_box(sum);
    ///     });
    ///
    /// let results = run.execute_on(&mut pool, 100);
    /// # }
    /// # #[cfg(feature = "all_the_time")]
    /// # example();
    /// ```
    fn measure_resource_usage<F>(self, operation_name: &'a str, configure: F) -> Self::Output
    where
        F: FnOnce(ResourceUsageMeasureBuilder<'a>) -> ResourceUsageMeasureBuilder<'a>;
}

/// Builder for configuring resource usage measurements.
///
/// This builder allows you to configure which types of resource usage should be tracked
/// during benchmark execution. The available methods depend on which features are enabled.
#[derive(Clone, Debug)]
pub struct ResourceUsageMeasureBuilder<'a> {
    #[cfg(feature = "alloc_tracker")]
    alloc_session: Option<&'a alloc_tracker::Session>,

    #[cfg(feature = "all_the_time")]
    time_session: Option<&'a all_the_time::Session>,
}

impl<'a> ResourceUsageMeasureBuilder<'a> {
    /// Creates a new empty resource usage measure builder.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(feature = "alloc_tracker")]
            alloc_session: None,

            #[cfg(feature = "all_the_time")]
            time_session: None,
        }
    }

    /// Configures allocation tracking for the benchmark run.
    ///
    /// This method is only available when the `alloc_tracker` feature is enabled.
    ///
    /// # Parameters
    ///
    /// * `session` - The allocation tracking session to use
    #[cfg(feature = "alloc_tracker")]
    #[must_use]
    pub fn allocs(mut self, session: &'a alloc_tracker::Session) -> Self {
        self.alloc_session = Some(session);
        self
    }

    /// Configures processor time tracking for the benchmark run.
    ///
    /// This method is only available when the `all_the_time` feature is enabled.
    ///
    /// # Parameters
    ///
    /// * `session` - The processor time tracking session to use
    #[cfg(feature = "all_the_time")]
    #[must_use]
    pub fn processor_time(mut self, session: &'a all_the_time::Session) -> Self {
        self.time_session = Some(session);
        self
    }
}

/// Combined resource usage measurement output.
///
/// This struct contains the measurement results from all configured resource usage tracking.
/// The available methods depend on which features are enabled and which measurements were
/// configured during the benchmark run.
#[derive(Debug)]
pub struct ResourceUsageOutput {
    #[cfg(feature = "alloc_tracker")]
    alloc_report: Option<alloc_tracker::Report>,

    #[cfg(feature = "all_the_time")]
    time_report: Option<all_the_time::Report>,
}

impl ResourceUsageOutput {
    /// Creates a new resource usage output with the specified reports.
    #[must_use]
    pub(crate) fn new(
        #[cfg(feature = "alloc_tracker")] alloc_report: Option<alloc_tracker::Report>,
        #[cfg(feature = "all_the_time")] time_report: Option<all_the_time::Report>,
    ) -> Self {
        Self {
            #[cfg(feature = "alloc_tracker")]
            alloc_report,
            #[cfg(feature = "all_the_time")]
            time_report,
        }
    }

    /// Returns the allocation tracking report, if allocation tracking was configured.
    ///
    /// This method is only available when the `alloc_tracker` feature is enabled.
    /// Returns `None` if allocation tracking was not configured for this benchmark run.
    #[cfg(feature = "alloc_tracker")]
    #[must_use]
    pub fn allocs(&self) -> Option<&alloc_tracker::Report> {
        self.alloc_report.as_ref()
    }

    /// Returns the processor time tracking report, if processor time tracking was configured.
    ///
    /// This method is only available when the `all_the_time` feature is enabled.
    /// Returns `None` if processor time tracking was not configured for this benchmark run.
    #[cfg(feature = "all_the_time")]
    #[must_use]
    pub fn processor_time(&self) -> Option<&all_the_time::Report> {
        self.time_report.as_ref()
    }
}

/// Internal state for managing resource usage spans during benchmark execution.
#[derive(Debug)]
pub struct ResourceUsageState {
    #[cfg(feature = "alloc_tracker")]
    alloc_span: Option<alloc_tracker::ThreadSpan>,

    #[cfg(feature = "all_the_time")]
    time_span: Option<all_the_time::ThreadSpan>,
}

impl ResourceUsageState {
    /// Creates a new resource usage state from the builder configuration.
    #[must_use]
    pub(crate) fn new(
        builder: &ResourceUsageMeasureBuilder<'_>,
        operation_name: &str,
        iterations: u64,
    ) -> Self {
        Self {
            #[cfg(feature = "alloc_tracker")]
            alloc_span: builder.alloc_session.map(|session| {
                session
                    .operation(operation_name)
                    .measure_thread()
                    .iterations(iterations)
            }),

            #[cfg(feature = "all_the_time")]
            time_span: builder.time_session.map(|session| {
                session
                    .operation(operation_name)
                    .measure_thread()
                    .iterations(iterations)
            }),
        }
    }

    /// Converts the state into the final resource usage output.
    #[must_use]
    pub(crate) fn into_output(
        self,
        builder: &ResourceUsageMeasureBuilder<'_>,
    ) -> ResourceUsageOutput {
        // Drop the spans to record the measurements
        #[cfg(feature = "alloc_tracker")]
        let alloc_report = self.alloc_span.and_then(|span| {
            drop(span);
            builder.alloc_session.map(alloc_tracker::Session::to_report)
        });

        #[cfg(feature = "all_the_time")]
        let time_report = self.time_span.and_then(|span| {
            drop(span);
            builder.time_session.map(all_the_time::Session::to_report)
        });

        ResourceUsageOutput::new(
            #[cfg(feature = "alloc_tracker")]
            alloc_report,
            #[cfg(feature = "all_the_time")]
            time_report,
        )
    }
}

/// Creates a resource usage state factory function for the given builder and operation name.
///
/// This function handles the calculation of per-thread iterations to avoid double-counting
/// in parallel benchmark scenarios.
fn create_resource_usage_state_factory<'a, ThreadState>(
    builder: ResourceUsageMeasureBuilder<'a>,
    operation_name: &'a str,
) -> impl Fn(crate::args::MeasureWrapperBegin<'_, ThreadState>) -> ResourceUsageState + 'a {
    move |args| {
        // NB! As we are working with parallel benchmarking, we need to ensure we count
        // each GLOBAL iteration for comparable results. The measurement wrapper,
        // however is LOCAL to each thread. We would multi-count iterations if we just
        // used this as-is (with 4 threads, we would count 4x iterations).
        //
        // We fixup this with a simple division to offset it back again.
        // Ensure we never get 0 iterations by using max(1, division_result).
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "NonZero eliminates division by zero"
        )]
        #[expect(
            clippy::integer_division,
            reason = "we accept imperfect accuracy - typical iteration counts are high enough for it not to matter"
        )]
        let iterations =
            (args.meta().iterations() / args.meta().thread_count().get() as u64).max(1);

        ResourceUsageState::new(&builder, operation_name, iterations)
    }
}

impl<'a> ResourceUsageExt<'a, ()> for crate::configure::RunInitial {
    type Output =
        crate::configure::RunWithWrapperState<'a, (), (), ResourceUsageState, ResourceUsageOutput>;

    fn measure_resource_usage<F>(self, operation_name: &'a str, configure: F) -> Self::Output
    where
        F: FnOnce(ResourceUsageMeasureBuilder<'a>) -> ResourceUsageMeasureBuilder<'a>,
    {
        let builder = configure(ResourceUsageMeasureBuilder::new());

        self.measure_wrapper(
            create_resource_usage_state_factory(builder.clone(), operation_name),
            move |state| state.into_output(&builder),
        )
    }
}

impl<'a, ThreadState> ResourceUsageExt<'a, ThreadState>
    for crate::configure::RunWithThreadState<'a, ThreadState>
{
    type Output = crate::configure::RunWithWrapperState<
        'a,
        ThreadState,
        (),
        ResourceUsageState,
        ResourceUsageOutput,
    >;

    fn measure_resource_usage<F>(self, operation_name: &'a str, configure: F) -> Self::Output
    where
        F: FnOnce(ResourceUsageMeasureBuilder<'a>) -> ResourceUsageMeasureBuilder<'a>,
    {
        let builder = configure(ResourceUsageMeasureBuilder::new());

        self.measure_wrapper(
            create_resource_usage_state_factory(builder.clone(), operation_name),
            move |state| state.into_output(&builder),
        )
    }
}

impl<'a, ThreadState, IterState> ResourceUsageExt<'a, ThreadState>
    for crate::configure::RunWithIterState<'a, ThreadState, IterState>
{
    type Output = crate::configure::RunWithWrapperState<
        'a,
        ThreadState,
        IterState,
        ResourceUsageState,
        ResourceUsageOutput,
    >;

    fn measure_resource_usage<F>(self, operation_name: &'a str, configure: F) -> Self::Output
    where
        F: FnOnce(ResourceUsageMeasureBuilder<'a>) -> ResourceUsageMeasureBuilder<'a>,
    {
        let builder = configure(ResourceUsageMeasureBuilder::new());

        self.measure_wrapper(
            create_resource_usage_state_factory(builder.clone(), operation_name),
            move |state| state.into_output(&builder),
        )
    }
}

#[cfg(test)]
mod tests {
    use many_cpus::ProcessorSet;

    use super::ResourceUsageExt;
    use crate::{Run, ThreadPool};

    #[test]
    fn module_loads() {
        // Basic test to ensure the module compiles and loads correctly under Miri.
        // This test does not require any OS functionality.
        let builder = super::ResourceUsageMeasureBuilder::new();
        // Just verify the builder can be created
        std::hint::black_box(builder);
    }

    #[test]
    #[cfg(all(not(miri), feature = "alloc_tracker"))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn measure_resource_usage_allocs_only() {
        let allocs = alloc_tracker::Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::single());

        let results = Run::new()
            .measure_resource_usage("test_operation", |measure| measure.allocs(&allocs))
            .iter(|_| {
                // Allocate some memory to generate allocation activity
                let _data = [1, 2, 3, 4, 5].to_vec();
            })
            .execute_on(&mut pool, 10);

        // Verify that we got results back
        assert!(results.measure_outputs().count() > 0);

        // Verify that allocation tracking worked
        for output in results.measure_outputs() {
            assert!(output.allocs().is_some());
        }

        // Verify that the session recorded the operation
        let report = allocs.to_report();
        assert!(!report.is_empty());
    }

    #[test]
    #[cfg(all(not(miri), feature = "all_the_time"))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn measure_resource_usage_processor_time_only() {
        let processor_time = all_the_time::Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::single());

        let results = Run::new()
            .measure_resource_usage("test_operation", |measure| {
                measure.processor_time(&processor_time)
            })
            .iter(|_| {
                // Perform some CPU-intensive work
                let mut sum = 0_u64;
                for i in 0_u64..1000 {
                    sum = sum.wrapping_add(i.wrapping_mul(i));
                }
                std::hint::black_box(sum);
            })
            .execute_on(&mut pool, 10);

        // Verify that we got results back
        assert!(results.measure_outputs().count() > 0);

        // Verify that processor time tracking worked
        for output in results.measure_outputs() {
            assert!(output.processor_time().is_some());
        }

        // Verify that the session recorded the operation
        let report = processor_time.to_report();
        assert!(!report.is_empty());
    }

    #[test]
    #[cfg(all(not(miri), feature = "alloc_tracker", feature = "all_the_time"))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn measure_resource_usage_combined() {
        let allocs = alloc_tracker::Session::new();
        let processor_time = all_the_time::Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::single());

        let results = Run::new()
            .measure_resource_usage("test_operation", |measure| {
                measure.allocs(&allocs).processor_time(&processor_time)
            })
            .iter(|_| {
                // Allocate memory and perform CPU work
                let _data = [1, 2, 3, 4, 5].to_vec();
                let mut sum = 0_u64;
                for i in 0_u64..100 {
                    sum = sum.wrapping_add(i);
                }
                std::hint::black_box(sum);
            })
            .execute_on(&mut pool, 5);

        // Verify that we got results back
        assert!(results.measure_outputs().count() > 0);

        // Verify that both types of tracking worked
        for output in results.measure_outputs() {
            assert!(output.allocs().is_some());
            assert!(output.processor_time().is_some());
        }

        // Verify that both sessions recorded the operations
        let alloc_report = allocs.to_report();
        assert!(!alloc_report.is_empty());

        let time_report = processor_time.to_report();
        assert!(!time_report.is_empty());
    }

    #[test]
    #[cfg(all(not(miri), feature = "alloc_tracker", feature = "all_the_time"))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn api_supports_groups_in_any_order() {
        let allocs = alloc_tracker::Session::new();
        let processor_time = all_the_time::Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::default());

        // Test 1: .measure_resource_usage() before .groups()
        let results1 = Run::new()
            .prepare_iter(|_| 42_i32)
            .measure_resource_usage("test1", |measure| {
                measure.allocs(&allocs).processor_time(&processor_time)
            })
            .groups(new_zealand::nz!(2))
            .iter(|_| {
                let _data = [1, 2, 3, 4, 5].to_vec();
            })
            .execute_on(&mut pool, 10);

        assert!(results1.measure_outputs().count() > 0);

        // Test 2: .groups() before .measure_resource_usage()
        let results2 = Run::new()
            .prepare_iter(|_| 42_i32)
            .groups(new_zealand::nz!(2))
            .measure_resource_usage("test2", |measure| {
                measure.allocs(&allocs).processor_time(&processor_time)
            })
            .iter(|_| {
                let _data = [1, 2, 3, 4, 5].to_vec();
            })
            .execute_on(&mut pool, 10);

        assert!(results2.measure_outputs().count() > 0);
    }

    #[test]
    fn api_supports_groups_immediately_after_new() {
        // Test that Run::new().groups() works
        let _run = Run::new().groups(new_zealand::nz!(2));

        // Test the full chain with groups first
        let _run = Run::new().groups(new_zealand::nz!(2)).iter(|_| {
            std::hint::black_box(42);
        });
    }

    #[test]
    #[cfg(all(not(miri), feature = "alloc_tracker"))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn api_supports_measure_resource_usage_after_groups_only() {
        let allocs = alloc_tracker::Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::default());

        // Test pattern: Run::new().groups().measure_resource_usage()
        let results = Run::new()
            .groups(new_zealand::nz!(2))
            .measure_resource_usage("test", |measure| measure.allocs(&allocs))
            .iter(|_| {
                let _data = [1, 2, 3, 4, 5].to_vec();
            })
            .execute_on(&mut pool, 10);

        assert!(results.measure_outputs().count() > 0);
    }

    #[test]
    #[cfg(all(not(miri), feature = "alloc_tracker"))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn api_supports_original_pattern_groups_prepare_iter_measure() {
        let allocs = alloc_tracker::Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::default());

        // Test original pattern: Run::new().groups().prepare_iter().measure_resource_usage()
        let results = Run::new()
            .groups(new_zealand::nz!(2))
            .prepare_iter(|_| 42_i32)
            .measure_resource_usage("test", |measure| measure.allocs(&allocs))
            .iter(|_| {
                let _data = [1, 2, 3, 4, 5].to_vec();
            })
            .execute_on(&mut pool, 10);

        assert!(results.measure_outputs().count() > 0);
    }

    #[test]
    #[cfg(all(not(miri), feature = "alloc_tracker"))] // Uses ThreadPool which requires OS threading functions that Miri cannot emulate.
    fn measure_resource_usage_with_thread_state() {
        let allocs = alloc_tracker::Session::new();
        let mut pool = ThreadPool::new(ProcessorSet::single());

        let results = Run::new()
            .prepare_thread(|_| String::from("thread_data"))
            .measure_resource_usage("test_with_state", |measure| measure.allocs(&allocs))
            .iter(|args| {
                // Use thread state and allocate memory
                let _combined = format!("{}_allocated", args.thread_state());
            })
            .execute_on(&mut pool, 5);

        // Verify that we got results back
        assert!(results.measure_outputs().count() > 0);

        // Verify that allocation tracking worked
        for output in results.measure_outputs() {
            assert!(output.allocs().is_some());
        }

        // Verify that the session recorded the operation
        let report = allocs.to_report();
        assert!(!report.is_empty());
    }
}
