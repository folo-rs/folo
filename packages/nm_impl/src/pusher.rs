use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::rc::Rc;
use std::sync::Arc;

use crate::{EventName, LOCAL_REGISTRY, ObservationBag, ObservationBagSync, Observations};

/// Facilitates the push-style publishing of metrics.
///
/// When creating a push-mode event, you must provide an instance of `MetricsPusher` to the event
/// builder. This instance is typically stored in a thread-local static variable.
///
/// On a regular basis, you must then call the `push` method on this instance to publish the metrics
/// to a storage location where they can be included in reports.
#[derive(Debug)]
pub struct MetricsPusher {
    /// When we are asked to push data, we publish everything from the local bag to the global bag.
    push_registry: Rc<RefCell<Vec<LocalGlobalPair>>>,

    _single_threaded: PhantomData<*const ()>,
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
    /// This method should be called periodically to ensure that push-model metrics are published.
    ///
    /// Pairs whose local observation bag has not received new observations since the
    /// last push are skipped entirely, avoiding unnecessary work and cache invalidation
    /// for unchanged pairs.
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
    /// // Observe some events first
    /// PUSH_EVENT.with(|e| e.observe_once());
    ///
    /// // Periodically push the accumulated metrics
    /// PUSHER.with(MetricsPusher::push);
    /// ```
    pub fn push(&self) {
        for pair in self.push_registry.borrow().iter() {
            let current_count = pair.local.count();

            // The local count is monotonically incremented by every data-changing
            // observation. If it has not advanced since the previous push, the local
            // bag's contents are identical to what we already published to the global
            // bag, so we can safely skip the copy.
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
    pub(crate) fn event_count(&self) -> usize {
        self.push_registry.borrow().len()
    }
}

impl Default for MetricsPusher {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct LocalGlobalPair {
    local: Rc<ObservationBag>,
    global: Arc<ObservationBagSync>,

    /// The local bag's `count` at the time of the most recent push for this pair.
    ///
    /// On every push, we compare this to `local.count()`; if they match, no new
    /// observations have arrived since the last push and we can skip the copy.
    /// Initialized to 0, which matches the bag's initial count and correctly
    /// causes the first push of a never-observed pair to be a no-op.
    last_pushed_count: Cell<u64>,
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

        // Register the global half of the pair. This essentially makes the pusher act
        // like a regular `Event<Pull>` as far as the global reporting logic is concerned.

        // This will panic if it is already registered. This is not strictly required and
        // we may relax this constraint in the future but for now we keep it here to help
        // uncover problematic patterns and learn when/where relaxed constraints may be useful.
        LOCAL_REGISTRY.with_borrow(|r| r.register(name, Arc::clone(&global)));

        self.push_registry.borrow_mut().push(LocalGlobalPair {
            local: source,
            global,
            last_pushed_count: Cell::new(0),
        });
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

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
        local.insert(1, 1);

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();

        pre_registration.register("test_event".into(), Rc::clone(&local));

        let global = Arc::clone(&pusher.push_registry.borrow().first().unwrap().global);

        let global_snapshot = global.snapshot();

        // The first occurrence was measured before a push, so it should not be published yet.
        assert_eq!(0, global_snapshot.count);

        pusher.push();

        let global_snapshot = global.snapshot();
        // Now it should show up.
        assert_eq!(1, global_snapshot.count);

        // This one shows up with the next push.
        local.insert(1, 1);

        let global_snapshot = global.snapshot();
        // Still 1, because we did not push yet.
        assert_eq!(1, global_snapshot.count);

        pusher.push();

        let global_snapshot = global.snapshot();
        // Now it should be 2.
        assert_eq!(2, global_snapshot.count);

        // This should not change anything.
        pusher.push();
        let global_snapshot = global.snapshot();
        // Still 2, because we did not observe anything new.
        assert_eq!(2, global_snapshot.count);
    }

    #[test]
    fn idle_pair_is_skipped_by_push() {
        // Construct a pair, push it to anchor `last_pushed_count`, then mutate the
        // global bag directly to simulate "external" state. A subsequent idle push
        // (no new observations on local) must not overwrite the manipulated global,
        // proving that the skip path took effect.
        let local = Rc::new(ObservationBag::new(&[]));
        local.insert(7, 1);

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();
        pre_registration.register("idle_skip_test".into(), Rc::clone(&local));

        let global = Arc::clone(&pusher.push_registry.borrow().first().unwrap().global);

        // First push synchronizes global with local.
        pusher.push();
        assert_eq!(global.snapshot().count, 1);

        // Mutate the global bag directly. A normal (non-skipping) push would
        // overwrite this; an optimized push that detects "local unchanged" leaves
        // the global bag alone.
        global.insert(99, 5);
        assert_eq!(global.snapshot().count, 6);

        // No observations on `local` since the previous push.
        pusher.push();
        assert_eq!(
            global.snapshot().count,
            6,
            "idle push must not overwrite global state",
        );
    }

    #[test]
    fn observe_after_idle_push_still_publishes() {
        let local = Rc::new(ObservationBag::new(&[]));

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();
        pre_registration.register("observe_after_idle_test".into(), Rc::clone(&local));

        let global = Arc::clone(&pusher.push_registry.borrow().first().unwrap().global);

        // Observe, push, then push again (idle): both pushes are exercised.
        local.insert(1, 1);
        pusher.push();
        assert_eq!(global.snapshot().count, 1);

        pusher.push();
        assert_eq!(global.snapshot().count, 1);

        // A subsequent observation must still be published by the next push.
        local.insert(1, 1);
        pusher.push();
        assert_eq!(global.snapshot().count, 2);
    }

    #[test]
    fn never_observed_event_first_push_is_skipped() {
        // A freshly registered event has local count == 0, which equals the initial
        // `last_pushed_count == 0`, so the very first push should be a no-op for it.
        let local = Rc::new(ObservationBag::new(&[]));

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();
        pre_registration.register("never_observed_test".into(), Rc::clone(&local));

        let global = Arc::clone(&pusher.push_registry.borrow().first().unwrap().global);

        // Directly populate global to detect any unexpected overwrite by push().
        global.insert(42, 3);
        assert_eq!(global.snapshot().count, 3);

        pusher.push();

        assert_eq!(
            global.snapshot().count,
            3,
            "first push of a never-observed event must not overwrite global",
        );
    }

    #[test]
    fn pre_existing_local_data_is_published_on_first_push() {
        // The bag may already contain observations at registration time. Ensure
        // they are published by the first push (count went from 0 to >0, so the
        // pair is considered dirty).
        let local = Rc::new(ObservationBag::new(&[]));
        local.insert(1, 1);
        local.insert(2, 1);

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();
        pre_registration.register("pre_existing_test".into(), Rc::clone(&local));

        let global = Arc::clone(&pusher.push_registry.borrow().first().unwrap().global);
        assert_eq!(global.snapshot().count, 0);

        pusher.push();

        let snapshot = global.snapshot();
        assert_eq!(snapshot.count, 2);
        assert_eq!(snapshot.sum, 3);
    }
}
