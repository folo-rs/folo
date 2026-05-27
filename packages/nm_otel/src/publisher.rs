//! Publisher for exporting nm metrics to OpenTelemetry.

use std::fmt;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::time::Duration;

use futures::StreamExt;
use nm::Report;
use opentelemetry::metrics::{Meter, MeterProvider};
use tick::{Clock, PeriodicTimer};

use crate::mapping::{InstrumentRegistry, export_report};
use crate::state::CollectionState;

/// Default collection interval.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);

/// Default meter name.
const DEFAULT_METER_NAME: &str = "nm";

/// Builder for configuring an nm-to-OpenTelemetry publisher.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use nm_otel::Publisher;
/// use tick::Clock;
/// # use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
///
/// # let exporter = InMemoryMetricExporter::default();
/// # let reader = PeriodicReader::builder(exporter).build();
/// # let my_meter_provider = SdkMeterProvider::builder().with_reader(reader).build();
/// let publisher = Publisher::builder()
///     .provider(my_meter_provider)
///     .clock(Clock::new_frozen())
///     .interval(Duration::from_secs(5))
///     .build();
/// ```
pub struct PublisherBuilder {
    provider: Option<Box<dyn MeterProvider>>,
    clock: Option<Clock>,
    interval: Duration,
    meter_name: &'static str,
}

// dyn MeterProvider does not implement Debug, so we provide a manual implementation
// that omits the provider field.
impl fmt::Debug for PublisherBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublisherBuilder")
            .field("provider", &self.provider.as_ref().map(|_| ".."))
            .field("clock", &self.clock)
            .field("interval", &self.interval)
            .field("meter_name", &self.meter_name)
            .finish()
    }
}

// PublisherBuilder stores an OpenTelemetry MeterProvider trait object.
// The trait object is not auto-RefUnwindSafe but is used only for metrics export
// configuration. Inconsistent state after a caught panic cannot affect safety.
impl UnwindSafe for PublisherBuilder {}
impl RefUnwindSafe for PublisherBuilder {}

impl PublisherBuilder {
    fn new() -> Self {
        Self {
            provider: None,
            clock: None,
            interval: DEFAULT_INTERVAL,
            meter_name: DEFAULT_METER_NAME,
        }
    }

    /// Sets the OpenTelemetry meter provider.
    ///
    /// Accepts any [`MeterProvider`] implementation.
    /// This is required - the builder will panic if not set.
    #[must_use]
    pub fn provider(mut self, provider: impl MeterProvider + 'static) -> Self {
        self.provider = Some(Box::new(provider));
        self
    }

    /// Sets the clock for timing.
    ///
    /// This is required for async operation - the builder will panic if not set.
    #[must_use]
    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Sets the collection interval.
    ///
    /// Defaults to 60 seconds.
    #[must_use]
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Sets the meter name.
    ///
    /// Defaults to "nm".
    #[must_use]
    pub fn meter_name(mut self, name: &'static str) -> Self {
        self.meter_name = name;
        self
    }

    /// Builds the publisher.
    ///
    /// # Panics
    ///
    /// Panics if no provider has been set.
    /// Panics if no clock has been set.
    #[must_use]
    pub fn build(self) -> Publisher {
        let provider = self
            .provider
            .expect("provider is required - call .provider() before .build()");

        let clock = self
            .clock
            .expect("clock is required - call .clock() before .build()");

        let meter = provider.meter(self.meter_name);

        Publisher {
            meter,
            clock,
            interval: self.interval,
            state: CollectionState::new(),
            instruments: None,
        }
    }
}

/// Publishes nm metrics to OpenTelemetry.
///
/// This type collects metrics from nm periodically and exports them to OpenTelemetry
/// instruments. It tracks state between collections to compute deltas for counter-type
/// metrics.
///
/// # Example
///
/// ```no_run
/// use nm_otel::Publisher;
/// use tick::Clock;
/// # use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
///
/// # let exporter = InMemoryMetricExporter::default();
/// # let reader = PeriodicReader::builder(exporter).build();
/// # let provider = SdkMeterProvider::builder().with_reader(reader).build();
/// # let mut publisher = Publisher::builder()
/// #     .provider(provider)
/// #     .clock(Clock::new_frozen())
/// #     .build();
/// # async fn example(mut publisher: nm_otel::Publisher) {
/// // Spawn as a background task in your async runtime.
/// publisher.publish_forever().await;
/// # }
/// ```
#[derive(Debug)]
pub struct Publisher {
    meter: Meter,
    clock: Clock,
    interval: Duration,
    state: CollectionState,
    instruments: Option<InstrumentRegistry>,
}

// Publisher wraps OpenTelemetry SDK types that use trait objects internally.
// The trait objects are not auto-RefUnwindSafe but are used only for metrics export.
// Inconsistent state after a caught panic cannot affect safety.
impl UnwindSafe for Publisher {}
impl RefUnwindSafe for Publisher {}

