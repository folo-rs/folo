use std::marker::PhantomData;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::rc::Rc;
use std::sync::Arc;
use std::thread::LocalKey;

use crate::{
    Event, EventName, LOCAL_REGISTRY, Magnitude, MetricsPusher, ObservationBag, ObservationBagSync,
    PublishModel, Pull, Push, PusherPreRegistration,
};

/// Configures and builds instances of [`Event`].
///
/// The event name is required.
///
/// Use `Event::builder()` to create a new instance of this builder.
///
/// See [crate-level documentation][crate] for more details on how to create and use events.
#[derive(Debug)]
pub struct EventBuilder<P = Pull>
where
    P: PublishModel,
{
    name: EventName,

    /// Upper bounds (inclusive) of histogram buckets to use.
    /// Defaults to empty, which means no histogram for this event.
    ///
    /// Must be in ascending order if provided.
    /// Must not have a `Magnitude::MAX` bucket (automatically synthesized).
    histogram_buckets: &'static [Magnitude],

    /// If configured for the push publishing model, this is a pre-registration ticket
    /// from the pusher that will deliver the data to the reporting system.
    push_via: Option<PusherPreRegistration>,

    _p: PhantomData<P>,
    // Builders carry thread-local registration state and must remain on their creating thread.
    _single_threaded: PhantomData<Rc<()>>,
}

// EventBuilder is single-threaded (!Send, !Sync) and uses interior mutability only for
// metrics registration. Inconsistent state after a caught panic cannot affect safety.
impl<P: PublishModel> UnwindSafe for EventBuilder<P> {}
impl<P: PublishModel> RefUnwindSafe for EventBuilder<P> {}

impl<P> EventBuilder<P>
where
    P: PublishModel,
{
    pub(crate) fn new() -> Self {
        Self {
            name: EventName::default(),
            histogram_buckets: &[],
            push_via: None,
            _p: PhantomData,
            _single_threaded: PhantomData,
        }
    }

    /// Configures the event name.
    ///
    /// The event name is a required setting.
    ///
    /// Recommended format: `big_medium_small_units`.
    /// For example: `net_http_connect_time_ns`.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::Event;
    ///
    /// thread_local! {
    ///     static MY_EVENT: Event = Event::builder()
    ///         .name("http_requests_duration_ms")
    ///         .build();
    /// }
    /// ```
    #[must_use]
    pub fn name(self, name: impl Into<EventName>) -> Self {
        Self {
            name: name.into(),
            ..self
        }
    }

    /// Configures the upper bounds (inclusive) of histogram buckets to use
    /// when creating a histogram of event magnitudes.
    ///
    /// The default is to not create a histogram.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::{Event, Magnitude};
    ///
    /// const RESPONSE_TIME_BUCKETS_MS: &[Magnitude] = &[1, 10, 50, 100, 500, 1000];
    ///
    /// thread_local! {
    ///     static HTTP_RESPONSE_TIME_MS: Event = Event::builder()
    ///         .name("http_response_time_ms")
    ///         .histogram(RESPONSE_TIME_BUCKETS_MS)
    ///         .build();
    /// }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if bucket magnitudes are not in ascending order.
    ///
    /// Panics if one of the values is `Magnitude::MAX`. You do not need to specify this
    /// bucket yourself; it is automatically synthesized for all histograms to catch values
    /// that exceed the user-defined buckets.
    #[must_use]
    pub fn histogram(self, buckets: &'static [Magnitude]) -> Self {
        assert!(
            buckets
                .array_windows::<2>()
                .all(|[lower, upper]| lower < upper)
        );
        assert!(!buckets.contains(&Magnitude::MAX));

        Self {
            histogram_buckets: buckets,
            ..self
        }
    }
}

impl EventBuilder<Pull> {
    /// Configures the event to be published using the push model,
    /// whereby a pusher is used to explicitly publish data for reporting purposes.
    ///
    /// Any data from such an event will only reach reports after [`MetricsPusher::push()`][1]
    /// is called on the referenced pusher.
    ///
    /// This can provide lower overhead than the pull model, which is the default,
    /// at the cost of delaying data updates until the pusher is invoked.
    /// Note that if nothing ever calls the pusher on a thread,
    /// the data from that thread will never be published.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::{Event, MetricsPusher, Push};
    ///
    /// let pusher = MetricsPusher::new();
    /// let push_event: Event<Push> = Event::builder()
    ///     .name("push_example")
    ///     .pusher(&pusher)
    ///     .build();
    /// push_event.observe_once();
    /// pusher.push();
    /// ```
    ///
    /// [1]: crate::MetricsPusher::push
    #[must_use]
    pub fn pusher(self, pusher: &MetricsPusher) -> EventBuilder<Push> {
        EventBuilder {
            name: self.name,
            histogram_buckets: self.histogram_buckets,
            push_via: Some(pusher.pre_register()),
            _p: PhantomData,
            _single_threaded: PhantomData,
        }
    }

