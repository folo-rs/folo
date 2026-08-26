use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::rc::Rc;
use std::sync::Arc;

use crate::{EventName, LOCAL_REGISTRY, ObservationBag, ObservationBagSync, Observations};

/// Publishes the metrics of events that use the push model.
///
/// When creating an event that uses the push model, you must provide an instance of
/// `MetricsPusher` to the event builder. This instance is typically stored in a thread-local
/// static variable.
///
/// On a regular basis, you must then call the `push` method on this instance to publish the metrics
/// to a storage location where they can be included in reports.
#[derive(Debug)]
pub struct MetricsPusher {
    /// The events that publish through this pusher, each represented by the pair of observation
    /// bags between which a push copies data.
    push_registry: Rc<RefCell<Vec<LocalGlobalPair>>>,

    // The pusher publishes thread-local observation bags and must not move between threads.
    _single_threaded: PhantomData<Rc<()>>,
}

// MetricsPusher is single-threaded (!Send, !Sync) and uses interior mutability only for
// metrics registration. Inconsistent state after a caught panic cannot affect safety.
impl UnwindSafe for MetricsPusher {}
impl RefUnwindSafe for MetricsPusher {}

impl MetricsPusher {
    /// Creates a new `MetricsPusher` instance.
    ///
    /// # Example
    ///
    /// ```
    /// use nm::MetricsPusher;
    ///
    /// thread_local! {
    ///     static PUSHER: MetricsPusher = MetricsPusher::new();
    /// }
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            push_registry: Rc::new(RefCell::new(Vec::new())),
            _single_threaded: PhantomData,
        }
    }

    /// Pushes the metrics to a storage location where they can be included in reports.
    ///
    /// This method should be called periodically to ensure that the metrics of events using the
    /// push model are published. Observations recorded since the previous push appear in reports
    /// only once this method has published them.
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
    ///         .name("push_example")
    ///         .pusher_local(&PUSHER)
    ///         .build();
    /// }
    ///
    /// // Observe some events first.
    /// PUSH_EVENT.with(Event::observe_once);
    ///
    /// // Periodically push the accumulated metrics.
    /// PUSHER.with(MetricsPusher::push);
    /// ```
    pub fn push(&self) {
        for pair in self.push_registry.borrow().iter() {
            let current_count = pair.local.count();

            // The local count is monotonically incremented by every data-changing
            // observation. If it has not advanced since the previous push, the local
            // bag's contents are identical to what we already published to the global
            // bag, so we can safely skip the copy. Events that are registered but rarely
            // observed are a common shape, so this skip is worth its comparison: see the
            // `push` benchmarks in `benches/nm_performance.rs`.
            //
            // Edge case (accepted under the crate's mathematics policy): if the local
            // count wraps back to the previously pushed value within a single push
            // interval (e.g., via `batch(usize::MAX).observe(...)`), this check
            // misidentifies the pair as clean. Treated as documented data mangling for
            // extreme values.
            if current_count == pair.last_pushed_count.get() {
                continue;
            }

            pair.global.copy_from(&pair.local);
            pair.last_pushed_count.set(current_count);
        }
    }

    pub(crate) fn pre_register(&self) -> PusherPreRegistration {
        PusherPreRegistration {
            push_registry: Rc::clone(&self.push_registry),
        }
    }

    /// The number of events currently registered for pushing.
    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(crate) fn event_count(&self) -> usize {
        self.push_registry.borrow().len()
    }
}

impl Default for MetricsPusher {
    fn default() -> Self {
        Self::new()
    }
}

/// The pusher hands out pre-registrations to the builder because the builder may not yet
/// be ready to register when it is given the pusher.
///
/// For example, it might not know the histogram buckets yet. Therefore, we hand out this
/// pre-registration, which entitles the builder to register the local observation bag
/// with the pusher once it is ready.
#[derive(Debug)]
pub(crate) struct PusherPreRegistration {
    push_registry: Rc<RefCell<Vec<LocalGlobalPair>>>,
}

