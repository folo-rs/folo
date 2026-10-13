//! A foreign subscriber needs its own process, separate from successful capture.

use testing::{TracingCapture, assert_panics};
use tracing::subscriber::{NoSubscriber, set_global_default};

testing::set_allocator!();

#[test]
fn rejects_an_existing_global_subscriber() {
    set_global_default(NoSubscriber::default()).unwrap();
    assert_panics(TracingCapture::start);
}
