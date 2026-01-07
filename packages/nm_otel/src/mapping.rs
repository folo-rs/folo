//! Mapping from nm metrics to OpenTelemetry instruments.

use std::sync::Arc;

use foldhash::HashMap;
use nm::{EventName, Magnitude, Report};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Gauge, Meter};

use crate::state::CollectionState;

/// Suffix for the sum metric name.
const SUM_SUFFIX: &str = "_sum";

/// Suffix for histogram bucket metric name.
const BUCKET_SUFFIX: &str = "_bucket";

/// Attribute key for histogram bucket upper bound (Prometheus convention).
const LE_ATTRIBUTE: &str = "le";

/// Cached OpenTelemetry instruments for a single nm event.
///
/// All instruments are created together when an event is first seen, avoiding repeated
/// string formatting on every metric lookup.
#[derive(Debug)]
struct EventInstruments {
    /// Counter for event count (named after the event itself, e.g. `http_requests`).
    count_counter: Counter<u64>,

    /// Gauge for event sum (named `{event}_sum`).
    sum_gauge: Gauge<i64>,

    /// Counter for histogram buckets (named `{event}_bucket`), if the event has a histogram.
    /// Different bucket bounds are distinguished by the `le` attribute, not by instrument name.
    bucket_counter: Option<Counter<u64>>,

    /// Cached formatted bucket bounds for the `le` attribute (e.g. "10", "50", "+Inf").
    /// Indexed by bucket index, matching the order from `histogram.magnitudes()`.
    /// Uses `Arc<str>` to avoid cloning strings on every metric export.
    bucket_bounds: Vec<Arc<str>>,
}

/// Manages OpenTelemetry instruments for nm metrics.
///
/// Creates instruments on-demand as events appear in reports and caches them for reuse.
/// All instruments for an event are created together, and string formatting only happens
/// once per event (not on every metric export).
#[derive(Debug)]
pub(crate) struct InstrumentRegistry {
    meter: Meter,

    /// Cached instruments per event name.
    events: HashMap<EventName, EventInstruments>,
}

impl InstrumentRegistry {
    /// Creates a new instrument registry with the given meter.
    pub(crate) fn new(meter: Meter) -> Self {
        Self {
            meter,
            events: HashMap::default(),
        }
    }

    /// Gets or creates instruments for an event.
    ///
    /// If histogram magnitudes are provided, the bucket counter and cached bucket bound
    /// strings are also created. This avoids creating them for events without histogram data.
    fn instruments(
        &mut self,
        event_name: &EventName,
        magnitudes: Option<impl Iterator<Item = Magnitude>>,
    ) -> &EventInstruments {
        let meter = &self.meter;
        let instruments = self.events.entry(event_name.clone()).or_insert_with(|| {
            let sum_name = format!("{event_name}{SUM_SUFFIX}");

            EventInstruments {
                count_counter: meter.u64_counter(event_name.to_string()).build(),
                sum_gauge: meter.i64_gauge(sum_name).build(),
                bucket_counter: None,
                bucket_bounds: Vec::new(),
            }
        });

        // Lazily create the bucket counter and cache bucket bounds if histogram data provided.
        if let Some(mags) = magnitudes
            && instruments.bucket_counter.is_none()
        {
            let bucket_name = format!("{event_name}{BUCKET_SUFFIX}");
            instruments.bucket_counter = Some(meter.u64_counter(bucket_name).build());
            instruments.bucket_bounds = mags.map(format_bucket_bound).collect();
        }

        instruments
    }
}

