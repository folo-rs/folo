use std::collections::HashMap;
use std::time::Duration;

use nm::{EventMetrics, Report};
use opentelemetry::metrics::{Histogram, Meter, MeterProvider, UpDownCounter};
use tick::Clock;

/// A publisher that periodically collects nm metrics and exports them to OpenTelemetry.
///
/// # Example
///
/// ```no_run
/// use std::time::Duration;
///
/// use nm_otel::Publisher;
///
/// # async fn example() {
/// Publisher::new()
///     .interval(Duration::from_secs(10))
///     .publish_forever()
///     .await;
/// # }
/// ```
#[derive(Debug)]
pub struct Publisher {
    meter: Meter,
    interval: Duration,
    clock: Clock,
}

impl Publisher {
    /// Creates a new publisher with a default meter.
    ///
    /// # Example
    ///
    /// ```
    /// use nm_otel::Publisher;
    ///
    /// let publisher = Publisher::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        let meter_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder().build();
        let meter = meter_provider.meter("nm_otel");

        Self {
            meter,
            interval: Duration::from_secs(60),
            clock: Clock::new_tokio(),
        }
    }

    /// Creates a publisher with a custom meter.
    ///
    /// This allows you to use your own OpenTelemetry meter provider and configuration.
    ///
    /// # Example
    ///
    /// ```
    /// use nm_otel::Publisher;
    /// use opentelemetry_sdk::metrics::MeterProvider;
    ///
    /// let meter_provider = MeterProvider::builder().build();
    /// let meter = meter_provider.meter("my_app");
    ///
    /// let publisher = Publisher::meter(meter);
    /// ```
    #[must_use]
    pub fn meter(meter: Meter) -> Self {
        Self {
            meter,
            interval: Duration::from_secs(60),
            clock: Clock::new_tokio(),
        }
    }

    /// Sets the interval between metric exports.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use nm_otel::Publisher;
    ///
    /// let publisher = Publisher::new().interval(Duration::from_secs(30));
    /// ```
    #[must_use]
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Runs one iteration of metric collection and export.
    ///
    /// This is exposed for testing purposes to allow testing the publishing logic
    /// without entering the infinite loop or dealing with delays.
    pub fn run_one_iteration(&self) {
        let report = Report::collect();
        self.export_report(&report);
    }

    /// Publishes metrics forever in a loop, sleeping between iterations.
    ///
    /// This future never completes.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use nm_otel::Publisher;
    ///
    /// # async fn example() {
    /// Publisher::new()
    ///     .interval(Duration::from_secs(10))
    ///     .publish_forever()
    ///     .await;
    /// # }
    /// ```
    pub async fn publish_forever(&self) -> ! {
        loop {
            self.run_one_iteration();
            self.clock.delay(self.interval).await;
        }
    }

    fn export_report(&self, report: &Report) {
        let mut state = InstrumentState::new();

        for event in report.events() {
            self.export_event(event, &mut state);
        }
    }

    fn export_event(&self, event: &EventMetrics, state: &mut InstrumentState) {
        let event_name = event.name();

        // Decide what type of instrument to use based on the event metadata.
        // We cannot make assumptions about the data (e.g., whether magnitudes are always 1)
        // because nm events are flexible and can collect any magnitude at any time.
        if let Some(histogram_data) = event.histogram() {
            // This event has histogram buckets configured, so we export it as an
            // OpenTelemetry histogram.
            let histogram = state.get_or_create_histogram(&self.meter, event_name);

            // Record each bucket's occurrences.
            for (magnitude, count) in histogram_data.buckets() {
                // Record each occurrence at the bucket magnitude.
                for _ in 0..count {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "converting magnitude to f64 for OpenTelemetry - precision loss is acceptable"
                    )]
                    histogram.record(magnitude as f64, &[]);
                }
            }
        } else {
            // This event has no histogram buckets, so we use an UpDownCounter.
            // We use UpDownCounter because it can handle arbitrary positive and negative
            // magnitudes, matching the flexibility of nm events.
            let counter = state.get_or_create_updown_counter(&self.meter, event_name);
            counter.add(event.sum(), &[]);
        }
    }
}

impl Default for Publisher {
    fn default() -> Self {
        Self::new()
    }
}

/// Manages the lifecycle of OpenTelemetry instruments.
///
/// Instruments are created on-demand when events are first seen and reused
/// in subsequent iterations.
struct InstrumentState {
    histograms: HashMap<String, Histogram<f64>>,
    updown_counters: HashMap<String, UpDownCounter<i64>>,
}

impl InstrumentState {
    fn new() -> Self {
        Self {
            histograms: HashMap::new(),
            updown_counters: HashMap::new(),
        }
    }

    fn get_or_create_histogram(&mut self, meter: &Meter, name: &str) -> Histogram<f64> {
        let name_owned = name.to_string();
        self.histograms
            .entry(name_owned.clone())
            .or_insert_with(|| meter.f64_histogram(name_owned).build())
            .clone()
    }

    fn get_or_create_updown_counter(&mut self, meter: &Meter, name: &str) -> UpDownCounter<i64> {
        let name_owned = name.to_string();
        self.updown_counters
            .entry(name_owned.clone())
            .or_insert_with(|| meter.i64_up_down_counter(name_owned).build())
            .clone()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publisher_can_be_created() {
        let publisher = Publisher::new();
        assert_eq!(publisher.interval, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn publisher_interval_can_be_set() {
        let publisher = Publisher::new().interval(Duration::from_secs(30));
        assert_eq!(publisher.interval, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn publisher_default_is_same_as_new() {
        let publisher1 = Publisher::new();
        let publisher2 = Publisher::default();
        assert_eq!(publisher1.interval, publisher2.interval);
    }

    #[tokio::test]
    async fn run_one_iteration_does_not_panic() {
        let publisher = Publisher::new();
        publisher.run_one_iteration();
    }
}
