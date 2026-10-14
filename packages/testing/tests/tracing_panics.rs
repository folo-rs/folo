//! Narrow panic assertions prove that enabled logging reaches expressions and formatters.

use std::fmt;

use testing::{TracingCapture, assert_panics_with};
use tracing::trace;

testing::set_allocator!();

/// Formatting failures must propagate without publishing partial events or poisoning later capture.
#[test]
fn logging_panics_propagate_without_poisoning_capture() {
    let capture = TracingCapture::start();

    assert_panics_with(
        || trace!(value = panicking_field()),
        |message| assert!(message.contains("field canary")),
    );
    assert_panics_with(
        || trace!(incomplete = "failed-event-canary", value = ?PanickingFormat),
        |message| assert!(message.contains("debug canary")),
    );
    assert_panics_with(
        || trace!(value = %PanickingFormat),
        |message| assert!(message.contains("display canary")),
    );
    assert_panics_with(
        || trace!("message {}", PanickingFormat),
        |message| assert!(message.contains("display canary")),
    );

    assert!(capture.snapshot().is_empty());
    trace!("capture survives formatting panics");
    let entries = capture.snapshot();
    assert_eq!(entries.len(), 1);
    let entry = entries.first().unwrap();
    assert!(entry.contains("capture survives"));
    assert!(!entry.contains("failed-event-canary"));
}

/// A failure in eager field evaluation, distinct from deferred formatting.
fn panicking_field() -> &'static str {
    panic!("field canary");
}

/// A value whose formatters expose subscribers that never evaluate lazy fields.
struct PanickingFormat;

impl fmt::Debug for PanickingFormat {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("debug canary");
    }
}

impl fmt::Display for PanickingFormat {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("display canary");
    }
}
