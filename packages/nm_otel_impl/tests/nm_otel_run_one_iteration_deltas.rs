//! Integration test for delta computation across explicitly requested collections.
//!
//! A separate binary prevents other tests from recording nm events, allowing exact assertions
//! about the exported metrics.

use nm::Event;
use nm_otel::Publisher;
use nm_otel_impl::{create_test_provider, find_u64_sum};
use tick::Clock;

// A test-specific name lets the assertion distinguish this event from registry noise.
const EVENT_NAME: &str = "delta_test_event";
// Distinct batches make the second assertion sensitive to lost publisher state.
const INITIAL_EVENT_COUNT: usize = 5;
const ADDITIONAL_EVENT_COUNT: usize = 3;
const CUMULATIVE_EVENT_COUNT: usize = INITIAL_EVENT_COUNT + ADDITIONAL_EVENT_COUNT;
// Distinct nonzero magnitudes ensure both collections exercise histogram export.
const INITIAL_MAGNITUDE: i64 = 50;
const ADDITIONAL_MAGNITUDE: i64 = 30;

thread_local! {
    static TEST_EVENT: Event = Event::builder()
        .name(EVENT_NAME)
        .build();
}

#[test]
#[cfg_attr(
    miri,
    ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
)]
fn run_one_iteration_computes_deltas_across_collections() {
    TEST_EVENT.with(|event| {
        event.batch(INITIAL_EVENT_COUNT).observe(INITIAL_MAGNITUDE);
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
        Some((true, u64::try_from(INITIAL_EVENT_COUNT).unwrap()))
    );

    TEST_EVENT.with(|event| {
        event
            .batch(ADDITIONAL_EVENT_COUNT)
            .observe(ADDITIONAL_MAGNITUDE);
    });

    publisher.run_one_iteration();
    let metrics = reader.collect();
    assert_eq!(
        find_u64_sum(&metrics, EVENT_NAME),
        Some((true, u64::try_from(CUMULATIVE_EVENT_COUNT).unwrap()))
    );
    drop(provider);
}