    /// Configures the event to be published using the push model,
    /// whereby a pusher is used to explicitly publish data for reporting purposes.
    ///
    /// Any data from such an event will only reach reports after [`MetricsPusher::push()`][1]
    /// is called on the referenced pusher.
    ///
    /// This can provide lower overhead than the pull model, which is the default,
    /// at the cost of delaying data updates until the pusher is invoked.
    /// Note that if nothing ever calls the pusher on a thread,
    /// the data from that thread will never be published.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::{Event, MetricsPusher, Push};
    ///
    /// thread_local! {
    ///     static PUSHER: MetricsPusher = MetricsPusher::new();
    ///
    ///     static PUSH_EVENT: Event<Push> = Event::builder()
    ///         .name("push_local_example")
    ///         .pusher_local(&PUSHER)
    ///         .build();
    /// }
    /// PUSH_EVENT.with(Event::observe_once);
    /// PUSHER.with(MetricsPusher::push);
    /// ```
    ///
    /// [1]: crate::MetricsPusher::push
    #[must_use]
    pub fn pusher_local(self, pusher: &'static LocalKey<MetricsPusher>) -> EventBuilder<Push> {
        pusher.with(|p| self.pusher(p))
    }

    /// Builds an event from this configuration.
    ///
    /// # Panics
    ///
    /// Panics if the event name is not set.
    ///
    /// Panics if an event with this name has already been registered on this thread.
    /// You can only create an event with each name once per thread.
    #[must_use]
    // Generated mutations cannot produce a viable alternate implementation for this signature.
    #[cfg_attr(test, mutants::skip)]
    pub fn build(self) -> Event<Pull> {
        assert!(!self.name.is_empty());

        let observation_bag = Arc::new(ObservationBagSync::new(self.histogram_buckets));

        // Duplicate names are rejected to expose invalid duplicate-registration patterns.
        LOCAL_REGISTRY.with_borrow(|r| r.register(self.name, Arc::clone(&observation_bag)));

        Event::new(Pull {
            observations: observation_bag,
        })
    }
}

impl EventBuilder<Push> {
    /// Builds an event from this configuration.
    ///
    /// # Panics
    ///
    /// Panics if the event name is not set.
    ///
    /// Panics if an event with this name has already been registered on this thread.
    /// You can only create an event with each name once per thread.
    #[must_use]
    pub fn build(self) -> Event<Push> {
        assert!(!self.name.is_empty());

        let observation_bag = Rc::new(ObservationBag::new(self.histogram_buckets));

        let pre_registration = self
            .push_via
            .expect("converting to EventBuilder<Push> always records a pusher registration");

        // Completing the registration retains the observation state for subsequent pushes.
        pre_registration.register(self.name, Rc::clone(&observation_bag));

        Event::new(Push {
            observations: observation_bag,
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::LocalEventRegistry;

    assert_impl_all!(EventBuilder<Pull>: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(EventBuilder<Push>: UnwindSafe, RefUnwindSafe);

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
    #[should_panic]
    fn build_with_magnitude_max_in_histogram_panics() {
        drop(
            Event::builder()
                .name("build_with_magnitude_max_in_histogram_panics")
                .histogram(&[1, 2, Magnitude::MAX])
                .build(),
        );
    }

    #[test]
    #[should_panic]
    fn build_with_non_ascending_histogram_buckets_panics() {
        drop(
            Event::builder()
                .name("build_with_non_ascending_histogram_buckets")
                .histogram(&[3, 2, 1])
                .build(),
        );
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

        // Note: this does not include the `Magnitude::MAX` bucket.
        // The data for that bucket is synthesized at reporting time.
        assert_eq!(snapshot.bucket_magnitudes, buckets);
        assert_eq!(snapshot.bucket_counts.len(), buckets.len());
    }

    #[test]
    fn pull_build_registers_with_registry() {
        let previous_count = LOCAL_REGISTRY.with_borrow(LocalEventRegistry::event_count);

        // Dropping the event handle does not unregister its observation state; it only
        // prevents further observations through this handle.
        drop(
            Event::builder()
                .name("pull_build_registers_with_registry")
                .build(),
        );

        let new_count = LOCAL_REGISTRY.with_borrow(LocalEventRegistry::event_count);

        assert_eq!(new_count, previous_count + 1);
    }

    #[test]
    fn pusher_build_registers_with_pusher() {
        let pusher = MetricsPusher::new();

        // Dropping the event handle does not unregister its observation state; it only
        // prevents further observations through this handle.
        drop(
            Event::builder()
                .name("push_build_registers_with_pusher")
                .pusher(&pusher)
                .build(),
        );

        assert_eq!(pusher.event_count(), 1);
    }

    thread_local!(static LOCAL_PUSHER: MetricsPusher = MetricsPusher::new());

    #[test]
    fn pusher_local_build_registers_with_pusher() {
        // Dropping the event handle does not unregister its observation state; it only
        // prevents further observations through this handle.
        drop(
            Event::builder()
                .name("push_build_registers_with_pusher")
                .pusher_local(&LOCAL_PUSHER)
                .build(),
        );

        assert_eq!(LOCAL_PUSHER.with(MetricsPusher::event_count), 1);
    }

    #[test]
    fn single_threaded_type() {
        assert_not_impl_any!(EventBuilder: Send, Sync);
    }

    #[test]
    #[should_panic]
    fn register_pull_and_push_same_name_panics() {
        let pusher = MetricsPusher::new();

        // Dropping the event handle does not unregister its observation state; it only
        // prevents further observations through this handle.
        drop(
            Event::builder()
                .name("conflicting_name_pull_and_push")
                .pusher(&pusher)
                .build(),
        );

        drop(
            Event::builder()
                .name("conflicting_name_pull_and_push")
                .build(),
        );
    }
}
