//! Smoke test for the public re-export contract.
//!
//! `nm` is a thin shell over `nm_impl` and exposes a hand-picked subset of items.
//! This test fails to compile if any of those re-exports go missing, which keeps the
//! intended public surface honest as `nm_impl` evolves. It does not exercise behavior
//! beyond what is needed to verify that each type is reachable from outside `nm`.

use std::time::Duration;

use nm::{
    Event, EventBuilder, EventMetrics, EventName, Histogram, Magnitude, MetricsPusher,
    ObservationBatch, Observe, PublishModel, Pull, Push, Report,
};
use static_assertions::assert_impl_all;

const BUCKETS: &[Magnitude] = &[0, 10, 100];

assert_impl_all!(Pull: PublishModel);
assert_impl_all!(Push: PublishModel);
assert_impl_all!(Event<Pull>: Observe);
assert_impl_all!(Event<Push>: Observe);

thread_local! {
    static REEXPORT_TEST_PUSHER: MetricsPusher = MetricsPusher::new();

    static REEXPORT_TEST_EVENT_PULL: Event<Pull> = Event::builder()
        .name("nm_reexports_pull")
        .histogram(BUCKETS)
        .build();

    static REEXPORT_TEST_EVENT_PUSH: Event<Push> = Event::builder()
        .name("nm_reexports_push")
        .pusher_local(&REEXPORT_TEST_PUSHER)
        .build();
}

fn make_builder() -> EventBuilder<Pull> {
    Event::builder()
}

fn require_event_name(_: &EventName) {}
fn require_count(_: u64) {}
fn require_magnitude(_: Magnitude) {}
fn require_histogram(_: Option<&Histogram>) {}
fn require_event_metrics(_: &EventMetrics) {}
fn require_observation_batch(_: ObservationBatch<'_, Pull>) {}

#[test]
fn pull_event_observes_via_re_exports() {
    let builder: EventBuilder<Pull> = make_builder();
    drop(builder);

    REEXPORT_TEST_EVENT_PULL.with(|event| {
        event.observe(42);
        event.observe_once();
        event.observe_millis(Duration::from_millis(7));

        require_observation_batch(event.batch(3));
        event.batch(3).observe(1);
    });
}

#[test]
fn push_event_pushes_via_re_exports() {
    REEXPORT_TEST_EVENT_PUSH.with(|event| {
        event.observe_once();
    });

    REEXPORT_TEST_PUSHER.with(MetricsPusher::push);
}

#[test]
fn report_exposes_event_metrics_via_re_exports() {
    REEXPORT_TEST_EVENT_PULL.with(|event| event.observe(100));

    let report: Report = Report::collect();

    for metrics in report.events() {
        require_event_metrics(metrics);
        require_event_name(metrics.name());
        require_count(metrics.count());
        require_magnitude(metrics.sum());
        require_magnitude(metrics.mean());
        require_histogram(metrics.histogram());
    }
}