/// Exports an nm report to OpenTelemetry instruments.
///
/// This function processes each event in the report, computes deltas where needed,
/// and records values to the appropriate OpenTelemetry instruments.
pub(crate) fn export_report(
    report: &Report,
    state: &mut CollectionState,
    instruments: &mut InstrumentRegistry,
) {
    for event in report.events() {
        let event_name = event.name();
        let event_state = state.event_state(event_name);

        // Get cached instruments for this event (creates them if first time seeing this event).
        // Pass histogram magnitudes if present so bucket counter and bounds can be created.
        let event_instruments = instruments.instruments(
            event_name,
            event.histogram().map(nm::Histogram::magnitudes),
        );

        // Export count as counter (delta).
        let count_delta = event_state.count_delta(event.count());
        if count_delta > 0 {
            event_instruments.count_counter.add(count_delta, &[]);
        }

        // Export sum as gauge (absolute value).
        event_instruments.sum_gauge.record(event.sum(), &[]);

        // Export histogram buckets if present.
        if let Some(histogram) = event.histogram() {
            let bucket_counter = event_instruments
                .bucket_counter
                .as_ref()
                .expect("bucket counter was created during instruments() call");

            let bucket_deltas =
                event_state.histogram_deltas(histogram.magnitudes(), histogram.counts());

            for (i, (_magnitude, _cumulative, delta)) in bucket_deltas.into_iter().enumerate() {
                if delta > 0 {
                    let le_value = event_instruments
                        .bucket_bounds
                        .get(i)
                        .expect("bucket_bounds length matches bucket_deltas length");
                    let attributes = [KeyValue::new(LE_ATTRIBUTE, Arc::<str>::clone(le_value))];
                    bucket_counter.add(delta, &attributes);
                }
            }
        }
    }
}

