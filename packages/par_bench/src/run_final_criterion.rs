#![cfg(any(test, feature = "criterion"))]

use criterion::BenchmarkGroup;
use criterion::measurement::WallTime;

use crate::{ConfiguredRun, ThreadPool};

impl<ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>
    ConfiguredRun<'_, ThreadState, IterState, MeasureWrapperState, MeasureOutput, CleanupState>
where
    MeasureOutput: Send + 'static,
{
    /// Executes the benchmark run one or more times as part of a Criterion benchmark group,
    /// recording the result with Criterion.
    ///
    /// The number of iterations is determined by Criterion and the run is repeated as many times
    /// as Criterion decides is appropriate to collect the desired data.
    ///
    /// # Panics
    ///
    /// Panics if the thread pool's processor count is not divisible by the number of groups
    /// the run is configured for.
    pub fn execute_criterion_on(
        &self,
        pool: &ThreadPool,
        group: &mut BenchmarkGroup<'_, WallTime>,
        name: &str,
    ) {
        group.bench_function(name, |b| {
            b.iter_custom(move |iters| self.execute_on(pool, iters).mean_duration());
        });
    }
}
