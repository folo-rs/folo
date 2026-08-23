//! Publisher for exporting nm metrics to OpenTelemetry.

use std::any::{type_name, type_name_of_val};
use std::fmt;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::time::Duration;

use futures::StreamExt;
use nm::Report;
use opentelemetry::metrics::MeterProvider;
use tick::{Clock, PeriodicTimer};

use crate::mapping::{InstrumentRegistry, export_report};
use crate::state::CollectionState;

/// Balances publication freshness against recurring collection and processing overhead.
const DEFAULT_INTERVAL: Duration = Duration::from_mins(1);

/// Identifies measurements as originating from nm in OpenTelemetry instrumentation scopes.
const DEFAULT_METER_NAME: &str = "nm";

/// Builder for configuring an nm-to-OpenTelemetry publisher.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use nm_otel::Publisher;
/// use tick::Clock;
/// # use opentelemetry_sdk::metrics::{
/// #     InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
/// # };
///
/// # let exporter = InMemoryMetricExporter::default();
/// # let reader = PeriodicReader::builder(exporter).build();
/// # let provider = SdkMeterProvider::builder().with_reader(reader).build();
/// let publisher = Publisher::builder()
///     .provider(provider)
///     .clock(Clock::new_tokio())
///     .interval(Duration::from_secs(5))
///     .build();
/// ```
pub struct PublisherBuilder {
    provider: Option<Box<dyn MeterProvider + Send>>,
    clock: Option<Clock>,
    interval: Duration,
    meter_name: &'static str,
}

// `dyn MeterProvider` does not implement `Debug`, so the provider is represented by its
// presence rather than its value.
// Debug formatting has no contractual representation, so mutations cannot be tested
// meaningfully.
#[cfg_attr(test, mutants::skip)]
#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for PublisherBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("provider", &self.provider.as_ref().map(|_| ".."))
            .field("clock", &self.clock)
            .field("interval", &self.interval)
            .field("meter_name", &self.meter_name)
            .finish()
    }
}

// `PublisherBuilder` stores an OpenTelemetry `MeterProvider` trait object.
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
    /// Transfers ownership of the provider into the resulting publisher, which retains it to keep
    /// the metric pipeline operational. The provider must implement [`Send`] so the publisher can
    /// be moved between threads.
    /// A provider must be configured before calling [`build()`][Self::build].
    #[must_use]
    pub fn provider(mut self, provider: impl MeterProvider + Send + 'static) -> Self {
        self.provider = Some(Box::new(provider));
        self
    }

    /// Sets the clock for timing.
    ///
    /// A clock must be configured before calling [`build()`][Self::build].
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
    /// Panics unless both a meter provider and a clock have been configured.
    #[must_use]
    pub fn build(self) -> Publisher {
        let Some(provider) = self.provider else {
            panic!("a meter provider must be configured before building a publisher");
        };
        let Some(clock) = self.clock else {
            panic!("a clock must be configured before building a publisher");
        };
        let instruments = InstrumentRegistry::new(provider.meter(self.meter_name));

        Publisher {
            clock,
            interval: self.interval,
            state: CollectionState::new(),
            instruments,
            provider,
        }
    }
}

/// Publishes nm metrics to OpenTelemetry.
///
/// This type periodically collects metrics from nm and publishes them through OpenTelemetry
/// instruments. It tracks state between collections to compute deltas for counter-type
/// metrics.
///
/// # Example
///
/// ```no_run
/// use nm_otel::Publisher;
/// use tick::Clock;
/// # use opentelemetry_sdk::metrics::{
/// #     InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
/// # };
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// # let exporter = InMemoryMetricExporter::default();
/// # let reader = PeriodicReader::builder(exporter).build();
/// # let provider = SdkMeterProvider::builder().with_reader(reader).build();
/// let mut publisher = Publisher::builder()
///     .provider(provider)
///     .clock(Clock::new_tokio())
///     .build();
///
/// let _publisher_task = tokio::spawn(async move {
///     publisher.publish_forever().await;
/// });
/// # }
/// ```
pub struct Publisher {
    clock: Clock,
    interval: Duration,
    state: CollectionState,
    instruments: InstrumentRegistry,

