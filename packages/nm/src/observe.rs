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
    ///
    /// # Example
    ///
    /// ```
    /// use nm::Event;
    ///
    /// thread_local! {
    ///     static REQUESTS: Event = Event::builder()
    ///         .name("requests")
    ///         .build();
    /// }
    ///
    /// // Count a single request occurrence
    /// REQUESTS.with(|event| event.observe_once());
    /// ```
    fn observe_once(&self);

    /// Observes an event with a specific magnitude.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::Event;
    ///
    /// thread_local! {
    ///     static SENT_BYTES: Event = Event::builder()
    ///         .name("sent_bytes")
    ///         .build();
    /// }
    ///
    /// // Record sending 1024 bytes
    /// SENT_BYTES.with(|event| event.observe(1024));
    /// ```
    fn observe(&self, magnitude: Magnitude);

    /// Observes an event with the magnitude being the indicated duration in milliseconds.
    ///
    /// Only the whole number part of the duration is used - fractional milliseconds are ignored.
    /// Values outside the i64 range are not guaranteed to be correctly represented.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use nm::Event;
    ///
    /// thread_local! {
    ///     static REQUEST_DURATION: Event = Event::builder()
    ///         .name("request_duration_ms")
    ///         .build();
    /// }
    ///
    /// let duration = Duration::from_millis(250);
    /// REQUEST_DURATION.with(|event| event.observe_millis(duration));
    /// ```
    fn observe_millis(&self, duration: Duration);

    /// Observes the duration of a function call, in milliseconds.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::Event;
    ///
    /// thread_local! {
    ///     static DATABASE_QUERY_TIME: Event = Event::builder()
    ///         .name("database_query_time_ms")
    ///         .build();
    /// }
    ///
    /// fn query_database() -> String {
    /// #     // Simulated database query
    /// #     "result".to_string()
    /// }
    ///
    /// let result =
    ///     DATABASE_QUERY_TIME.with(|event| event.observe_duration_millis(|| query_database()));
    /// ```
    fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R;
}
