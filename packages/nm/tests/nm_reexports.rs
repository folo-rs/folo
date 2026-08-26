//! Smoke test for the public re-export contract.
//!
//! `nm` is a thin shell over `nm_impl` and exposes an explicit subset of items.
//! Compilation verifies that each type is reachable from outside `nm`; the trait assertions
//! preserve the intended type-level contracts.

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::time::Duration;

use nm::{
    Event, EventBuilder, EventMetrics, EventName, Histogram, Magnitude, MetricsPusher,
    ObservationBatch, Observe, PublishModel, Pull, Push, Report,
};
use static_assertions::{assert_impl_all, assert_not_impl_any};

/// Boundaries cover low, intermediate, and high fixture magnitudes.
const BUCKETS: &[Magnitude] = &[0, 10, 100];

/// A nontrivial magnitude exercises the generic observation conversion.
const SAMPLE_MAGNITUDE: Magnitude = 42;

/// A nonzero duration exercises millisecond observation.
const SAMPLE_DURATION: Duration = Duration::from_millis(7);

/// Multiple observations exercise the batch specialization.
const SAMPLE_BATCH_SIZE: usize = 3;

/// A separate magnitude makes the report scenario independent from the observation scenario.
const REPORT_MAGNITUDE: Magnitude = 100;

assert_impl_all!(Magnitude: Send, Sync, UnwindSafe, RefUnwindSafe);
assert_impl_all!(EventName: Send, Sync, UnwindSafe, RefUnwindSafe);
assert_impl_all!(Pull: PublishModel, Send, Sync, UnwindSafe, RefUnwindSafe);
assert_impl_all!(Push: PublishModel, UnwindSafe, RefUnwindSafe);
assert_not_impl_any!(Push: Send, Sync);
assert_impl_all!(EventBuilder<Pull>: UnwindSafe, RefUnwindSafe);
assert_impl_all!(EventBuilder<Push>: UnwindSafe, RefUnwindSafe);
assert_not_impl_any!(EventBuilder<Pull>: Send, Sync);
assert_not_impl_any!(EventBuilder<Push>: Send, Sync);
assert_impl_all!(Event<Pull>: Observe, UnwindSafe, RefUnwindSafe);
assert_impl_all!(Event<Push>: Observe, UnwindSafe, RefUnwindSafe);
assert_not_impl_any!(Event<Pull>: Send, Sync);
assert_not_impl_any!(Event<Push>: Send, Sync);
assert_impl_all!(ObservationBatch<'static, Pull>: Observe, UnwindSafe, RefUnwindSafe);
assert_impl_all!(ObservationBatch<'static, Push>: Observe, UnwindSafe, RefUnwindSafe);
assert_not_impl_any!(ObservationBatch<'static, Pull>: Send, Sync);
assert_not_impl_any!(ObservationBatch<'static, Push>: Send, Sync);
assert_impl_all!(MetricsPusher: UnwindSafe, RefUnwindSafe);
assert_not_impl_any!(MetricsPusher: Send, Sync);
assert_impl_all!(Report: Send, Sync, UnwindSafe, RefUnwindSafe);
assert_impl_all!(EventMetrics: Send, Sync, UnwindSafe, RefUnwindSafe);
assert_impl_all!(Histogram: Send, Sync, UnwindSafe, RefUnwindSafe);

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
        event.observe(SAMPLE_MAGNITUDE);
        event.observe_once();
        event.observe_millis(SAMPLE_DURATION);

        require_observation_batch(event.batch(SAMPLE_BATCH_SIZE));
        event.batch(SAMPLE_BATCH_SIZE).observe(SAMPLE_MAGNITUDE);
    });
}

#[test]
fn push_event_pushes_via_re_exports() {
    REEXPORT_TEST_EVENT_PUSH.with(Event::observe_once);

    REEXPORT_TEST_PUSHER.with(MetricsPusher::push);
}

#[test]
fn report_exposes_event_metrics_via_re_exports() {
    REEXPORT_TEST_EVENT_PULL.with(observe_report_fixture);

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

fn observe_report_fixture(event: &Event<Pull>) {
    event.observe(REPORT_MAGNITUDE);
}
