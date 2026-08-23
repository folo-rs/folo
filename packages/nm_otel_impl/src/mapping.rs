//! Mapping from nm metrics to OpenTelemetry instruments.

use std::hash::BuildHasher;
use std::slice;
use std::sync::Arc;

use foldhash::fast::FixedState;
use hashbrown::HashTable;
use hashbrown::hash_table::Entry;
use nm::{EventName, Histogram, Magnitude, Report};
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Gauge, Meter};

use crate::state::CollectionState;

/// Implements the Prometheus-compatible name for an event's running-total companion metric.
const SUM_SUFFIX: &str = "_sum";

/// Implements the Prometheus-compatible name for an event's per-bound histogram series.
const BUCKET_SUFFIX: &str = "_bucket";

/// Attribute key for histogram bucket upper bound (Prometheus convention).
const LE_ATTRIBUTE: &str = "le";

/// Manages OpenTelemetry instruments for nm metrics.
///
/// Creates instruments on-demand as events appear in reports and caches them for reuse.
/// All instruments for an event are created together, and string formatting only happens
/// once per event (not on every metric export).
#[derive(Debug)]
pub(crate) struct InstrumentRegistry {
    meter: Meter,

    // See the matching comment on `CollectionState::hasher` in `state.rs` for the full
    // rationale. In short: `hashbrown::HashTable` in place of `std::collections::HashMap`
    // clones the `EventName` key only on insertion (not on every lookup), and the hasher is
    // `foldhash::fast::FixedState` rather than a randomly seeded one so that this
    // export-path map produces stable Callgrind instruction counts across builds. Event
    // names are trusted internal identifiers, so forfeiting HashDoS resistance is safe.
    hasher: FixedState,

    /// Cached instruments per event name.
    events: HashTable<(EventName, EventInstruments)>,
}

impl InstrumentRegistry {
    /// Creates a new instrument registry with the given meter.
    pub(crate) fn new(meter: Meter) -> Self {
        Self {
            meter,
            hasher: FixedState::default(),
            events: HashTable::new(),
        }
    }

    /// Gets or creates instruments for an event.
    ///
    /// If histogram magnitudes are provided, the bucket counter and cached bucket attribute
    /// values are also created. This avoids creating them for events without histogram data.
    fn instruments(
        &mut self,
        event_name: &EventName,
        magnitudes: Option<impl Iterator<Item = Magnitude>>,
    ) -> &EventInstruments {
        let meter = &self.meter;

        let hash = self.hasher.hash_one(event_name);
        let hasher = &self.hasher;
        // See `CollectionState::event_state` for the per-closure breakdown; the same three-
        // closure contract (lookup hash, equality, growth-time rehash) applies here.
        let instruments = match self.events.entry(
            hash,
            |(existing, _)| existing == event_name,
            |(existing, _)| hasher.hash_one(existing),
        ) {
            Entry::Occupied(occupied) => &mut occupied.into_mut().1,
            Entry::Vacant(vacant) => {
                let sum_name = format!("{event_name}{SUM_SUFFIX}");
                let new_instruments = EventInstruments {
                    // OpenTelemetry accepts the name as `Cow<'static, str>`, and `EventName`
                    // already is one, so cloning it avoids the allocation that `to_string()`
                    // would force for a borrowed (`Cow::Borrowed`) name.
                    count_counter: meter.u64_counter(event_name.clone()).build(),
                    sum_gauge: meter.i64_gauge(sum_name).build(),
                    bucket_counter: None,
                    bucket_attrs: Vec::new(),
                };
                // The key is cloned only here, on the vacant branch, i.e. the first time this
                // event name is seen. Every later export takes the occupied branch above and
                // performs no clone.
                &mut vacant
                    .insert((event_name.clone(), new_instruments))
                    .into_mut()
                    .1
            }
        };

        // Lazily create the bucket counter and cache bucket attributes if histogram data is
        // provided. Building the `KeyValue` once per bucket here eliminates per-export
        // `Arc<str>` clone and `KeyValue::new` work in the export loop.
        if let Some(magnitudes) = magnitudes
            && instruments.bucket_counter.is_none()
        {
            let bucket_name = format!("{event_name}{BUCKET_SUFFIX}");
            instruments.bucket_counter = Some(meter.u64_counter(bucket_name).build());
            instruments.bucket_attrs = magnitudes
                .map(|magnitude| KeyValue::new(LE_ATTRIBUTE, format_bucket_bound(magnitude)))
                .collect();
        }

        instruments
    }
}

