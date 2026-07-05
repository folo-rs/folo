//! The single, fully-computed processor time report.
//!
//! A [`Report`](crate::Report) is a mergeable snapshot of the streaming
//! statistics; it is not yet resolved into the per-iteration figures that the
//! outputs present. [`Report::finalize`](crate::Report::finalize) resolves it
//! exactly once into a [`FinalizedReport`]: every operation's complete
//! statistics — the warmup-robust slope *and* its confidence interval — are
//! computed up front. Both the human-readable stdout summary and the
//! machine-readable JSON files are then rendered from this one finalized value,
//! so there is no cheap "slope-only" path that silently skips the interval
//! computation and hides its cost from benchmarks.

use std::fmt;
use std::time::Duration;

use crate::OperationStatistics;
use crate::statistics::nanos_to_duration;

/// A processor time report with every operation's statistics fully computed.
///
/// Produced by [`Report::finalize`](crate::Report::finalize). There is exactly
/// one report and it is always fully calculated: the slope and its confidence
/// interval are resolved for every operation up front, regardless of which
/// outputs (if any) end up consuming them.
#[derive(Clone, Debug)]
pub struct FinalizedReport {
    /// Operations sorted by name, so every output presents them in a stable order.
    operations: Vec<FinalizedOperation>,
}

/// One operation's fully-computed processor time statistics.
#[derive(Clone, Debug)]
pub struct FinalizedOperation {
    name: String,
    total_iterations: u64,
    total_processor_time: Duration,
    mean: Duration,
    statistics: Option<OperationStatistics>,
}

impl FinalizedReport {
    /// Assembles a finalized report from the resolved per-operation figures.
    ///
    /// The operations are sorted by name so both outputs present them
    /// identically.
    pub(crate) fn new(mut operations: Vec<FinalizedOperation>) -> Self {
        operations.sort_by(|a, b| a.name.cmp(&b.name));
        Self { operations }
    }

    /// Whether the report captured any measurable work.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        // Emptiness means "no measurable work was recorded", matching the
        // pre-finalize `OperationMetrics::is_empty` (total iterations == 0). It is
        // deliberately independent of whether a slope could be fit: an operation
        // can hold a span yet zero iterations (a fitted slope but no measurable
        // work), or be created with no spans at all (no slope, rendered as `n/a`).
        // Keying off `statistics.is_none()` instead would misclassify the former
        // as non-empty and print a table for a run that recorded nothing.
        self.operations
            .iter()
            .all(|operation| operation.total_iterations == 0)
    }

    /// Returns an iterator over the finalized operations, sorted by name.
    pub fn operations(&self) -> impl Iterator<Item = &FinalizedOperation> {
        self.operations.iter()
    }

    /// Prints the processor time statistics to stdout.
    ///
    /// Prints nothing if no operations recorded measurable work. This may
    /// indicate that the session was part of a "list available benchmarks" probe
    /// run instead of some real activity, in which case printing anything might
    /// violate the output protocol the tool is speaking.
    // Pure stdout side effect: the empty-guard early return is unreachable from
    // the covered `Session::drop` path (it pre-checks emptiness), and stdout
    // output is not meaningfully assertable — matching the sibling `Display` impl.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)] // Too difficult to test stdout output reliably - manually tested.
    pub fn print_to_stdout(&self) {
        if self.is_empty() {
            return;
        }
        println!("{self}");
    }
}

impl FinalizedOperation {
    /// Creates a finalized operation from its resolved figures.
    pub(crate) fn new(
        name: String,
        total_iterations: u64,
        total_processor_time: Duration,
        mean: Duration,
        statistics: Option<OperationStatistics>,
    ) -> Self {
        Self {
            name,
            total_iterations,
            total_processor_time,
            mean,
            statistics,
        }
    }

    /// The operation name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Total iterations recorded across all spans.
    #[must_use]
    pub fn total_iterations(&self) -> u64 {
        self.total_iterations
    }

    /// Total processor time recorded across all spans.
    #[must_use]
    pub fn total_processor_time(&self) -> Duration {
        self.total_processor_time
    }

    /// Mean processor time per iteration across all spans.
    #[must_use]
    pub fn mean(&self) -> Duration {
        self.mean
    }

