use nm::{EventMetrics, Histogram, Magnitude, Report};
use nm_otel::Publisher;
use nm_otel_impl::{TestMetricReader, create_test_provider};
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use tick::Clock;

// Prometheus-compatible companion names and bucket attributes are part of the export contract.
const SUM_SUFFIX: &str = "_sum";
const BUCKET_SUFFIX: &str = "_bucket";
const LE_ATTRIBUTE: &str = "le";

fn create_publisher() -> (Publisher, TestMetricReader) {
    let (provider, reader) = create_test_provider();
    let publisher = Publisher::builder()
        .provider(provider)
        .clock(Clock::new_frozen())
        .meter_name("test")
        .build();
    (publisher, reader)
}

fn collect_metrics(reader: &TestMetricReader) -> Vec<ResourceMetrics> {
    vec![reader.collect()]
}

/// Looks up a monotonic counter by metric name and optional bucket bound.
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

/// Detects collisions that produce duplicate metric names.
fn metric_count(metrics: &[ResourceMetrics], name: &str) -> usize {
    let mut count = 0_usize;
    for resource_metrics in metrics {
        for scope_metrics in resource_metrics.scope_metrics() {
            for metric in scope_metrics.metrics() {
                if metric.name() == name {
                    count = count.saturating_add(1);
                }
            }
        }
    }
    count
}

