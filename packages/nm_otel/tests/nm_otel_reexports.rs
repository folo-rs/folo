//! Smoke test for the public re-export contract.
//!
//! `nm_otel` is a thin shell over `nm_otel_impl` and exposes a hand-picked subset of
//! items. This test fails to compile if any of those re-exports go missing, which keeps
//! the intended public surface honest as `nm_otel_impl` evolves. It does not exercise
//! behavior beyond what is needed to verify that each type is reachable from outside
//! `nm_otel`.

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::time::Duration;

use nm::Event;
use nm_otel::{Publisher, PublisherBuilder};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use static_assertions::assert_impl_all;
use tick::Clock;

assert_impl_all!(Publisher: UnwindSafe, RefUnwindSafe);
assert_impl_all!(PublisherBuilder: UnwindSafe, RefUnwindSafe);

thread_local! {
    static REEXPORT_TEST_EVENT: Event = Event::builder()
        .name("nm_otel_reexports_event")
        .build();
}

// OpenTelemetry SDK uses system time calls not available under Miri isolation.
#[cfg_attr(miri, ignore)]
#[test]
fn publisher_builder_reachable_via_re_exports() {
    REEXPORT_TEST_EVENT.with(|event| {
        event.observe_once();
    });

    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter).build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();

    let builder: PublisherBuilder = Publisher::builder()
        .provider(provider)
        .clock(Clock::new_frozen())
        .interval(Duration::from_secs(5))
        .meter_name("nm_otel_reexports");

    let _publisher: Publisher = builder.build();
}
