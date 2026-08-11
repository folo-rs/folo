#![allow(
    dead_code,
    reason = "conditional compilation can leave these unused in some cases"
)]

use std::backtrace::Backtrace;
#[cfg(not(debug_assertions))]
use std::marker::PhantomData;
#[cfg(debug_assertions)]
use std::sync::Arc;

/// A captured backtrace is a shared owner so that it can be snapshotted out of an event without
/// keeping the event's lock held while the snapshot is used. The snapshot also outlives the event
/// it came from, which is what allows an event to be released while a snapshot is in flight.
///
/// Every capture gets its own allocation even when backtrace capture is disabled, which costs one
/// heap allocation per awaited event. That is deliberate: it is confined to debug builds, and it
/// keeps each event's diagnostic state independently owned, so both the reference-count assertions
/// in the release tests and Miri's leak checker can see an event failing to release it.
#[cfg(debug_assertions)]
pub(crate) type BacktraceType = Arc<Backtrace>;
#[cfg(not(debug_assertions))]
pub(crate) type BacktraceType = PhantomData<Backtrace>;

/// Captures a backtrace if both:
///
/// 1. `RUST_BACKTRACE=1` is set.
/// 2. `cfg(debug_assertions)` is enabled (e.g. you are using the default `dev` Cargo profile).
pub(crate) fn capture_backtrace() -> BacktraceType {
    #[cfg(debug_assertions)]
    {
        Arc::new(Backtrace::capture())
    }
    #[cfg(not(debug_assertions))]
    {
        PhantomData
    }
}