/// Cached OpenTelemetry instruments for a single nm event.
///
/// All instruments are created together when an event is first seen, avoiding repeated
/// string formatting on every metric lookup. Fields are ordered by how central they are
/// to an event: the always-present count and sum come first, and the optional histogram
/// bucket counter with its precomputed attributes forms a coherent trailing block.
#[derive(Debug)]
struct EventInstruments {
    /// Counter for event count (named after the event itself, e.g. `http_requests`).
    count_counter: Counter<u64>,

    /// Gauge for event sum (named `{event}_sum`).
    sum_gauge: Gauge<i64>,

    /// Counter for histogram buckets (named `{event}_bucket`), if the event has a histogram.
    /// Different bucket bounds are distinguished by the `le` attribute, not by instrument name.
    bucket_counter: Option<Counter<u64>>,

    /// Precomputed `le` attribute for each histogram bucket.
    ///
    /// Indexed by bucket index, matching the order from `histogram.magnitudes()`, so this
    /// vector has exactly one entry per bucket. Building the `KeyValue` once per bucket at
    /// registration time eliminates the per-export `Arc<str>` clone and `KeyValue`
    /// construction that the export hot path would otherwise perform on every call to
    /// `add_bucket_delta`.
    bucket_attrs: Vec<KeyValue>,
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
        let histogram = event.histogram();

        // Get cached instruments for this event (creates them if first time seeing this event).
        // Pass histogram magnitudes if present so bucket counter and bounds can be created.
        let event_instruments =
            instruments.instruments(event_name, histogram.map(Histogram::magnitudes));

        // Export count as counter (delta).
        let count_delta = event_state.count_delta(event.count());
        add_count_delta(&event_instruments.count_counter, count_delta);

        // Export sum as gauge (absolute value).
        event_instruments.sum_gauge.record(event.sum(), &[]);

        // Export histogram buckets if present.
        if let Some(histogram) = histogram {
            let bucket_counter = event_instruments
                .bucket_counter
                .as_ref()
                .expect("supplying histogram magnitudes creates the matching bucket counter");

            // The delta iterator and `bucket_attrs` are both derived from
            // `histogram.magnitudes()`, so they have one element per bucket and pair up
            // exactly. Zipping encodes that equal length structurally instead of indexing
            // with a bounds check. A histogram whose bucket count changed between exports
            // still panics inside `histogram_deltas`, because `zip` pulls the delta iterator
            // (its left operand) first on every step.
            for ((_magnitude, _cumulative, delta), attr) in event_state
                .histogram_deltas(histogram.magnitudes(), histogram.counts())
                .zip(&event_instruments.bucket_attrs)
            {
                add_bucket_delta(bucket_counter, delta, attr);
            }
        }
    }
}

/// Adds a count delta to a counter if positive.
///
/// This is a separate function to allow skipping equivalent mutations - adding 0 to a counter
/// produces the same observable result as not calling add at all.
#[cfg_attr(test, mutants::skip)]
fn add_count_delta(counter: &Counter<u64>, delta: u64) {
    if delta > 0 {
        counter.add(delta, &[]);
    }
}

/// Adds a bucket delta to a counter if positive.
///
/// `attr` is the cached `le` attribute for one bucket, stored in the registry.
/// `slice::from_ref` turns it into the `&[KeyValue]` that `Counter::add` expects without
/// allocating, so no per-call attribute construction happens on the hot path.
///
/// This is a separate function to allow skipping equivalent mutations - adding 0 to a counter
/// produces the same observable result as not calling add at all.
#[cfg_attr(test, mutants::skip)]
fn add_bucket_delta(counter: &Counter<u64>, delta: u64, attr: &KeyValue) {
    if delta > 0 {
        counter.add(delta, slice::from_ref(attr));
    }
}

