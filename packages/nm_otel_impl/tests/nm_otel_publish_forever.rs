//! Integration test for `Publisher::publish_forever()`.
//!
//! A separate binary prevents other tests from recording nm events, allowing exact assertions
//! about the exported metrics.

use std::task::{Context, Waker};
use std::time::Duration;

use nm::Event;
use nm_otel::Publisher;
use nm_otel_impl::{create_test_provider, find_u64_sum};
use testing::with_watchdog;
use tick::ClockControl;

// A test-specific name lets the assertion distinguish this event from registry noise.
const EVENT_NAME: &str = "publish_forever_test_event";
// The interval is arbitrary because the fake clock advances directly to each deadline.
const INTERVAL: Duration = Duration::from_secs(5);
// A batch ensures that the timer-triggered collection exports a nontrivial counter value.
const OBSERVED_EVENT_COUNT: usize = 10;
// A nonzero magnitude exercises the histogram path while the count metric is asserted.
const OBSERVED_MAGNITUDE: i64 = 100;

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
fn publish_forever_collects_metrics_on_timer_tick() {
    with_watchdog(|| {
        TEST_EVENT.with(|event| {
            event
                .batch(OBSERVED_EVENT_COUNT)
                .observe(OBSERVED_MAGNITUDE);
        });

        let (provider, reader) = create_test_provider();
        let control = ClockControl::new();

        let mut publisher = Publisher::builder()
            .provider(provider.clone())
            .clock(control.to_clock())
            .interval(INTERVAL)
            .build();
        let mut future = Box::pin(publisher.publish_forever());
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut context).is_pending());
        control.advance(INTERVAL);
        assert!(future.as_mut().poll(&mut context).is_pending());

        let metrics = reader.collect();
        assert_eq!(
            find_u64_sum(&metrics, EVENT_NAME),
            Some((true, u64::try_from(OBSERVED_EVENT_COUNT).unwrap()))
        );
        drop(provider);
    });
}
