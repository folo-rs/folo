use std::cell::RefCell;
use std::marker::PhantomData;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::rc::Rc;
use std::time::Duration;

use fast_time::Clock;
use num_traits::AsPrimitive;

#[cfg(test)]
use crate::ObservationBagSnapshot;
use crate::{
    EventBuilder, IMPLICIT_OCCURRENCE_MAGNITUDE, Magnitude, ONE_ITEM_BATCH, Observe, PublishModel,
    Pull,
};

/// Allows you to observe the occurrences of an event in your code.
///
/// The typical pattern is to observe events via thread-local static variables.
///
/// # Publishing models
///
/// The ultimate goal of the metrics collected by an [`Event`] is to end up in a [`Report`][1].
/// The available publishing models are:
///
/// - **Pull model:** The reporting system queries each event in the process for its latest data
///   set when generating a report. This is the default and requires no action from you.
/// - **Push model:** Data from an event only flows to a thread-local [`MetricsPusher`][2], which
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
/// pub fn http_connect() -> bool {
///     CONNECT_TIME_MS.with(|event| event.observe_duration_millis(do_http_connect))
/// }
/// assert!(http_connect());
/// # fn do_http_connect() -> bool { true }
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
/// pub fn http_connect() -> bool {
///     CONNECT_TIME_MS.with(|event| event.observe_duration_millis(do_http_connect))
/// }
///
/// loop {
///     assert!(http_connect());
///
///     // Periodically push the data to the reporting system.
///     if is_time_to_push() {
///         HTTP_EVENTS_PUSHER.with(MetricsPusher::push);
///     }
///     # break; // Avoid infinite loop when running example.
/// }
/// # fn do_http_connect() -> bool { true }
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

    /// Low-overhead clock for duration observation. We store one per event to maximize
    /// cache efficiency of the underlying platform time source.
    clock: RefCell<Clock>,

    // Event registration and local observation storage belong to the creating thread.
    _single_threaded: PhantomData<Rc<()>>,
}

// Event is single-threaded (!Send, !Sync) and uses interior mutability only for metrics
// tracking. Inconsistent state after a caught panic cannot affect safety.
impl<P: PublishModel> UnwindSafe for Event<P> {}
impl<P: PublishModel> RefUnwindSafe for Event<P> {}

impl Event<Pull> {
    /// Creates a new event builder with the default builder configuration.
    #[must_use]
    // The mutation only renames the function and leaves behavior unchanged.
    #[cfg_attr(test, mutants::skip)]
    pub fn builder() -> EventBuilder<Pull> {
        EventBuilder::new()
    }
}

// Callgrind and disassembly showed that the default cross-CGU decision left the complete
// insertion chain out of line. Inlining its forwarders removes a call from every observation.
impl<P> Event<P>
where
    P: PublishModel,
{
    #[must_use]
    pub(crate) fn new(publish_model: P) -> Self {
        Self {
            publish_model,
            clock: RefCell::new(Clock::new()),
            _single_threaded: PhantomData,
        }
    }

    /// Observes an event that has no explicit magnitude.
    ///
    /// By convention, this is represented as a magnitude of 1. We expose a separate
    /// method for this to make it clear that the magnitude has no inherent meaning.
    #[inline]
    pub fn observe_once(&self) {
        self.batch(ONE_ITEM_BATCH)
            .observe(IMPLICIT_OCCURRENCE_MAGNITUDE);
    }

    /// Observes an event with a specific magnitude.
    #[inline]
    pub fn observe(&self, magnitude: impl AsPrimitive<Magnitude>) {
        self.batch(ONE_ITEM_BATCH).observe(magnitude);
    }

    /// Observes an event with the magnitude being the indicated duration in milliseconds.
    ///
    /// Only the whole number part of the duration is used; fractional milliseconds are ignored.
    /// Values outside the i64 range are not guaranteed to be correctly represented.
    #[inline]
    pub fn observe_millis(&self, duration: Duration) {
        self.batch(ONE_ITEM_BATCH).observe_millis(duration);
    }

    /// Observes the duration of a function call, in milliseconds.
    ///
    /// Uses a low-precision clock optimized for high-frequency capture. The measurement
    /// has a granularity of roughly 1-20 ms. Durations shorter than the granularity may
    /// appear as zero.
    ///
    /// # Reentrancy
    ///
    /// The measured function may observe this event or any other event.
    #[inline]
    pub fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.batch(ONE_ITEM_BATCH).observe_duration_millis(f)
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
    /// // Record a batch of HTTP response durations.
    /// HTTP_RESPONSE_TIME_MS.with(|event| {
    ///     event.batch(100).observe(50);
    /// });
    ///
    /// // Record a batch of count events.
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
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(crate) fn snapshot(&self) -> ObservationBagSnapshot {
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

// Callgrind and disassembly showed that the default cross-CGU decision left the complete
// insertion chain out of line. Inlining its forwarders removes a call from every observation.
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
        self.event
            .publish_model
            .insert(IMPLICIT_OCCURRENCE_MAGNITUDE, self.count);
    }

    /// Observes a batch of events with a specific magnitude.
    #[inline]
    pub fn observe(&self, magnitude: impl AsPrimitive<Magnitude>) {
        self.event.publish_model.insert(magnitude.as_(), self.count);
    }

    /// Observes an event with the magnitude being the indicated duration in milliseconds.
    ///
    /// Only the whole number part of the duration is used; fractional milliseconds are ignored.
    /// Values outside the i64 range are not guaranteed to be correctly represented.
    #[inline]
    pub fn observe_millis(&self, duration: Duration) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "The truncation is intentional because typical duration values are in range."
        )]
        let millis = duration.as_millis() as i64;

        self.event.publish_model.insert(millis, self.count);
    }

    /// Observes the duration of a function call, in milliseconds.
    ///
    /// Uses a low-precision clock optimized for high-frequency capture. The measurement
    /// has a granularity of roughly 1-20 ms. Durations shorter than the granularity may
    /// appear as zero.
    ///
    /// # Reentrancy
    ///
    /// The measured function may observe this event or any other event.
    #[inline]
    pub fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let start = {
            let mut clock = self.event.clock.borrow_mut();
            clock.now()
        };

        let result = f();

        let elapsed = {
            let mut clock = self.event.clock.borrow_mut();
            start.elapsed(&mut clock)
        };

        self.observe_millis(elapsed);

        result
    }
}

