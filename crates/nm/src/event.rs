use std::{
    marker::PhantomData,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{EventName, LOCAL_REGISTRY, Magnitude, ObservationBag};

/// Allows you to capture the occurrence of events in your code.
///
/// The typical pattern is to observe events via static thread-local variables.
///
/// # Example
///
/// ```
/// use nm::Event;
/// use std::time::Instant;
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
/// This type is single-threaded. You are expected to put this in a
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
    /// Creates a new builder with the default configuration.
    #[must_use]
    pub fn builder() -> EventBuilder {
        EventBuilder::default()
    }

    /// Observes an event with a magnitude of 1.
    #[inline]
    pub fn observe_unit(&self) {
        self.observe(1);
    }

    /// Observes an event with a specific magnitude.
    #[inline]
    pub fn observe(&self, magnitude: Magnitude) {
        self.observe_many(magnitude, 1);
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

        self.observe(millis);
    }

    /// Observes a number of events, all with the specified magnitude.
    #[inline]
    pub fn observe_many(&self, magnitude: Magnitude, count: usize) {
        self.observations.insert(magnitude, count);
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