impl Publisher {
    /// Creates a new publisher builder.
    ///
    /// This is the recommended way to create a publisher. Use the builder methods
    /// to configure the publisher before calling `.build()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use nm_otel::Publisher;
    /// use tick::Clock;
    /// # use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
    ///
    /// # let exporter = InMemoryMetricExporter::default();
    /// # let reader = PeriodicReader::builder(exporter).build();
    /// # let my_meter_provider = SdkMeterProvider::builder().with_reader(reader).build();
    /// let publisher = Publisher::builder()
    ///     .provider(my_meter_provider)
    ///     .clock(Clock::new_frozen())
    ///     .interval(Duration::from_secs(5))
    ///     .build();
    /// ```
    #[must_use]
    pub fn builder() -> PublisherBuilder {
        PublisherBuilder::new()
    }

    /// Runs the publisher forever, collecting and exporting metrics at each interval.
    ///
    /// This method never returns under normal operation. Drop the future to cancel
    /// the publishing.
    pub async fn publish_forever(&mut self) {
        let mut timer = PeriodicTimer::new(&self.clock, self.interval);

        while timer.next().await.is_some() {
            self.run_one_iteration();
        }
    }

    /// Runs a single collection iteration.
    ///
    /// This is an internal method for testing purposes.
    #[doc(hidden)]
    pub fn run_one_iteration(&mut self) {
        let report = Report::collect();
        self.export(&report);
    }

    /// Runs a single export iteration using the supplied report.
    ///
    /// This bypasses [`Report::collect`] so callers can drive the export pipeline with
    /// fabricated reports built via [`nm::Report::fake`]. Available only under the
    /// `test-util` feature for use by benchmarks and tests.
    #[cfg(any(test, feature = "test-util"))]
    #[doc(hidden)]
    pub fn run_one_iteration_with_report(&mut self, report: &Report) {
        self.export(report);
    }

    fn export(&mut self, report: &Report) {
        // Lazily initialize instruments on first use.
        if self.instruments.is_none() {
            self.instruments = Some(InstrumentRegistry::new(self.meter.clone()));
        }

        let instruments = self
            .instruments
            .as_mut()
            .expect("we just initialized it above");

        export_report(report, &mut self.state, instruments);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use nm::{EventMetrics, Histogram, Magnitude};
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Publisher: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(PublisherBuilder: UnwindSafe, RefUnwindSafe);

    fn create_test_provider() -> (SdkMeterProvider, InMemoryMetricExporter) {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone()).build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        (provider, exporter)
    }

    fn create_test_clock() -> Clock {
        Clock::new_frozen()
    }