impl PusherPreRegistration {
    /// Registers a local observation bag for publishing.
    ///
    /// When the pusher is asked to publish data, it will publish the latest state of this
    /// local observation bag into the global store, making it available for reports.
    pub(crate) fn register(self, name: EventName, source: Rc<ObservationBag>) {
        let global = Arc::new(ObservationBagSync::new(source.bucket_magnitudes()));

        // Duplicate names are rejected to expose invalid duplicate-registration patterns.
        LOCAL_REGISTRY.with_borrow(|r| r.register(name, Arc::clone(&global)));

        self.push_registry.borrow_mut().push(LocalGlobalPair {
            local: source,
            global,
            last_pushed_count: Cell::new(0),
        });
    }
}

/// One event's pair of observation bags: the thread-local bag that observations are recorded
/// into and the shared bag that reports read from.
///
/// A push copies the local bag into the global one, which is what makes observations of events
/// using the push model visible to reports. Keeping the two halves together lets a push walk
/// the pusher's registry without consulting any other data structure.
#[derive(Debug)]
struct LocalGlobalPair {
    local: Rc<ObservationBag>,
    global: Arc<ObservationBagSync>,

    /// The local bag's `count` at the time of the most recent push for this pair.
    ///
    /// On every push, we compare this to `local.count()`; if they match, no new
    /// observations have arrived since the last push and we can skip the copy.
    /// Initialized to the bag's initial count, which correctly causes the first push of a
    /// never-observed pair to be a no-op.
    last_pushed_count: Cell<u64>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;
    use crate::Magnitude;

    /// A single observation, used where a test only needs the local bag to become dirty.
    const ONE_OBSERVATION: usize = 1;

    /// An arbitrary magnitude for observations whose value is irrelevant to the assertion.
    const ANY_MAGNITUDE: Magnitude = 1;

    /// Observations written straight into a global bag to stand in for state that a push must
    /// not disturb. The count differs from `ONE_OBSERVATION` so an unwanted overwrite by a push
    /// is visible in the assertions.
    const EXTERNAL_OBSERVATION_COUNT: usize = 5;

    /// Expected count of a global bag that holds both a pushed local observation and the
    /// externally written observations.
    const LOCAL_PLUS_EXTERNAL_COUNT: u64 = (ONE_OBSERVATION + EXTERNAL_OBSERVATION_COUNT) as u64;

    /// The number of observations a test records when it needs more than one of them.
    const TWO_OBSERVATIONS: u64 = 2 * ONE_OBSERVATION as u64;

    /// Expected sum of a bag that received `TWO_OBSERVATIONS` observations of `ANY_MAGNITUDE`.
    const TWO_OBSERVATIONS_SUM: Magnitude = 2 * ANY_MAGNITUDE;

    assert_impl_all!(MetricsPusher: UnwindSafe, RefUnwindSafe);

    #[test]
    fn default_creates_valid_pusher() {
        let pusher = MetricsPusher::default();

        // Verify the pusher is functional by pre-registering and checking event count.
        assert_eq!(pusher.event_count(), 0);

        let pre_registration = pusher.pre_register();
        let source = Rc::new(ObservationBag::new(&[]));
        pre_registration.register("default_test_event".into(), source);

        assert_eq!(pusher.event_count(), 1);
    }

    #[test]
    fn data_updated_only_on_push() {
        let local = Rc::new(ObservationBag::new(&[]));

        // Observe one occurrence right away. We do NOT expect this to be published until a push.
        local.insert(ANY_MAGNITUDE, ONE_OBSERVATION);

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();

        pre_registration.register("test_event".into(), Rc::clone(&local));

        let global = Arc::clone(&pusher.push_registry.borrow().first().unwrap().global);

        let global_snapshot = global.snapshot();

        // Observations remain local until the first push.
        assert_eq!(0, global_snapshot.count);

        pusher.push();

        let global_snapshot = global.snapshot();
        assert_eq!(ONE_OBSERVATION as u64, global_snapshot.count);

        local.insert(ANY_MAGNITUDE, ONE_OBSERVATION);

        let global_snapshot = global.snapshot();
        assert_eq!(ONE_OBSERVATION as u64, global_snapshot.count);

        pusher.push();

        let global_snapshot = global.snapshot();
        assert_eq!(TWO_OBSERVATIONS, global_snapshot.count);

        pusher.push();
        let global_snapshot = global.snapshot();
        assert_eq!(TWO_OBSERVATIONS, global_snapshot.count);
    }

