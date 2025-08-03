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
    #[cfg_attr(test, mutants::skip)] // Manually tested due to Criterion dependency.
    pub fn execute_criterion_on(
        &self,
        pool: &mut ThreadPool,
        group: &mut BenchmarkGroup<'_, WallTime>,
        name: &str,
    ) -> Vec<MeasureOutput> {
        let mut measure_outputs = Vec::new();

        group.bench_function(name, |b| {
            b.iter_custom(|iters| {
                let mut result = self.execute_on(pool, iters);

                measure_outputs.extend(result.take_measure_outputs());

                result.mean_duration()
            });
        });

        measure_outputs
    }
}
