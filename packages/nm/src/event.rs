use std::marker::PhantomData;
use std::time::{Duration, Instant};

use num_traits::AsPrimitive;

use crate::{EventBuilder, Magnitude, Observe, PublishModel, Pull};

/// Allows you to observe the occurrences of an event in your code.
///
/// The typical pattern is to observe events via thread-local static variables.
///
/// # Publishing models
///
/// The ultimate goal of the metrics collected by an [`Event`] is to end up in a [`Report`][1].
/// There are two models by which this can happen:
///
/// - **Pull** model - the reporting system queries each event in the process for its latest data
///   set when generating a report. This is the default and requires no action from you.
/// - **Push** model - data from an event only flows to a thread-local [`MetricsPusher`][2], which
///   publishes the data into the reporting system on demand. This requires you to periodically
///   trigger the publishing via [`MetricsPusher::push()`][3].
///
/// The push model has lower overhead but requires action from you to ensure that data is published.
/// You may consider using it under controlled conditions, such as when you are certain that every
/// thread that will be reporting data will also call the pusher at some point.
///
/// The choice of publishing model can be made separately for each event.
///
/// # Example (pull model)
///
/// ```
/// use nm::Event;
///
/// thread_local! {
///     static CONNECT_TIME_MS: Event = Event::builder()
///         .name("net_http_connect_time_ms")
///         .build();
/// }
///
/// pub fn http_connect() {
///     CONNECT_TIME_MS.with(|e| {
///         e.observe_duration_millis(|| {
///             do_http_connect();
///         })
///     });
/// }
/// # http_connect();
/// # fn do_http_connect() {}
/// ```
///
/// # Example (push model)
///
/// ```
/// use nm::{Event, MetricsPusher, Push};
///
/// thread_local! {
///     static HTTP_EVENTS_PUSHER: MetricsPusher = MetricsPusher::new();
///
///     static CONNECT_TIME_MS: Event<Push> = Event::builder()
///         .name("net_http_connect_time_ms")
///         .pusher_local(&HTTP_EVENTS_PUSHER)
///         .build();
/// }
///
/// pub fn http_connect() {
///     CONNECT_TIME_MS.with(|e| {
///         e.observe_duration_millis(|| {
///             do_http_connect();
///         })
///     });
/// }
///
/// loop {
///     http_connect();
///
///     // Periodically push the data to the reporting system.
///     if is_time_to_push() {
///         HTTP_EVENTS_PUSHER.with(MetricsPusher::push);
///     }
///     # break; // Avoid infinite loop when running example.
/// }
/// # fn do_http_connect() {}
/// # fn is_time_to_push() -> bool { true }
/// ```
///
/// # Thread safety
///
/// This type is single-threaded. You would typically create instances in a
/// `thread_local!` block, so each thread gets its own instance.
///
/// [1]: crate::Report
/// [2]: crate::MetricsPusher
/// [3]: crate::MetricsPusher::push
#[derive(Debug)]
pub struct Event<P = Pull>
where
    P: PublishModel,
{
    publish_model: P,

    _single_threaded: PhantomData<*const ()>,
}

impl Event<Pull> {
    /// Creates a new event builder with the default builder configuration.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Gets replaced with itself by different name, bad mutation.
    pub fn builder() -> EventBuilder<Pull> {
        EventBuilder::new()
    }
}

impl<P> Event<P>
where
    P: PublishModel,
{
    #[must_use]
    pub(crate) fn new(publish_model: P) -> Self {
        Self {
            publish_model,
            _single_threaded: PhantomData,
        }
    }

    /// Observes an event that has no explicit magnitude.
    ///
    /// By convention, this is represented as a magnitude of 1. We expose a separate
    /// method for this to make it clear that the magnitude has no inherent meaning.
    #[inline]
    pub fn observe_once(&self) {
        self.batch(1).observe(1);
    }

    /// Observes an event with a specific magnitude.
    #[inline]
    pub fn observe(&self, magnitude: impl AsPrimitive<Magnitude>) {
        self.batch(1).observe(magnitude);
    }

    /// Observes an event with the magnitude being the indicated duration in milliseconds.
    ///
    /// Only the whole number part of the duration is used - fractional milliseconds are ignored.
    /// Values outside the i64 range are not guaranteed to be correctly represented.
    #[inline]
    pub fn observe_millis(&self, duration: Duration) {
        self.batch(1).observe_millis(duration);
    }

    /// Observes the duration of a function call, in milliseconds.
    #[inline]
    pub fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.batch(1).observe_duration_millis(f)
    }

    /// Prepares to observe a batch of events with the same magnitude.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::Event;
    ///
    /// thread_local! {
    ///     static REQUESTS_PROCESSED: Event = Event::builder()
    ///         .name("requests_processed")
    ///         .build();
    ///     static HTTP_RESPONSE_TIME_MS: Event = Event::builder()
    ///         .name("http_response_time_ms")
    ///         .build();
    /// }
    ///
    /// // Record 100 HTTP responses, each taking 50ms
    /// HTTP_RESPONSE_TIME_MS.with(|event| {
    ///     event.batch(100).observe(50);
    /// });
    ///
    /// // Record 50 simple count events
    /// REQUESTS_PROCESSED.with(|event| {
    ///     event.batch(50).observe_once();
    /// });
    /// ```
    #[must_use]
    #[inline]
    pub fn batch(&self, count: usize) -> ObservationBatch<'_, P> {
        ObservationBatch { event: self, count }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> crate::ObservationBagSnapshot {
        self.publish_model.snapshot()
    }
}

