//! Integration test for one explicitly requested `Publisher` collection.
//!
//! A separate binary prevents other tests from recording nm events, allowing exact assertions
//! about the exported metrics.

mod common;

use common::{TestMetricReader, find_u64_sum};
use nm::Event;
use nm_otel::Publisher;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use tick::Clock;

// A test-specific name lets the assertion distinguish this event from registry noise.
const EVENT_NAME: &str = "integration_test_event";
// The batch distinguishes batched observation from a single-event observation.
const OBSERVED_EVENT_COUNT: usize = 10;
// A nonzero magnitude exercises the histogram path while the count metric is asserted.
const OBSERVED_MAGNITUDE: i64 = 100;

thread_local! {
    static TEST_EVENT: Event = Event::builder()
        .name(EVENT_NAME)
        .build();
}

fn create_test_provider() -> (SdkMeterProvider, TestMetricReader) {
    let reader = TestMetricReader::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(reader.clone())
        .build();
    (provider, reader)
}

#[test]
#[cfg_attr(
    miri,
    ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
)]
fn run_one_iteration_exports_recorded_events() {
    TEST_EVENT.with(|event| {
        event
            .batch(OBSERVED_EVENT_COUNT)
            .observe(OBSERVED_MAGNITUDE);
    });

    let (provider, reader) = create_test_provider();

    let mut publisher = Publisher::builder()
        .provider(provider.clone())
        .clock(Clock::new_frozen())
        .build();

    publisher.run_one_iteration();
    let metrics = reader.collect();

    assert_eq!(
        find_u64_sum(&metrics, EVENT_NAME),
        Some((true, u64::try_from(OBSERVED_EVENT_COUNT).unwrap()))
    );
    drop(provider);
}
