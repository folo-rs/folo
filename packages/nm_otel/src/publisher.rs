//! Publisher for exporting nm metrics to OpenTelemetry.

use std::time::Duration;

use futures::StreamExt;
use nm::Report;
use opentelemetry::metrics::{Meter, MeterProvider as _};
use opentelemetry_sdk::metrics::SdkMeterProvider;
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
#[derive(Debug)]
pub struct PublisherBuilder {
    provider: Option<SdkMeterProvider>,
    clock: Option<Clock>,
    interval: Duration,
    meter_name: &'static str,
}

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
    /// This is required - the builder will panic if not set.
    #[must_use]
    pub fn provider(mut self, provider: SdkMeterProvider) -> Self {
        self.provider = Some(provider);
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
/// # let mut publisher = Publisher::builder().provider(provider).clock(Clock::new_frozen()).build();
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
    /// This method never returns under normal operation. Drop the future to cancel the publishing.
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
        // Lazily initialize instruments on first use.
        if self.instruments.is_none() {
            self.instruments = Some(InstrumentRegistry::new(self.meter.clone()));
        }

        let report = Report::collect();

        let instruments = self
            .instruments
            .as_mut()
            .expect("we just initialized it above");

        export_report(&report, &mut self.state, instruments);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader};

    use super::*;

    fn create_test_provider() -> (SdkMeterProvider, InMemoryMetricExporter) {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone()).build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        (provider, exporter)
    }

    fn create_test_clock() -> Clock {
        Clock::new_frozen()
    }

    #[test]
    fn builder_with_defaults() {
        let (provider, _exporter) = create_test_provider();

        let publisher = Publisher::builder()
            .provider(provider)
            .clock(create_test_clock())
            .build();

        assert_eq!(publisher.interval, DEFAULT_INTERVAL);
    }

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

    #[test]
    #[should_panic]
    fn builder_without_clock_panics() {
        let (provider, _exporter) = create_test_provider();
        let _publisher = Publisher::builder().provider(provider).build();
    }

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
}
