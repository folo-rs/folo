use std::num::NonZero;

use crate::{EventMetrics, EventName, Histogram, Magnitude, Report};

/// Test data construction utilities for the metrics types in `nm`.
///
/// This type is **not** re-exported from the `nm` crate and is intended only
/// for use by tests and benchmarks of other workspace crates (such as
/// `nm_otel`) that need to build fabricated [`Report`], [`EventMetrics`], or
/// [`Histogram`] instances without touching the global event registry.
///
/// It is the documented replacement for the previously feature-gated
/// `Report::fake` / `EventMetrics::fake` / `Histogram::fake` constructors.
#[derive(Debug)]
#[non_exhaustive]
pub struct TestFacade;

impl TestFacade {
    /// Creates a [`Report`] instance from the supplied [`EventMetrics`] list.
    #[must_use]
    pub fn report(events: Vec<EventMetrics>) -> Report {
        Report::from_parts(events)
    }

    /// Creates an [`EventMetrics`] instance from the supplied parts.
    ///
    /// The mean is calculated as `sum / count` (rounded down). When `count`
    /// is zero, the mean is zero.
    #[must_use]
    pub fn event_metrics(
        name: impl Into<EventName>,
        count: u64,
        sum: Magnitude,
        histogram: Option<Histogram>,
    ) -> EventMetrics {
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "NonZero protects against division by zero"
        )]
        #[expect(
            clippy::integer_division,
            reason = "we accept that we lose the remainder - 100% precision not required"
        )]
        let mean = Magnitude::try_from(count)
            .ok()
            .and_then(NonZero::new)
            .map_or(0, |count| sum / count.get());

        EventMetrics::from_parts(name.into(), count, sum, mean, histogram)
    }

    /// Creates a [`Histogram`] instance from the supplied parts.
    ///
    /// The `magnitudes` slice must be sorted in ascending order. The `counts`
    /// slice must have the same length as `magnitudes`. The
    /// `plus_infinity_count` is the count for the synthetic `Magnitude::MAX`
    /// bucket.
    #[must_use]
    pub fn histogram(
        magnitudes: &'static [Magnitude],
        counts: Vec<u64>,
        plus_infinity_count: u64,
    ) -> Histogram {
        Histogram::from_parts(magnitudes, counts, plus_infinity_count)
    }
}
