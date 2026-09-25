use nm::{EventMetrics, Histogram, Magnitude, Report};
use nm_otel::Publisher;
use nm_otel_impl::create_test_provider;
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use tick::Clock;

// Keep this report separate from events registered by the real-collection scenario.
const FAKE_EVENT_NAME: &str = "fake_event";
// Distinct bounds exercise cumulative conversion and the synthetic overflow bucket.
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

fn find_metric_value(metrics: &ResourceMetrics, name: &str, bucket: Option<&str>) -> Option<u64> {
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
fn run_one_iteration_with_report_publishes_fake_report() {
    const METER_NAME: &str = "custom_meter_name_for_test";

    let (provider, reader) = create_test_provider();

    let mut publisher = Publisher::builder()
        .provider(provider)
        .clock(Clock::new_frozen())
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

    // Cumulative SDK output distinguishes exporting deltas from replaying cumulative reports.
    assert_eq!(find_metric_value(&metrics, FAKE_EVENT_NAME, None), Some(25));

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
