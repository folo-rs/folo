//! A foreign subscriber needs its own process, separate from successful capture.

use testing::{TracingCapture, assert_panics, initialize_tracing};
use tracing::subscriber::{NoSubscriber, set_global_default};

testing::set_allocator!();

/// Neither helper may silently accept a foreign subscriber that could suppress coverage.
#[test]
fn rejects_an_existing_global_subscriber() {
    set_global_default(NoSubscriber::default()).unwrap();
    assert_panics(TracingCapture::start);
    assert_panics(initialize_tracing);
}