/// Formats a bucket bound for the `le` attribute.
///
/// `Magnitude::MAX` is formatted as `+Inf` following Prometheus conventions.
fn format_bucket_bound(magnitude: Magnitude) -> Arc<str> {
    if magnitude == Magnitude::MAX {
        Arc::from("+Inf")
    } else {
        Arc::from(magnitude.to_string())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use nm::{EventMetrics, Histogram};
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    fn create_test_provider() -> (SdkMeterProvider, InMemoryMetricExporter) {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone()).build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        (provider, exporter)
    }

    #[test]
    fn format_bucket_bound_regular_values() {
        assert_eq!(format_bucket_bound(10).as_ref(), "10");
        assert_eq!(format_bucket_bound(0).as_ref(), "0");
        assert_eq!(format_bucket_bound(-5).as_ref(), "-5");
        assert_eq!(format_bucket_bound(1000).as_ref(), "1000");
    }

    #[test]
    fn format_bucket_bound_max_is_plus_inf() {
        assert_eq!(format_bucket_bound(Magnitude::MAX).as_ref(), "+Inf");
    }

    // OpenTelemetry SDK uses system time calls not available under Miri isolation.
    #[cfg_attr(miri, ignore)]
    #[test]
    fn export_report_simple_event() {
        let (provider, exporter) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        let event = EventMetrics::fake("test_event", 100, 5000, None);
        let report = Report::fake(vec![event]);

        export_report(&report, &mut state, &mut instruments);

        // Force flush to ensure metrics are exported.
        provider.force_flush().unwrap();

        let metrics = exporter.get_finished_metrics().unwrap();
        assert!(!metrics.is_empty());
    }

    // OpenTelemetry SDK uses system time calls not available under Miri isolation.
    #[cfg_attr(miri, ignore)]
    #[test]
    fn export_report_event_with_histogram() {
        static BUCKETS: &[Magnitude] = &[10, 50, 100, 500];

        let (provider, exporter) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        let histogram = Histogram::fake(BUCKETS, vec![5, 12, 8, 3], 2);
        let event = EventMetrics::fake("latency_ms", 30, 4567, Some(histogram));
        let report = Report::fake(vec![event]);

        export_report(&report, &mut state, &mut instruments);

        provider.force_flush().unwrap();

        let metrics = exporter.get_finished_metrics().unwrap();
        assert!(!metrics.is_empty());
    }

    // OpenTelemetry SDK uses system time calls not available under Miri isolation.
    #[cfg_attr(miri, ignore)]
    #[test]
    fn export_report_delta_computation() {
        static BUCKETS: &[Magnitude] = &[10, 50];

        let (provider, _exporter) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        // First collection.
        let histogram1 = Histogram::fake(BUCKETS, vec![5, 10], 2);
        let event1 = EventMetrics::fake("test_event", 17, 100, Some(histogram1));
        let report1 = Report::fake(vec![event1]);

        export_report(&report1, &mut state, &mut instruments);

        // Verify state was updated.
        let event_state = state.event_state(&"test_event".into());
        assert_eq!(event_state.count, 17);
        assert_eq!(event_state.histogram_buckets, vec![5, 15, 17]); // Cumulative: 5, 15, 17.

        // Second collection with more data.
        let histogram2 = Histogram::fake(BUCKETS, vec![8, 15], 4);
        let event2 = EventMetrics::fake("test_event", 27, 200, Some(histogram2));
        let report2 = Report::fake(vec![event2]);

        export_report(&report2, &mut state, &mut instruments);

        // Verify state was updated again.
        let event_state = state.event_state(&"test_event".into());
        assert_eq!(event_state.count, 27);
        assert_eq!(event_state.histogram_buckets, vec![8, 23, 27]); // Cumulative: 8, 23, 27.
    }

    // OpenTelemetry SDK uses system time calls not available under Miri isolation.
    #[cfg_attr(miri, ignore)]
    #[test]
    fn export_report_multiple_events() {
        let (provider, exporter) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        let event1 = EventMetrics::fake("event_a", 10, 100, None);
        let event2 = EventMetrics::fake("event_b", 20, 200, None);
        let report = Report::fake(vec![event1, event2]);

        export_report(&report, &mut state, &mut instruments);

        provider.force_flush().unwrap();

        let metrics = exporter.get_finished_metrics().unwrap();
        assert!(!metrics.is_empty());
    }

    // OpenTelemetry SDK uses system time calls not available under Miri isolation.
    #[cfg_attr(miri, ignore)]
    #[test]
    fn export_report_zero_count_delta_does_not_add_to_counter() {
        static BUCKETS: &[Magnitude] = &[10, 50];

        let (provider, _exporter) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        // First collection establishes baseline.
        let histogram1 = Histogram::fake(BUCKETS, vec![5, 10], 2);
        let event1 = EventMetrics::fake("test_event", 100, 500, Some(histogram1));
        let report1 = Report::fake(vec![event1]);
        export_report(&report1, &mut state, &mut instruments);

        // Second collection with same count - zero delta.
        let histogram2 = Histogram::fake(BUCKETS, vec![5, 10], 2);
        let event2 = EventMetrics::fake("test_event", 100, 500, Some(histogram2));
        let report2 = Report::fake(vec![event2]);

        // This should complete without error even when delta is zero.
        export_report(&report2, &mut state, &mut instruments);

        // Verify the state shows zero delta was computed.
        let event_state = state.event_state(&"test_event".into());
        assert_eq!(event_state.count, 100);
    }

    // OpenTelemetry SDK uses system time calls not available under Miri isolation.
    #[cfg_attr(miri, ignore)]
    #[test]
    fn export_report_zero_bucket_delta_does_not_add_to_counter() {
        static BUCKETS: &[Magnitude] = &[10, 50];

        let (provider, _exporter) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        // First collection establishes baseline.
        let histogram1 = Histogram::fake(BUCKETS, vec![5, 10], 2);
        let event1 = EventMetrics::fake("test_event", 10, 100, Some(histogram1));
        let report1 = Report::fake(vec![event1]);
        export_report(&report1, &mut state, &mut instruments);

        // Second collection with same histogram bucket counts - zero bucket deltas.
        let histogram2 = Histogram::fake(BUCKETS, vec![5, 10], 2);
        let event2 = EventMetrics::fake("test_event", 10, 100, Some(histogram2));
        let report2 = Report::fake(vec![event2]);

        // This should complete without error even when bucket deltas are zero.
        export_report(&report2, &mut state, &mut instruments);

        // Verify histogram state is unchanged (same cumulative values).
        let event_state = state.event_state(&"test_event".into());
        assert_eq!(event_state.histogram_buckets, vec![5, 15, 17]);
    }
}