/// A batch of pending observations for an event, waiting for the magnitude to be specified.
#[derive(Debug)]
pub struct ObservationBatch<'a, P>
where
    P: PublishModel,
{
    event: &'a Event<P>,
    count: usize,
}

impl<P> ObservationBatch<'_, P>
where
    P: PublishModel,
{
    /// Observes a batch of events that have no explicit magnitude.
    ///
    /// By convention, this is represented as a magnitude of 1. We expose a separate
    /// method for this to make it clear that the magnitude has no inherent meaning.
    #[inline]
    pub fn observe_once(&self) {
        self.event.publish_model.insert(1, self.count);
    }

    /// Observes a batch of events with a specific magnitude.
    #[inline]
    pub fn observe(&self, magnitude: impl AsPrimitive<Magnitude>) {
        self.event.publish_model.insert(magnitude.as_(), self.count);
    }

    /// Observes an event with the magnitude being the indicated duration in milliseconds.
    ///
    /// Only the whole number part of the duration is used - fractional milliseconds are ignored.
    /// Values outside the i64 range are not guaranteed to be correctly represented.
    #[inline]
    pub fn observe_millis(&self, duration: Duration) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "intentional - nothing we can do about it; typical values are in safe range"
        )]
        let millis = duration.as_millis() as i64;

        self.event.publish_model.insert(millis, self.count);
    }

    /// Observes the duration of a function call, in milliseconds.
    #[inline]
    pub fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // TODO: Use low precision time to make this faster.
        // TODO: Consider supporting ultra low precision time from external source.
        let start = Instant::now();

        let result = f();

        self.observe_millis(start.elapsed());

        result
    }
}

impl<P> Observe for Event<P>
where
    P: PublishModel,
{
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn observe_once(&self) {
        self.observe_once();
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn observe(&self, magnitude: impl AsPrimitive<Magnitude>) {
        self.observe(magnitude);
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn observe_millis(&self, duration: Duration) {
        self.observe_millis(duration);
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.observe_duration_millis(f)
    }
}

impl<P> Observe for ObservationBatch<'_, P>
where
    P: PublishModel,
{
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn observe_once(&self) {
        self.observe_once();
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn observe(&self, magnitude: impl AsPrimitive<Magnitude>) {
        self.observe(magnitude);
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn observe_millis(&self, duration: Duration) {
        self.observe_millis(duration);
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.observe_duration_millis(f)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use static_assertions::assert_not_impl_any;

    use super::*;
    use crate::{ObservationBag, ObservationBagSync, Push};

    #[test]
    fn pull_event_observations_are_recorded() {
        // Histogram logic is tested as part of ObservationBag tests, so we do not bother
        // with it here - we assume that if data is correctly recorded, it will reach the histogram.
        let observations = Arc::new(ObservationBagSync::new(&[]));

        let event = Event {
            publish_model: Pull { observations },
            _single_threaded: PhantomData,
        };

        let snapshot = event.snapshot();

        assert_eq!(snapshot.count, 0);
        assert_eq!(snapshot.sum, 0);

        event.observe_once();

        let snapshot = event.snapshot();

        assert_eq!(snapshot.count, 1);
        assert_eq!(snapshot.sum, 1);

        event.batch(3).observe_once();

        let snapshot = event.snapshot();
        assert_eq!(snapshot.count, 4);
        assert_eq!(snapshot.sum, 4);

        event.observe(5);

        let snapshot = event.snapshot();
        assert_eq!(snapshot.count, 5);
        assert_eq!(snapshot.sum, 9);

        event.observe_millis(Duration::from_millis(100));

        let snapshot = event.snapshot();
        assert_eq!(snapshot.count, 6);
        assert_eq!(snapshot.sum, 109);

        event.batch(2).observe(10);

        let snapshot = event.snapshot();
        assert_eq!(snapshot.count, 8);
        assert_eq!(snapshot.sum, 129);
    }

    #[test]
    fn push_event_observations_are_recorded() {
        // Histogram logic is tested as part of ObservationBag tests, so we do not bother
        // with it here - we assume that if data is correctly recorded, it will reach the histogram.
        let observations = Rc::new(ObservationBag::new(&[]));

        let event = Event {
            publish_model: Push { observations },
            _single_threaded: PhantomData,
        };

        let snapshot = event.snapshot();

        assert_eq!(snapshot.count, 0);
        assert_eq!(snapshot.sum, 0);

        event.observe_once();

        let snapshot = event.snapshot();

        assert_eq!(snapshot.count, 1);
        assert_eq!(snapshot.sum, 1);

        event.batch(3).observe_once();

        let snapshot = event.snapshot();
        assert_eq!(snapshot.count, 4);
        assert_eq!(snapshot.sum, 4);

        event.observe(5);

        let snapshot = event.snapshot();
        assert_eq!(snapshot.count, 5);
        assert_eq!(snapshot.sum, 9);

        event.observe_millis(Duration::from_millis(100));

        let snapshot = event.snapshot();
        assert_eq!(snapshot.count, 6);
        assert_eq!(snapshot.sum, 109);

        event.batch(2).observe(10);

        let snapshot = event.snapshot();
        assert_eq!(snapshot.count, 8);
        assert_eq!(snapshot.sum, 129);
    }

    #[test]
    fn event_accepts_different_numeric_types_without_casting() {
        let event = Event::builder().name("test_event").build();

        event.observe(1_u8);
        event.observe(2_u16);
        event.observe(3_u32);
        event.observe(4_u64);
        event.observe(5_usize);
        event.observe(6.66);
        event.observe(7_i32);
        event.observe(8_i128);
    }

    #[test]
    fn single_threaded_type() {
        assert_not_impl_any!(Event: Send, Sync);
    }
}