#[test]
fn export_report_simple_event() {
    const EVENT_NAME: &str = "test_event";
    const COUNT: u64 = 100;
    const SUM: Magnitude = 5000;

    let (mut publisher, reader) = create_publisher();
    let event = EventMetrics::fake(EVENT_NAME, COUNT, SUM, None);
    let report = Report::fake(vec![event]);

    publisher.run_one_iteration_with_report(&report);

    let metrics = collect_metrics(&reader);

    // Initial publication sends the full count, an absolute sum gauge, and no histogram.
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
fn export_report_event_with_histogram() {
    const EVENT_NAME: &str = "latency_ms";
    const COUNT: u64 = 30;
    const SUM: Magnitude = 4567;
    static BUCKETS: &[Magnitude] = &[10, 50, 100, 500];
    const PLUS_INFINITY_BUCKET_COUNT: u64 = 2;

    let (mut publisher, reader) = create_publisher();
    let histogram = Histogram::fake(BUCKETS, vec![5, 12, 8, 3], PLUS_INFINITY_BUCKET_COUNT);
    let event = EventMetrics::fake(EVENT_NAME, COUNT, SUM, Some(histogram));
    let report = Report::fake(vec![event]);

    publisher.run_one_iteration_with_report(&report);

    let metrics = collect_metrics(&reader);

    assert_eq!(counter_value(&metrics, EVENT_NAME, None), Some(COUNT));
    assert_eq!(
        gauge_value(&metrics, &format!("{EVENT_NAME}{SUM_SUFFIX}")),
        Some(SUM)
    );

    // Initial bucket deltas equal cumulative totals, including the overflow bucket.
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
fn export_report_multiple_events() {
    const FIRST_NAME: &str = "event_a";
    const FIRST_COUNT: u64 = 10;
    const FIRST_SUM: Magnitude = 100;
    const SECOND_NAME: &str = "event_b";
    const SECOND_COUNT: u64 = 20;
    const SECOND_SUM: Magnitude = 200;

    let (mut publisher, reader) = create_publisher();
    let first_event = EventMetrics::fake(FIRST_NAME, FIRST_COUNT, FIRST_SUM, None);
    let second_event = EventMetrics::fake(SECOND_NAME, SECOND_COUNT, SECOND_SUM, None);
    let report = Report::fake(vec![first_event, second_event]);

    publisher.run_one_iteration_with_report(&report);

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
fn export_report_separates_event_named_like_sum_companion() {
    const BASE_EVENT: &str = "latency";
    const BASE_COUNT: u64 = 7;
    const BASE_SUM: Magnitude = 700;
    const COLLIDING_EVENT: &str = "latency_sum";
    const COLLIDING_COUNT: u64 = 3;
    const COLLIDING_SUM: Magnitude = 300;

    let (mut publisher, reader) = create_publisher();
    let report = Report::fake(vec![
        EventMetrics::fake(BASE_EVENT, BASE_COUNT, BASE_SUM, None),
        EventMetrics::fake(COLLIDING_EVENT, COLLIDING_COUNT, COLLIDING_SUM, None),
    ]);

    publisher.run_one_iteration_with_report(&report);

    let metrics = collect_metrics(&reader);

    // The base event keeps its unshifted names, so its sum gauge owns `latency_sum`.
    assert_eq!(counter_value(&metrics, BASE_EVENT, None), Some(BASE_COUNT));
    assert_eq!(gauge_value(&metrics, "latency_sum"), Some(BASE_SUM));

    // The colliding event keeps distinct counter and gauge aggregations.
    assert_eq!(
        counter_value(&metrics, "latency_sum_", None),
        Some(COLLIDING_COUNT)
    );
    assert_eq!(
        gauge_value(&metrics, "latency_sum__sum"),
        Some(COLLIDING_SUM)
    );

    // One instrument per name means the gauge and the counter were never merged.
    for name in ["latency", "latency_sum", "latency_sum_", "latency_sum__sum"] {
        assert_eq!(metric_count(&metrics, name), 1);
    }
}

#[test]
fn export_report_separates_event_named_like_bucket_companion() {
    const BASE_EVENT: &str = "latency";
    const BASE_COUNT: u64 = 9;
    const BASE_SUM: Magnitude = 900;
    static BASE_BUCKETS: &[Magnitude] = &[10, 50];
    const BASE_PLUS_INFINITY_BUCKET_COUNT: u64 = 2;
    const COLLIDING_EVENT: &str = "latency_bucket";
    const COLLIDING_COUNT: u64 = 4;
    const COLLIDING_SUM: Magnitude = 400;

    let (mut publisher, reader) = create_publisher();
    let histogram = Histogram::fake(BASE_BUCKETS, vec![5, 2], BASE_PLUS_INFINITY_BUCKET_COUNT);
    let report = Report::fake(vec![
        EventMetrics::fake(BASE_EVENT, BASE_COUNT, BASE_SUM, Some(histogram)),
        EventMetrics::fake(COLLIDING_EVENT, COLLIDING_COUNT, COLLIDING_SUM, None),
    ]);

    publisher.run_one_iteration_with_report(&report);

    let metrics = collect_metrics(&reader);

    // The base event's bucket counter reports a cumulative series per bound.
    assert_eq!(counter_value(&metrics, BASE_EVENT, None), Some(BASE_COUNT));
    assert_eq!(gauge_value(&metrics, "latency_sum"), Some(BASE_SUM));
    assert_eq!(
        counter_value(&metrics, "latency_bucket", Some("10")),
        Some(5)
    );
    assert_eq!(
        counter_value(&metrics, "latency_bucket", Some("50")),
        Some(7)
    );
    assert_eq!(
        counter_value(&metrics, "latency_bucket", Some("+Inf")),
        Some(BASE_COUNT)
    );

    // An attribute-free count cannot hide inside the bucket counter's attributed series.
    assert_eq!(counter_value(&metrics, "latency_bucket", None), None);

    assert_eq!(
        counter_value(&metrics, "latency_bucket_", None),
        Some(COLLIDING_COUNT)
    );
    assert_eq!(
        gauge_value(&metrics, "latency_bucket__sum"),
        Some(COLLIDING_SUM)
    );

    for name in [
        "latency",
        "latency_sum",
        "latency_bucket",
        "latency_bucket_",
        "latency_bucket__sum",
    ] {
        assert_eq!(metric_count(&metrics, name), 1);
    }
}