// Callgrind and disassembly showed that the default cross-CGU decision left the complete
// insertion chain out of line. Inlining its forwarders removes a call from every observation.
impl<P> Observe for Event<P>
where
    P: PublishModel,
{
    #[cfg_attr(test, mutants::skip)] // Mutation testing does not benefit from trivial forwarding.
    #[inline]
    fn observe_once(&self) {
        self.observe_once();
    }

    #[cfg_attr(test, mutants::skip)] // Mutation testing does not benefit from trivial forwarding.
    #[inline]
    fn observe(&self, magnitude: impl AsPrimitive<Magnitude>) {
        self.observe(magnitude);
    }

    #[cfg_attr(test, mutants::skip)] // Mutation testing does not benefit from trivial forwarding.
    #[inline]
    fn observe_millis(&self, duration: Duration) {
        self.observe_millis(duration);
    }

    #[cfg_attr(test, mutants::skip)] // Mutation testing does not benefit from trivial forwarding.
    #[inline]
    fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.observe_duration_millis(f)
    }
}

// Callgrind and disassembly showed that the default cross-CGU decision left the complete
// insertion chain out of line. Inlining its forwarders removes a call from every observation.
impl<P> Observe for ObservationBatch<'_, P>
where
    P: PublishModel,
{
    #[cfg_attr(test, mutants::skip)] // Mutation testing does not benefit from trivial forwarding.
    #[inline]
    fn observe_once(&self) {
        self.observe_once();
    }

    #[cfg_attr(test, mutants::skip)] // Mutation testing does not benefit from trivial forwarding.
    #[inline]
    fn observe(&self, magnitude: impl AsPrimitive<Magnitude>) {
        self.observe(magnitude);
    }

    #[cfg_attr(test, mutants::skip)] // Mutation testing does not benefit from trivial forwarding.
    #[inline]
    fn observe_millis(&self, duration: Duration) {
        self.observe_millis(duration);
    }

    #[cfg_attr(test, mutants::skip)] // Mutation testing does not benefit from trivial forwarding.
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
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::rc::Rc;
    use std::sync::Arc;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{ObservationBag, ObservationBagSync, Push};

    assert_impl_all!(Event<Pull>: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(Event<Push>: UnwindSafe, RefUnwindSafe);

    #[test]
    fn pull_event_observations_are_recorded() {
        // ObservationBag tests cover histogram bucketing, so this test verifies count and
        // sum recording only.
        let observations = Arc::new(ObservationBagSync::new(&[]));

        let event = Event {
            publish_model: Pull { observations },
            clock: RefCell::new(Clock::new()),
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
        // ObservationBag tests cover histogram bucketing, so this test verifies count and
        // sum recording only.
        let observations = Rc::new(ObservationBag::new(&[]));

        let event = Event {
            publish_model: Push { observations },
            clock: RefCell::new(Clock::new()),
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
    fn duration_callback_can_reenter_the_same_event() {
        let observations = Arc::new(ObservationBagSync::new(&[]));
        let event = Event {
            publish_model: Pull { observations },
            clock: RefCell::new(Clock::new()),
            _single_threaded: PhantomData,
        };

        let result = event.observe_duration_millis(|| event.observe_duration_millis(|| true));

        assert!(result);
        assert_eq!(event.snapshot().count, 2);
    }

    #[test]
    fn single_threaded_type() {
        assert_not_impl_any!(Event: Send, Sync);
    }
}