    /// The warmup-robust per-iteration statistics, or `None` when no spans were
    /// recorded.
    #[must_use]
    pub fn statistics(&self) -> Option<OperationStatistics> {
        self.statistics
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Too annoying to test every question mark operator.
impl fmt::Display for FinalizedReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            writeln!(f, "No processor time statistics captured.")?;
            return Ok(());
        }
        writeln!(f, "Processor time statistics:")?;
        writeln!(f)?;

        // Render the warmup-robust per-iteration slope, not the raw mean: the mean
        // folds warmup and one-off costs into the figure, while the slope recovers
        // the marginal per-iteration cost. The confidence interval is computed as
        // part of finalization and preserved in the JSON output and the
        // `statistics()` API; it is kept out of this summary purely for
        // readability, not to save computation.
        let cells: Vec<(&str, String)> = self
            .operations
            .iter()
            .map(|operation| {
                let value = match operation.statistics {
                    Some(statistics) => {
                        format!("{:?}", nanos_to_duration(statistics.slope_nanos))
                    }
                    None => "n/a".to_owned(),
                };
                (operation.name.as_str(), value)
            })
            .collect();

        let max_name_width = cells
            .iter()
            .map(|(name, _)| name.len())
            .max()
            .unwrap_or(0)
            .max("Operation".len());
        let max_value_width = cells
            .iter()
            .map(|(_, value)| value.len())
            .max()
            .unwrap_or(0)
            .max("Per iteration".len());

        // Print table header.
        writeln!(
            f,
            "| {:<name_width$} | {:>value_width$} |",
            "Operation",
            "Per iteration",
            name_width = max_name_width,
            value_width = max_value_width
        )?;
        let separator_name_width = max_name_width
            .checked_add(2)
            .expect("operation name width fits in memory, adding 2 cannot overflow");
        let separator_value_width = max_value_width
            .checked_add(2)
            .expect("value width fits in memory, adding 2 cannot overflow");
        writeln!(
            f,
            "|{:-<name_width$}|{:-<value_width$}|",
            "",
            "",
            name_width = separator_name_width,
            value_width = separator_value_width
        )?;

        // Print table rows.
        for (name, value) in cells {
            writeln!(f, "| {name:<max_name_width$} | {value:>max_value_width$} |")?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use super::*;

    // The finalized report is a plain owned value: thread-safe and unwind-safe.
    static_assertions::assert_impl_all!(FinalizedReport: Send, Sync);
    static_assertions::assert_impl_all!(FinalizedOperation: Send, Sync);
    static_assertions::assert_impl_all!(FinalizedReport: UnwindSafe, RefUnwindSafe);
    static_assertions::assert_impl_all!(FinalizedOperation: UnwindSafe, RefUnwindSafe);

    #[test]
    fn empty_report_is_empty() {
        let report = FinalizedReport::new(Vec::new());
        assert!(report.is_empty());
    }

    #[test]
    fn operations_are_sorted_by_name() {
        let report = FinalizedReport::new(vec![
            FinalizedOperation::new(
                "zebra".to_owned(),
                1,
                Duration::from_nanos(10),
                Duration::from_nanos(10),
                Some(OperationStatistics {
                    span_count: 1,
                    slope_nanos: 10.0,
                    interval_nanos: None,
                }),
            ),
            FinalizedOperation::new(
                "alpha".to_owned(),
                1,
                Duration::from_nanos(20),
                Duration::from_nanos(20),
                Some(OperationStatistics {
                    span_count: 1,
                    slope_nanos: 20.0,
                    interval_nanos: None,
                }),
            ),
        ]);

        let names: Vec<&str> = report.operations().map(FinalizedOperation::name).collect();
        assert_eq!(names, ["alpha", "zebra"]);
    }

    #[test]
    fn report_with_only_unmeasured_operations_is_empty() {
        let report = FinalizedReport::new(vec![FinalizedOperation::new(
            "unmeasured".to_owned(),
            0,
            Duration::ZERO,
            Duration::ZERO,
            None,
        )]);
        assert!(report.is_empty());
    }

    #[test]
    fn report_with_recorded_work_but_no_statistics_is_not_empty() {
        // is_empty reflects recorded work (iterations), independent of whether a
        // slope was fit: an operation with recorded iterations is never empty even
        // when its statistics are absent. Guards against regressing back to keying
        // off `statistics.is_none()`.
        let report = FinalizedReport::new(vec![FinalizedOperation::new(
            "recorded".to_owned(),
            5,
            Duration::from_nanos(50),
            Duration::from_nanos(10),
            None,
        )]);
        assert!(!report.is_empty());
    }
}
