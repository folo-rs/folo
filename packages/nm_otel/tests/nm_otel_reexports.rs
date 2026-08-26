//! Smoke test for the public re-export contract.
//!
//! The explicit exports keep implementation-only maintainer APIs out of the public shell.
//! This test verifies that every advertised type remains reachable through that shell.

use std::panic::{RefUnwindSafe, UnwindSafe};

use nm_otel::{Publisher, PublisherBuilder};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use static_assertions::assert_impl_all;
use tick::Clock;

assert_impl_all!(Publisher: Send, UnwindSafe, RefUnwindSafe);
assert_impl_all!(PublisherBuilder: UnwindSafe, RefUnwindSafe);

#[cfg_attr(
    miri,
    ignore = "OpenTelemetry SDK resource detection requires OS metadata unavailable under Miri."
)]
#[test]
fn publisher_builder_reachable_via_re_exports() {
    let meter_provider = SdkMeterProvider::builder().build();

    let builder: PublisherBuilder = Publisher::builder()
        .provider(meter_provider)
        .clock(Clock::new_frozen())
        .meter_name("nm_otel_reexports");

    let _publisher: Publisher = builder.build();
}
