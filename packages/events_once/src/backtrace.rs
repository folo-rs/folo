#![cfg(debug_assertions)]
//! Diagnostic capture of awaiter backtraces.
//!
//! Every consumer of this module is gated on `cfg(debug_assertions)`, so the module itself only
//! exists in debug builds and does not need a release-build representation.

use std::backtrace::Backtrace;
use std::sync::Arc;

/// A captured backtrace is a shared owner so that it can be snapshotted out of an event without
/// keeping the event's lock held while the snapshot is used. The snapshot also outlives the event
/// it came from, which is what allows an event to be released while a snapshot is in flight.
///
/// Every capture gets its own allocation even when backtrace capture is disabled, which costs one
/// heap allocation per awaited event. That is deliberate: it is confined to debug builds, and it
/// keeps each event's diagnostic state independently owned, so both the reference-count assertions
/// in the release tests and Miri's leak checker can see an event failing to release it.
pub(crate) type BacktraceType = Arc<Backtrace>;

/// Captures a backtrace if `RUST_BACKTRACE=1` is set.
pub(crate) fn capture_backtrace() -> BacktraceType {
    Arc::new(Backtrace::capture())
}
