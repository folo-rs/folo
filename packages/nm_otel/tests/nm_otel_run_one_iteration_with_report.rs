//! Integration test for `Publisher::run_one_iteration_with_report()`.
//!
//! Exercises the `test-util`-gated entry point that drives the export pipeline using a
//! synthetic [`nm::Report`] instead of [`nm::Report::collect`]. This isolates the
//! Publisher's behavior from the global nm registry so deltas across iterations can be
//! verified with exact, fabricated inputs.

use nm::{EventMetrics, Histogram, Magnitude, Report};
use nm_otel::Publisher;
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use tick::Clock;

const EVENT_NAME: &str = "fake_event";
const HISTOGRAM_MAGNITUDES: &[Magnitude] = &[10, 50, 100];

fn create_test_provider() -> (SdkMeterProvider, InMemoryMetricExporter) {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone()).build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();
    (provider, exporter)
}

fn make_report(count: u64, sum: Magnitude, bucket_counts: Vec<u64>, plus_inf: u64) -> Report {
    let histogram = Histogram::fake(HISTOGRAM_MAGNITUDES, bucket_counts, plus_inf);
    let event = EventMetrics::fake(EVENT_NAME, count, sum, Some(histogram));
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

// OpenTelemetry SDK uses system time calls not available under Miri isolation.
#[cfg_attr(miri, ignore)]
#[test]
fn run_one_iteration_with_report_exports_fake_report() {
    const METER_NAME: &str = "custom_meter_name_for_test";

    let (provider, exporter) = create_test_provider();

    let mut publisher = Publisher::builder()
        .provider(provider.clone())
        .clock(Clock::new_frozen())
        .meter_name(METER_NAME)
        .build();

    // First iteration: cumulative buckets are [4, 7, 9] from non-cumulative [4, 3, 2],
    // plus 1 in the `+Inf` overflow bucket.
    let report1 = make_report(10, 4567, vec![4, 3, 2], 1);
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

    assert_eq!(find_metric_value(&exporter, EVENT_NAME, None), Some(10));
    exporter.reset();

    // Second iteration: non-cumulative becomes [6, 5, 3], cumulative [6, 11, 14],
    // so per-bucket deltas vs the first iteration are [2, 4, 5]. Counter delta:
    // 25 - 10 = 15.
    let report2 = make_report(25, 8901, vec![6, 5, 3], 2);
    publisher.run_one_iteration_with_report(&report2);
    provider.force_flush().unwrap();

    // OTel counters accumulate across flushes. After iter 2 the exported cumulative is
    // 10 + 15 = 25. This confirms the publisher shipped a *delta* of 15 (not the raw
    // cumulative of 25, which would yield 10 + 25 = 35).
    assert_eq!(find_metric_value(&exporter, EVENT_NAME, None), Some(25));

    // Histogram bucket counters likewise accumulate. After iter 1 their cumulative
    // values are [4, 7, 9, 10]; iter 2 deltas are [2, 4, 5, 6]; so post-iter-2
    // cumulatives are [6, 11, 14, 16]. Any of these matching the expected sum-of-deltas
    // confirms that histogram_deltas is computing correctly across iterations.
    let bucket_metric = format!("{EVENT_NAME}_bucket");
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