/// Formats a bucket bound for the `le` attribute.
///
/// The terminal `Magnitude::MAX` bucket is the plus-infinity bucket (the overflow bucket for
/// occurrences that fit no explicit bound); it is formatted as `+Inf` following Prometheus
/// conventions.
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
    use nm::{EventMetrics, Histogram};
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};

    use super::*;
    use crate::{TestMetricReader, create_test_provider};

    fn collect_metrics(reader: &TestMetricReader) -> Vec<ResourceMetrics> {
        vec![reader.collect()]
    }

    /// Looks up a monotonic `u64` counter value by metric name and optional `le` bucket bound.
    ///
    /// Returns `None` when no matching metric or data point exists. Asserts that any metric
    /// found under `name` is a monotonic sum, which is the exported kind for count and bucket
    /// counters.
    fn counter_value(metrics: &[ResourceMetrics], name: &str, le: Option<&str>) -> Option<u64> {
        for resource_metrics in metrics {
            for scope_metrics in resource_metrics.scope_metrics() {
                for metric in scope_metrics.metrics() {
                    if metric.name() != name {
                        continue;
                    }
                    let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                        continue;
                    };
                    assert!(sum.is_monotonic());
                    for point in sum.data_points() {
                        let matches = match le {
                            None => point.attributes().next().is_none(),
                            Some(expected) => point.attributes().any(|kv: &KeyValue| {
                                kv.key.as_str() == LE_ATTRIBUTE && kv.value.as_str() == expected
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

    /// Looks up an `i64` gauge value by metric name, returning `None` when absent.
    ///
    /// Asserts that any metric found under `name` is a gauge, which is the exported kind for
    /// the sum metric.
    fn gauge_value(metrics: &[ResourceMetrics], name: &str) -> Option<i64> {
        for resource_metrics in metrics {
            for scope_metrics in resource_metrics.scope_metrics() {
                for metric in scope_metrics.metrics() {
                    if metric.name() != name {
                        continue;
                    }
                    let AggregatedMetrics::I64(MetricData::Gauge(gauge)) = metric.data() else {
                        continue;
                    };
                    if let Some(point) = gauge.data_points().next() {
                        return Some(point.value());
                    }
                }
            }
        }
        None
    }

    #[test]
    fn format_bucket_bound_regular_values() {
        assert_eq!(format_bucket_bound(10).as_ref(), "10");
        assert_eq!(format_bucket_bound(0).as_ref(), "0");
        assert_eq!(format_bucket_bound(-5).as_ref(), "-5");
        assert_eq!(format_bucket_bound(1000).as_ref(), "1000");
    }

    #[test]
    fn format_bucket_bound_max_is_plus_infinity() {
        assert_eq!(format_bucket_bound(Magnitude::MAX).as_ref(), "+Inf");
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn export_report_simple_event() {
        const EVENT_NAME: &str = "test_event";
        const COUNT: u64 = 100;
        const SUM: Magnitude = 5000;

        let (provider, reader) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        let event = EventMetrics::fake(EVENT_NAME, COUNT, SUM, None);
        let report = Report::fake(vec![event]);

        export_report(&report, &mut state, &mut instruments);

        let metrics = collect_metrics(&reader);

        // The first collection publishes the full count as the counter delta, the sum as an
        // absolute gauge, and no bucket metric because the event has no histogram.
        assert_eq!(counter_value(&metrics, EVENT_NAME, None), Some(COUNT));
        assert_eq!(
            gauge_value(&metrics, &format!("{EVENT_NAME}{SUM_SUFFIX}")),
            Some(SUM)
        );
        assert_eq!(
            counter_value(&metrics, &format!("{EVENT_NAME}{BUCKET_SUFFIX}"), None),
            None
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn export_report_event_with_histogram() {
        const EVENT_NAME: &str = "latency_ms";
        const COUNT: u64 = 30;
        const SUM: Magnitude = 4567;
        static BUCKETS: &[Magnitude] = &[10, 50, 100, 500];
        const PLUS_INFINITY_BUCKET_COUNT: u64 = 2;

        let (provider, reader) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        let histogram = Histogram::fake(BUCKETS, vec![5, 12, 8, 3], PLUS_INFINITY_BUCKET_COUNT);
        let event = EventMetrics::fake(EVENT_NAME, COUNT, SUM, Some(histogram));
        let report = Report::fake(vec![event]);

        export_report(&report, &mut state, &mut instruments);

        let metrics = collect_metrics(&reader);

        assert_eq!(counter_value(&metrics, EVENT_NAME, None), Some(COUNT));
        assert_eq!(
            gauge_value(&metrics, &format!("{EVENT_NAME}{SUM_SUFFIX}")),
            Some(SUM)
        );

        // First-collection bucket deltas equal the cumulative bucket totals, which the bucket
        // counter reports under one `le` series per bound plus the synthetic `+Inf` overflow
        // bucket.
        let bucket_metric = format!("{EVENT_NAME}{BUCKET_SUFFIX}");
        assert_eq!(counter_value(&metrics, &bucket_metric, Some("10")), Some(5));
        assert_eq!(
            counter_value(&metrics, &bucket_metric, Some("50")),
            Some(17)
        );
        assert_eq!(
            counter_value(&metrics, &bucket_metric, Some("100")),
            Some(25)
        );
        assert_eq!(
            counter_value(&metrics, &bucket_metric, Some("500")),
            Some(28)
        );
        assert_eq!(
            counter_value(&metrics, &bucket_metric, Some("+Inf")),
            Some(30)
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn export_report_delta_computation() {
        static BUCKETS: &[Magnitude] = &[10, 50];

        let (provider, _reader) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        // First collection.
        let first_histogram = Histogram::fake(BUCKETS, vec![5, 10], 2);
        let first_event = EventMetrics::fake("test_event", 17, 100, Some(first_histogram));
        let first_report = Report::fake(vec![first_event]);

        export_report(&first_report, &mut state, &mut instruments);

        // Verify state was updated to the cumulative bucket totals.
        let event_state = state.event_state(&"test_event".into());
        assert_eq!(event_state.count, 17);
        assert_eq!(event_state.histogram_buckets, vec![5, 15, 17]);

        // Second collection with more data.
        let second_histogram = Histogram::fake(BUCKETS, vec![8, 15], 4);
        let second_event = EventMetrics::fake("test_event", 27, 200, Some(second_histogram));
        let second_report = Report::fake(vec![second_event]);

        export_report(&second_report, &mut state, &mut instruments);

        // Verify state advanced to the new cumulative totals.
        let event_state = state.event_state(&"test_event".into());
        assert_eq!(event_state.count, 27);
        assert_eq!(event_state.histogram_buckets, vec![8, 23, 27]);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn export_report_multiple_events() {
        const FIRST_NAME: &str = "event_a";
        const FIRST_COUNT: u64 = 10;
        const FIRST_SUM: Magnitude = 100;
        const SECOND_NAME: &str = "event_b";
        const SECOND_COUNT: u64 = 20;
        const SECOND_SUM: Magnitude = 200;

        let (provider, reader) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        let first_event = EventMetrics::fake(FIRST_NAME, FIRST_COUNT, FIRST_SUM, None);
        let second_event = EventMetrics::fake(SECOND_NAME, SECOND_COUNT, SECOND_SUM, None);
        let report = Report::fake(vec![first_event, second_event]);

        export_report(&report, &mut state, &mut instruments);

        let metrics = collect_metrics(&reader);

        // Both events must be exported independently, each with its own count and sum.
        assert_eq!(counter_value(&metrics, FIRST_NAME, None), Some(FIRST_COUNT));
        assert_eq!(
            gauge_value(&metrics, &format!("{FIRST_NAME}{SUM_SUFFIX}")),
            Some(FIRST_SUM)
        );
        assert_eq!(
            counter_value(&metrics, SECOND_NAME, None),
            Some(SECOND_COUNT)
        );
        assert_eq!(
            gauge_value(&metrics, &format!("{SECOND_NAME}{SUM_SUFFIX}")),
            Some(SECOND_SUM)
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn instrument_registry_preserves_entries_across_table_growth() {
        // The underlying `HashTable` grows when capacity is exceeded, which calls the
        // rehash closure passed to `entry()` on every existing entry. This test inserts
        // enough events to force multiple grows and then re-exports the same events to
        // verify that lookups still find the existing entries — otherwise entries would
        // land in the wrong buckets after a grow and the second export would create
        // duplicate `EventInstruments`, leaving the table with more than `NUM_EVENTS`
        // entries.
        //
        // A freshly constructed `HashTable` starts with no heap capacity, so this count
        // is chosen to be well beyond the first allocated capacity and therefore to
        // trigger several successive grow-and-rehash cycles rather than just one.
        const NUM_EVENTS: u64 = 64;

        let (provider, _reader) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        // First pass: register every event, forcing the `HashTable` to grow.
        let events: Vec<_> = (0..NUM_EVENTS)
            .map(|i| EventMetrics::fake(format!("growth_event_{i}"), i.saturating_add(1), 0, None))
            .collect();
        let report = Report::fake(events);
        export_report(&report, &mut state, &mut instruments);

        let expected_len = usize::try_from(NUM_EVENTS).unwrap();
        assert_eq!(instruments.events.len(), expected_len);

        // Second pass with the same events: every lookup must hit the existing entry.
        let second_events: Vec<_> = (0..NUM_EVENTS)
            .map(|i| {
                EventMetrics::fake(
                    format!("growth_event_{i}"),
                    i.saturating_add(1).saturating_mul(2),
                    0,
                    None,
                )
            })
            .collect();
        let second_report = Report::fake(second_events);
        export_report(&second_report, &mut state, &mut instruments);

        // If the rehash closure produced inconsistent hashes for any existing entry,
        // the second export would have inserted a duplicate instead of reusing it.
        assert_eq!(instruments.events.len(), expected_len);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn export_report_zero_count_delta_does_not_add_to_counter() {
        static BUCKETS: &[Magnitude] = &[10, 50];

        let (provider, _reader) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        // First collection establishes the baseline.
        let baseline_histogram = Histogram::fake(BUCKETS, vec![5, 10], 2);
        let baseline_event = EventMetrics::fake("test_event", 100, 500, Some(baseline_histogram));
        let baseline_report = Report::fake(vec![baseline_event]);
        export_report(&baseline_report, &mut state, &mut instruments);

        // Second collection repeats the same count, so the count delta is zero.
        let repeat_histogram = Histogram::fake(BUCKETS, vec![5, 10], 2);
        let repeat_event = EventMetrics::fake("test_event", 100, 500, Some(repeat_histogram));
        let repeat_report = Report::fake(vec![repeat_event]);

        export_report(&repeat_report, &mut state, &mut instruments);

        // Verify the state shows the unchanged cumulative count.
        let event_state = state.event_state(&"test_event".into());
        assert_eq!(event_state.count, 100);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
    )]
    fn export_report_zero_bucket_delta_does_not_add_to_counter() {
        static BUCKETS: &[Magnitude] = &[10, 50];

        let (provider, _reader) = create_test_provider();
        let meter = provider.meter("test");

        let mut state = CollectionState::new();
        let mut instruments = InstrumentRegistry::new(meter);

        // First collection establishes the baseline.
        let baseline_histogram = Histogram::fake(BUCKETS, vec![5, 10], 2);
        let baseline_event = EventMetrics::fake("test_event", 10, 100, Some(baseline_histogram));
        let baseline_report = Report::fake(vec![baseline_event]);
        export_report(&baseline_report, &mut state, &mut instruments);

        // Second collection repeats the same bucket counts, so every bucket delta is zero.
        let repeat_histogram = Histogram::fake(BUCKETS, vec![5, 10], 2);
        let repeat_event = EventMetrics::fake("test_event", 10, 100, Some(repeat_histogram));
        let repeat_report = Report::fake(vec![repeat_event]);

        export_report(&repeat_report, &mut state, &mut instruments);

        // Verify the histogram state is unchanged (same cumulative values).
        let event_state = state.event_state(&"test_event".into());
        assert_eq!(event_state.histogram_buckets, vec![5, 15, 17]);
    }
}
