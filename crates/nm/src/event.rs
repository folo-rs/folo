use std::{
    marker::PhantomData,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{EventName, LOCAL_REGISTRY, Magnitude, ObservationBag, Observe};

/// Allows you to observe the occurrences of an event in your code.
///
/// The typical pattern is to observe events via thread-local static variables.
///
/// # Example
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
///     CONNECT_TIME_MS.with(|e| e.observe_duration_millis(|| {
///         do_http_connect();
///     }));
/// }
/// # http_connect();
/// # fn do_http_connect() {}
/// ```
///
/// # Thread safety
///
/// This type is single-threaded. You would typically create instances in a
/// `thread_local!` block, so each thread gets its own instance.
#[derive(Debug)]
pub struct Event {
    // While an event is a single-threaded type, we need to use an Arc here because the
    // observations may still be read (without locking) from other threads. We could
    // theoretically do that without sharing but only conditionally, if each thread actively
    // pushed its data to a central repository. That would be impractical, so we must enable
    // reading of the data from other threads, painful as that may be.
    observations: Arc<ObservationBag>,

    _single_threaded: PhantomData<*const ()>,
}

impl Event {
    /// Creates a new event builder with the default configuration.
    #[must_use]
    pub fn builder() -> EventBuilder {
        EventBuilder::default()
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
    pub fn observe(&self, magnitude: Magnitude) {
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
    #[must_use]
    pub fn batch(&self, count: usize) -> ObservationBatch<'_> {
        ObservationBatch { event: self, count }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> crate::ObservationBagSnapshot {
        self.observations.snapshot()
    }
}

/// A batch of pending observations for an event, waiting for the magnitude to be specified.
#[derive(Debug)]
pub struct ObservationBatch<'a> {
    event: &'a Event,
    count: usize,
}

impl ObservationBatch<'_> {
    /// Observes a batch of events that have no explicit magnitude.
    ///
    /// By convention, this is represented as a magnitude of 1. We expose a separate
    /// method for this to make it clear that the magnitude has no inherent meaning.
    #[inline]
    pub fn observe_once(&self) {
        self.event.observations.insert(1, self.count);
    }

    /// Observes a batch of events with a specific magnitude.
    #[inline]
    pub fn observe(&self, magnitude: Magnitude) {
        self.event.observations.insert(magnitude, self.count);
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

        self.event.observations.insert(millis, self.count);
    }

    /// Observes the duration of a function call, in milliseconds.
    #[inline]
    pub fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // TODO: Use low precision time
        let start = Instant::now();

        let result = f();

        self.observe_millis(start.elapsed());

        result
    }
}

impl Observe for Event {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn observe_once(&self) {
        self.observe_once();
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn observe(&self, magnitude: Magnitude) {
        self.observe(magnitude);
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn observe_millis(&self, duration: Duration) {
        self.observe_millis(duration);
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.observe_duration_millis(f)
    }
}

impl Observe for ObservationBatch<'_> {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn observe_once(&self) {
        self.observe_once();
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn observe(&self, magnitude: Magnitude) {
        self.observe(magnitude);
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn observe_millis(&self, duration: Duration) {
        self.observe_millis(duration);
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn observe_duration_millis<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.observe_duration_millis(f)
    }
}

/// Creates instances of [`Event`].
///
/// Required parameters:
/// * `name`
#[derive(Debug, Default)]
pub struct EventBuilder {
    name: EventName,

    /// Upper bounds (inclusive) of histogram buckets to use.
    /// Defaults to empty, which means no histogram for this event.
    histogram_buckets: &'static [Magnitude],
}

impl EventBuilder {
    /// Sets the name of the event.
    ///
    /// Recommended format: `big_medium_small_units`
    /// For example: `net_http_connect_time_ns`
    #[must_use]
    pub fn name(self, name: impl Into<EventName>) -> Self {
        Self {
            name: name.into(),
            ..self
        }
    }

    /// Sets the upper bounds (inclusive) of histogram buckets to use
    /// when creating a histogram of event magnitudes.
    ///
    /// The default is to not create a histogram.
    #[must_use]
    pub fn histogram(self, buckets: &'static [Magnitude]) -> Self {
        Self {
            histogram_buckets: buckets,
            ..self
        }
    }

    /// # Panics
    ///
    /// Panics if a required parameter is not set.
    ///
    /// Panics if an event with this name is already registered.
    /// You can only create an event with each name once (per thread).
    #[must_use]
    pub fn build(self) -> Event {
        assert!(!self.name.is_empty());

        let observation_bag = Arc::new(ObservationBag::new(self.histogram_buckets));

        // This will panic if it is already registered. This is not strictly required and
        // we may relax this constraint in the future but for now we keep it here to help
        // uncover problematic patterns and learn when/where relaxed constraints may be useful.
        LOCAL_REGISTRY.with_borrow(|r| r.register(self.name, Arc::clone(&observation_bag)));

        Event {
            observations: observation_bag,
            _single_threaded: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    #[test]
    fn observations_are_recorded() {
        // Histogram logic is tested as part of ObservationBag tests, so we do not bother
        // with it here - we assume that if data is correctly recorded, it will reach the histogram.
        let observations = Arc::new(ObservationBag::new(&[]));

        let event = Event {
            observations: Arc::clone(&observations),
            _single_threaded: PhantomData,
        };

        let snapshot = observations.snapshot();

        assert_eq!(snapshot.count, 0);
        assert_eq!(snapshot.sum, 0);

        event.observe_once();

        let snapshot = observations.snapshot();

        assert_eq!(snapshot.count, 1);
        assert_eq!(snapshot.sum, 1);

        event.batch(3).observe_once();

        let snapshot = observations.snapshot();
        assert_eq!(snapshot.count, 4);
        assert_eq!(snapshot.sum, 4);

        event.observe(5);

        let snapshot = observations.snapshot();
        assert_eq!(snapshot.count, 5);
        assert_eq!(snapshot.sum, 9);

        event.observe_millis(Duration::from_millis(100));

        let snapshot = observations.snapshot();
        assert_eq!(snapshot.count, 6);
        assert_eq!(snapshot.sum, 109);

        event.batch(2).observe(10);

        let snapshot = observations.snapshot();
        assert_eq!(snapshot.count, 8);
        assert_eq!(snapshot.sum, 129);
    }

    #[test]
    #[should_panic]
    fn build_without_name_panics() {
        drop(Event::builder().build());
    }

    #[test]
    #[should_panic]
    fn build_with_empty_name_panics() {
        drop(Event::builder().name("").build());
    }

    #[test]
    fn build_correctly_configures_histogram() {
        let no_histogram = Event::builder().name("no_histogram").build();
        assert!(no_histogram.snapshot().bucket_magnitudes.is_empty());
        assert!(no_histogram.snapshot().bucket_counts.is_empty());

        let empty_buckets = Event::builder()
            .name("empty_buckets")
            .histogram(&[])
            .build();
        assert!(empty_buckets.snapshot().bucket_magnitudes.is_empty());
        assert!(empty_buckets.snapshot().bucket_counts.is_empty());

        let buckets = &[1, 10, 100, 1000];
        let event_with_buckets = Event::builder()
            .name("with_buckets")
            .histogram(buckets)
            .build();

        let snapshot = event_with_buckets.snapshot();
        assert_eq!(snapshot.bucket_magnitudes, buckets);
        assert_eq!(snapshot.bucket_counts.len(), buckets.len());
    }

    #[test]
    fn single_threaded_type() {
        assert_not_impl_any!(Event: Send, Sync);
    }
}
