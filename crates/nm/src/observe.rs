use std::time::Duration;

use crate::Magnitude;

/// Operations for observing the occurrences of an event.
///
/// This is implemented primarily by [`Event`][1] but also by [`ObservationBatch`][2],
/// thereby providing an abstraction for callers that wish to support both single and batch events.
///
/// [1]: crate::Event
/// [2]: crate::ObservationBatch
pub trait Observe {
    /// Observes an event that has no explicit magnitude.
    ///
    /// By convention, this is represented as a magnitude of 1. We expose a separate
    /// method for this to make it clear that the magnitude has no inherent meaning.
    fn observe_once(&self);

    /// Observes an event with a specific magnitude.
    fn observe(&self, magnitude: Magnitude);

    /// Observes an event with the magnitude being the indicated duration in milliseconds.
    ///
    /// Only the whole number part of the duration is used - fractional milliseconds are ignored.
    /// Values outside the i64 range are not guaranteed to be correctly represented.
    fn observe_millis(&self, duration: Duration);

    /// Observes the duration of a function call, in milliseconds.
    fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R;
}