    #[test]
    fn idle_pair_is_skipped_by_push() {
        // Construct a pair, push it to anchor `last_pushed_count`, then mutate the
        // global bag directly to simulate "external" state. A subsequent idle push
        // (no new observations on local) must not overwrite the manipulated global,
        // proving that the skip path took effect.
        let local = Rc::new(ObservationBag::new(&[]));
        local.insert(ANY_MAGNITUDE, ONE_OBSERVATION);

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();
        pre_registration.register("idle_skip_test".into(), Rc::clone(&local));

        let global = Arc::clone(&pusher.push_registry.borrow().first().unwrap().global);

        // First push synchronizes global with local.
        pusher.push();
        assert_eq!(global.snapshot().count, ONE_OBSERVATION as u64);

        // Mutate the global bag directly. A push that copies unconditionally would
        // overwrite this; a push that detects "local unchanged" leaves the global bag alone.
        global.insert(ANY_MAGNITUDE, EXTERNAL_OBSERVATION_COUNT);
        assert_eq!(global.snapshot().count, LOCAL_PLUS_EXTERNAL_COUNT);

        // No observations on `local` since the previous push.
        pusher.push();
        assert_eq!(global.snapshot().count, LOCAL_PLUS_EXTERNAL_COUNT);
    }

    #[test]
    fn observe_after_idle_push_still_publishes() {
        let local = Rc::new(ObservationBag::new(&[]));

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();
        pre_registration.register("observe_after_idle_test".into(), Rc::clone(&local));

        let global = Arc::clone(&pusher.push_registry.borrow().first().unwrap().global);

        // Observe, push, then push again (idle): both pushes are exercised.
        local.insert(ANY_MAGNITUDE, ONE_OBSERVATION);
        pusher.push();
        assert_eq!(global.snapshot().count, ONE_OBSERVATION as u64);

        pusher.push();
        assert_eq!(global.snapshot().count, ONE_OBSERVATION as u64);

        // A subsequent observation must still be published by the next push.
        local.insert(ANY_MAGNITUDE, ONE_OBSERVATION);
        pusher.push();
        assert_eq!(global.snapshot().count, TWO_OBSERVATIONS);
    }

    #[test]
    fn never_observed_event_first_push_is_skipped() {
        // A freshly registered event has a local count equal to the initial
        // `last_pushed_count`, so the very first push is a no-op for it.
        let local = Rc::new(ObservationBag::new(&[]));

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();
        pre_registration.register("never_observed_test".into(), Rc::clone(&local));

        let global = Arc::clone(&pusher.push_registry.borrow().first().unwrap().global);

        // Directly populate global to detect any unexpected overwrite by push().
        global.insert(ANY_MAGNITUDE, EXTERNAL_OBSERVATION_COUNT);
        assert_eq!(global.snapshot().count, EXTERNAL_OBSERVATION_COUNT as u64);

        pusher.push();

        assert_eq!(global.snapshot().count, EXTERNAL_OBSERVATION_COUNT as u64);
    }

    #[test]
    fn pre_existing_local_data_is_published_on_first_push() {
        // The bag may already contain observations at registration time. Ensure
        // they are published by the first push (the count advanced past the initial
        // value, so the pair is considered dirty).
        let local = Rc::new(ObservationBag::new(&[]));
        local.insert(ANY_MAGNITUDE, ONE_OBSERVATION);
        local.insert(ANY_MAGNITUDE, ONE_OBSERVATION);

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();
        pre_registration.register("pre_existing_test".into(), Rc::clone(&local));

        let global = Arc::clone(&pusher.push_registry.borrow().first().unwrap().global);
        assert_eq!(global.snapshot().count, 0);

        pusher.push();

        let snapshot = global.snapshot();
        assert_eq!(snapshot.count, TWO_OBSERVATIONS);
        assert_eq!(snapshot.sum, TWO_OBSERVATIONS_SUM);
    }
}