    /// Keeps the metric pipeline alive and drops after the instruments that depend on it.
    provider: Box<dyn MeterProvider + Send>,
}

// `dyn MeterProvider` does not implement `Debug`, so the provider is represented by its presence.
#[cfg_attr(test, mutants::skip)]
#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for Publisher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("provider", &type_name_of_val(self.provider.as_ref()))
            .field("clock", &self.clock)
            .field("interval", &self.interval)
            .field("state", &self.state)
            .field("instruments", &self.instruments)
            .finish()
    }
}

// `Publisher` wraps OpenTelemetry SDK trait objects that are not auto-RefUnwindSafe.
// Publication exposes no borrowed internal state across an unwind, so catching a provider panic
// cannot violate memory safety. These marker traits do not make publication transactional: a
// provider panic may leave some instruments and delta baselines advanced, and the publisher is
// not intended to be retried after such a panic.
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
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use nm_otel::Publisher;
    /// use tick::Clock;
    /// # use opentelemetry_sdk::metrics::{
    /// #     InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
    /// # };
    ///
    /// # let exporter = InMemoryMetricExporter::default();
    /// # let reader = PeriodicReader::builder(exporter).build();
    /// # let provider = SdkMeterProvider::builder().with_reader(reader).build();
    /// let publisher = Publisher::builder()
    ///     .provider(provider)
    ///     .clock(Clock::new_tokio())
    ///     .interval(Duration::from_secs(5))
    ///     .build();
    /// ```
    #[must_use]
    pub fn builder() -> PublisherBuilder {
        PublisherBuilder::new()
    }

    /// Runs the publisher indefinitely, collecting and publishing metrics at each interval.
    ///
    /// This method never returns under normal operation. Drop the future to cancel
    /// the publishing.
    ///
    /// # Panics
    ///
    /// Panics if nm report collection finds incompatible configurations for the same event, or
    /// if successive collections contain different histogram bucket counts for one event.
    ///
    /// # Reentrancy
    ///
    /// OpenTelemetry callbacks may record nm events, but must not attempt to poll this same
    /// publisher future recursively.
    // Mutations of this perpetual loop can busy-loop without yielding. Mutation testing
    // disables watchdogs, so executing such a mutant hangs the test process.
    #[cfg_attr(test, mutants::skip)]
    pub async fn publish_forever(&mut self) {
        let mut timer = PeriodicTimer::new(&self.clock, self.interval);

        while timer.next().await.is_some() {
            self.run_one_iteration();
        }
    }

    /// Collects and publishes metrics once.
    ///
    /// # Panics
    ///
    /// Panics if nm report collection finds incompatible configurations for the same event, or
    /// if the collection contains a different histogram bucket count from a previous collection.
    #[doc(hidden)]
    pub fn run_one_iteration(&mut self) {
        let report = Report::collect();
        self.export(&report);
    }

    /// Publishes the supplied report once.
    ///
    /// This bypasses [`Report::collect`] so callers can drive the export pipeline with
    /// a pre-built [`Report`] (typically constructed via `Report::fake` in tests and
    /// benchmarks).
    ///
    /// # Panics
    ///
    /// Panics if the report contains a different histogram bucket count from a previous
    /// report for the same event.
    #[cfg(any(test, feature = "private-test-util"))]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[doc(hidden)]
    pub fn run_one_iteration_with_report(&mut self, report: &Report) {
        self.export(report);
    }

    fn export(&mut self, report: &Report) {
        export_report(report, &mut self.state, &mut self.instruments);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use nm::{EventMetrics, Histogram, Magnitude};
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
    use static_assertions::assert_impl_all;
    use testing::assert_panics;

    use super::*;
    use crate::create_test_provider;

    assert_impl_all!(Publisher: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(PublisherBuilder: UnwindSafe, RefUnwindSafe);

    fn create_test_clock() -> Clock {
        Clock::new_frozen()
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn builder_with_defaults() {
        let (provider, _) = create_test_provider();

        let publisher = Publisher::builder()
            .provider(provider)
            .clock(create_test_clock())
            .build();

        assert_eq!(publisher.interval, DEFAULT_INTERVAL);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn builder_with_custom_interval() {
        let (provider, _) = create_test_provider();

        let publisher = Publisher::builder()
            .provider(provider)
            .clock(create_test_clock())
            .interval(Duration::from_secs(5))
            .build();

        assert_eq!(publisher.interval, Duration::from_secs(5));
    }

    #[test]
    fn builder_without_provider_panics() {
        assert_panics(|| {
            _ = Publisher::builder().clock(create_test_clock()).build();
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn builder_without_clock_panics() {
        let (provider, _) = create_test_provider();
        let builder = Publisher::builder().provider(provider);

        assert_panics(|| {
            _ = builder.build();
        });
    }

    const FAKE_EVENT_NAME: &str = "fake_event";
    const FAKE_HISTOGRAM_MAGNITUDES: &[Magnitude] = &[10, 50, 100];

    fn make_fake_report(
        count: u64,
        sum: Magnitude,
        bucket_counts: Vec<u64>,
        plus_infinity_bucket_count: u64,
    ) -> Report {
        let histogram = Histogram::fake(
            FAKE_HISTOGRAM_MAGNITUDES,
            bucket_counts,
            plus_infinity_bucket_count,
        );
        let event = EventMetrics::fake(FAKE_EVENT_NAME, count, sum, Some(histogram));
        Report::fake(vec![event])
    }

    fn find_metric_value(
        metrics: &ResourceMetrics,
        name: &str,
        bucket: Option<&str>,
    ) -> Option<u64> {
        for scope_metrics in metrics.scope_metrics() {
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
        None
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn run_one_iteration_with_report_publishes_fake_report() {
        const METER_NAME: &str = "custom_meter_name_for_test";

        let (provider, reader) = create_test_provider();

        let mut publisher = Publisher::builder()
            .provider(provider)
            .clock(create_test_clock())
            .meter_name(METER_NAME)
            .build();

        let initial_report = make_fake_report(10, 4567, vec![4, 3, 2], 1);
        publisher.run_one_iteration_with_report(&initial_report);

        let metrics = reader.collect();
        let has_expected_scope = metrics
            .scope_metrics()
            .map(|scope_metrics| scope_metrics.scope().name())
            .any(|scope_name| scope_name == METER_NAME);
        assert!(has_expected_scope);

        assert_eq!(find_metric_value(&metrics, FAKE_EVENT_NAME, None), Some(10));

        let next_report = make_fake_report(25, 8901, vec![6, 5, 3], 2);
        publisher.run_one_iteration_with_report(&next_report);
        let metrics = reader.collect();

        // OpenTelemetry counters accumulate across flushes, so the observed total
        // distinguishes publishing deltas from replaying raw cumulative report values.
        assert_eq!(find_metric_value(&metrics, FAKE_EVENT_NAME, None), Some(25));

        // Bucket counters have the same cumulative OpenTelemetry semantics and therefore
        // verify delta publication independently for every bound and the overflow bucket.
        let bucket_metric = format!("{FAKE_EVENT_NAME}_bucket");
        assert_eq!(
            find_metric_value(&metrics, &bucket_metric, Some("10")),
            Some(6)
        );
        assert_eq!(
            find_metric_value(&metrics, &bucket_metric, Some("50")),
            Some(11)
        );
        assert_eq!(
            find_metric_value(&metrics, &bucket_metric, Some("100")),
            Some(14)
        );
        assert_eq!(
            find_metric_value(&metrics, &bucket_metric, Some("+Inf")),
            Some(16)
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn run_one_iteration_with_report_panics_on_incompatible_histograms() {
        let (provider, _) = create_test_provider();
        let mut publisher = Publisher::builder()
            .provider(provider)
            .clock(create_test_clock())
            .build();

        let initial_report = make_fake_report(10, 4567, vec![4, 3, 2], 1);
        publisher.run_one_iteration_with_report(&initial_report);

        let histogram = Histogram::fake(&[10, 50], vec![6, 5], 2);
        let event = EventMetrics::fake(FAKE_EVENT_NAME, 25, 8901, Some(histogram));
        let incompatible_report = Report::fake(vec![event]);

        assert_panics(|| {
            publisher.run_one_iteration_with_report(&incompatible_report);
        });
    }
}