    // The OpenTelemetry SDK calls `env::current_exe()` during default resource detection,
    // which Miri aborts on. Tracked upstream as
    // https://github.com/open-telemetry/opentelemetry-rust/issues/3523 (regression in 0.32).
    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_with_defaults() {
        let (provider, _exporter) = create_test_provider();

        let publisher = Publisher::builder()
            .provider(provider)
            .clock(create_test_clock())
            .build();

        assert_eq!(publisher.interval, DEFAULT_INTERVAL);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_with_custom_interval() {
        let (provider, _exporter) = create_test_provider();

        let publisher = Publisher::builder()
            .provider(provider)
            .clock(create_test_clock())
            .interval(Duration::from_secs(5))
            .build();

        assert_eq!(publisher.interval, Duration::from_secs(5));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_with_custom_meter_name() {
        let (provider, _exporter) = create_test_provider();

        let _publisher = Publisher::builder()
            .provider(provider)
            .clock(create_test_clock())
            .meter_name("custom_meter")
            .build();
    }

    #[test]
    #[should_panic]
    fn builder_without_provider_panics() {
        let _publisher = Publisher::builder().clock(create_test_clock()).build();
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    #[should_panic]
    fn builder_without_clock_panics() {
        let (provider, _exporter) = create_test_provider();
        let _publisher = Publisher::builder().provider(provider).build();
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn run_one_iteration_collects_metrics() {
        let (provider, exporter) = create_test_provider();

        let mut publisher = Publisher::builder()
            .provider(provider.clone())
            .clock(create_test_clock())
            .build();

        publisher.run_one_iteration();
        provider.force_flush().unwrap();

        // The report may be empty if no nm events have been recorded,
        // but the operation should complete successfully.
        let metrics = exporter.get_finished_metrics().unwrap();

        // We do not assert on content since nm may have no events.
        // The test validates that the iteration completes without error.
        drop(metrics);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn multiple_iterations_track_state() {
        let (provider, _exporter) = create_test_provider();

        let mut publisher = Publisher::builder()
            .provider(provider)
            .clock(create_test_clock())
            .build();

        // Run multiple iterations to verify state tracking works.
        publisher.run_one_iteration();
        publisher.run_one_iteration();
        publisher.run_one_iteration();

        // If this completes without panic, state tracking is working.
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn builder_debug_formatting() {
        let (provider, _exporter) = create_test_provider();

        let builder = Publisher::builder().provider(provider);
        let debug = format!("{builder:?}");

        assert!(debug.contains("PublisherBuilder"));
    }

    const FAKE_EVENT_NAME: &str = "fake_event";
    const FAKE_HISTOGRAM_MAGNITUDES: &[Magnitude] = &[10, 50, 100];

    fn make_fake_report(
        count: u64,
        sum: Magnitude,
        bucket_counts: Vec<u64>,
        plus_inf: u64,
    ) -> Report {
        let histogram = Histogram::fake(FAKE_HISTOGRAM_MAGNITUDES, bucket_counts, plus_inf);
        let event = EventMetrics::fake(FAKE_EVENT_NAME, count, sum, Some(histogram));
        Report::fake(vec![event])
    }

    fn find_metric_value(
        exporter: &InMemoryMetricExporter,
        name: &str,
        bucket: Option<&str>,
    ) -> Option<u64> {
        let metrics = exporter.get_finished_metrics().ok()?;
        for resource_metrics in &metrics {
            for scope_metrics in resource_metrics.scope_metrics() {
                for metric in scope_metrics.metrics() {
                    if metric.name() != name {
                        continue;
                    }
                    let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                        continue;
                    };
                    for point in sum.data_points() {
                        let matches = match bucket {
                            None => point.attributes().next().is_none(),
                            Some(expected) => point.attributes().any(|kv: &KeyValue| {
                                kv.key.as_str() == "le" && kv.value.as_str() == expected
                            }),
                        };
                        if matches {
                            return Some(point.value());
                        }
                    }
                }
            }
        }
        None
    }

    // Same Miri limitation as the other tests above
    // (https://github.com/open-telemetry/opentelemetry-rust/issues/3523).
    #[cfg_attr(miri, ignore)]
    #[test]
    fn run_one_iteration_with_report_exports_fake_report() {
        const METER_NAME: &str = "custom_meter_name_for_test";

        let (provider, exporter) = create_test_provider();

        let mut publisher = Publisher::builder()
            .provider(provider.clone())
            .clock(create_test_clock())
            .meter_name(METER_NAME)
            .build();

        // First iteration: cumulative buckets are [4, 7, 9] from non-cumulative [4, 3, 2],
        // plus 1 in the `+Inf` overflow bucket.
        let report1 = make_fake_report(10, 4567, vec![4, 3, 2], 1);
        publisher.run_one_iteration_with_report(&report1);
        provider.force_flush().unwrap();

        // Verify the custom meter name propagates into the OTel instrumentation scope.
        let metrics = exporter.get_finished_metrics().unwrap();
        let observed_scope_names: Vec<&str> = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .map(|sm| sm.scope().name())
            .collect();
        assert!(
            observed_scope_names.contains(&METER_NAME),
            "expected scope name {METER_NAME:?} in {observed_scope_names:?}"
        );

        assert_eq!(
            find_metric_value(&exporter, FAKE_EVENT_NAME, None),
            Some(10)
        );
        exporter.reset();

        // Second iteration: non-cumulative becomes [6, 5, 3], cumulative [6, 11, 14],
        // so per-bucket deltas vs the first iteration are [2, 4, 5]. Counter delta:
        // 25 - 10 = 15.
        let report2 = make_fake_report(25, 8901, vec![6, 5, 3], 2);
        publisher.run_one_iteration_with_report(&report2);
        provider.force_flush().unwrap();

        // OTel counters accumulate across flushes. After iter 2 the exported cumulative is
        // 10 + 15 = 25. This confirms the publisher shipped a *delta* of 15 (not the raw
        // cumulative of 25, which would yield 10 + 25 = 35).
        assert_eq!(
            find_metric_value(&exporter, FAKE_EVENT_NAME, None),
            Some(25)
        );

        // Histogram bucket counters likewise accumulate. After iter 1 their cumulative
        // values are [4, 7, 9, 10]; iter 2 deltas are [2, 4, 5, 6]; so post-iter-2
        // cumulatives are [6, 11, 14, 16]. Any of these matching the expected
        // sum-of-deltas confirms that histogram_deltas is computing correctly across
        // iterations.
        let bucket_metric = format!("{FAKE_EVENT_NAME}_bucket");
        assert_eq!(
            find_metric_value(&exporter, &bucket_metric, Some("10")),
            Some(6)
        );
        assert_eq!(
            find_metric_value(&exporter, &bucket_metric, Some("50")),
            Some(11)
        );
        assert_eq!(
            find_metric_value(&exporter, &bucket_metric, Some("100")),
            Some(14)
        );
        assert_eq!(
            find_metric_value(&exporter, &bucket_metric, Some("+Inf")),
            Some(16)
        );
    }
}
