use std::collections::HashMap;
use std::time::Duration;

use nm::{EventMetrics, Report};
use opentelemetry::metrics::{Counter, Histogram, Meter, MeterProvider, UpDownCounter};

use crate::clock::{Clock, RealClock};

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
pub struct Publisher<C = RealClock>
where
    C: Clock,
{
    meter: Meter,
    interval: Duration,
    clock: C,
}

impl Publisher<RealClock> {
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
            clock: RealClock,
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
    /// let publisher = Publisher::with_meter(meter);
    /// ```
    #[must_use]
    pub fn with_meter(meter: Meter) -> Self {
        Self {
            meter,
            interval: Duration::from_secs(60),
            clock: RealClock,
        }
    }
}

impl<C> Publisher<C>
where
    C: Clock,
{
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

    /// Creates a publisher with a custom clock for testing.
    #[cfg(test)]
    #[expect(dead_code, reason = "will be used in future integration tests")]
    pub(crate) fn with_clock(meter: Meter, clock: C) -> Self {
        Self {
            meter,
            interval: Duration::from_secs(60),
            clock,
        }
    }

    /// Runs one iteration of metric collection and export.
    ///
    /// This is exposed for testing purposes to allow testing the publishing logic
    /// without entering the infinite loop or dealing with delays.
    #[expect(
        clippy::unused_async,
        reason = "async for API consistency, may become async in the future"
    )]
    pub async fn run_once_iteration(&self) {
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
            self.run_once_iteration().await;
            self.clock.sleep(self.interval).await;
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

        // Decide what type of instrument to use based on the event characteristics.
        if Self::is_counter_event(event) {
            // This is a simple counter (only magnitude 1 events).
            let counter = state.get_or_create_counter(&self.meter, event_name);
            counter.add(event.count(), &[]);
        } else if let Some(histogram_data) = event.histogram() {
            // This event has a histogram, so we export it as an OpenTelemetry histogram.
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
            // This is a gauge-like metric (has arbitrary magnitudes but no histogram).
            // We'll use an UpDownCounter to track the sum.
            let gauge = state.get_or_create_updown_counter(&self.meter, event_name);
            gauge.add(event.sum(), &[]);
        }
    }

    fn is_counter_event(event: &EventMetrics) -> bool {
        // A counter event is one where all observations had magnitude 1.
        // This is indicated by count == sum and no histogram.
        let count_as_i64 = i64::try_from(event.count()).unwrap_or(i64::MAX);
        count_as_i64 == event.sum() && event.histogram().is_none()
    }
}

impl Default for Publisher<RealClock> {
    fn default() -> Self {
        Self::new()
    }
}

/// Manages the lifecycle of OpenTelemetry instruments.
///
/// Instruments are created on-demand when events are first seen and reused
/// in subsequent iterations.
struct InstrumentState {
    counters: HashMap<String, Counter<u64>>,
    histograms: HashMap<String, Histogram<f64>>,
    updown_counters: HashMap<String, UpDownCounter<i64>>,
}

impl InstrumentState {
    fn new() -> Self {
        Self {
            counters: HashMap::new(),
            histograms: HashMap::new(),
            updown_counters: HashMap::new(),
        }
    }

    fn get_or_create_counter(&mut self, meter: &Meter, name: &str) -> Counter<u64> {
        let name_owned = name.to_string();
        self.counters
            .entry(name_owned.clone())
            .or_insert_with(|| meter.u64_counter(name_owned).build())
            .clone()
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

    #[test]
    fn publisher_can_be_created() {
        let publisher = Publisher::new();
        assert_eq!(publisher.interval, Duration::from_secs(60));
    }

    #[test]
    fn publisher_interval_can_be_set() {
        let publisher = Publisher::new().interval(Duration::from_secs(30));
        assert_eq!(publisher.interval, Duration::from_secs(30));
    }

    #[test]
    fn publisher_default_is_same_as_new() {
        let publisher1 = Publisher::new();
        let publisher2 = Publisher::default();
        assert_eq!(publisher1.interval, publisher2.interval);
    }

    #[tokio::test]
    async fn run_once_iteration_does_not_panic() {
        let publisher = Publisher::new();
        publisher.run_once_iteration().await;
    }
}
