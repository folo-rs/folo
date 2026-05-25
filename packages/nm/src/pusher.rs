use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::ptr::NonNull;
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
    ///
    /// Each pair lives in its own `Rc` so that `LocalGlobalPair::dirty` has a stable
    /// address that the bag can hold a raw back-pointer to. We deliberately do not use
    /// `Box` even though that would be the obvious owning indirection: under Stacked
    /// Borrows, every move of a `Box` (including when the `Vec` reallocates and moves
    /// the existing slots) issues a `Unique` retag on the pointee, which invalidates
    /// any raw pointer derived from `&pair.dirty` that was installed in the bag earlier.
    /// `Rc` carries no `noalias`/`Unique` semantics, so its moves leave the pointee's
    /// borrow stack untouched and prior raw pointers remain valid. Registration is
    /// cold, so the small per-pair `Rc` header (two counters) is negligible.
    ///
    /// The alternative — keeping `Vec<LocalGlobalPair>` contiguous and reconnecting
    /// every existing bag's back-pointer on reallocation — is a more invasive design
    /// with a larger `unsafe` surface and is deferred to a follow-up if the
    /// `Rc`-per-pair layout proves insufficient (see issue #160 design notes).
    push_registry: Rc<RefCell<Vec<Rc<LocalGlobalPair>>>>,

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
            // The dirty bit is set by every data-changing observation through the bag's
            // back-pointer into this pair (see `ObservationBag::external_dirty`). Reading
            // it here means the idle scan touches only the pair's own cache line, never
            // the bag through its `Rc`.
            if !pair.dirty.get() {
                continue;
            }

            // Clear the bit BEFORE the copy. `copy_from` currently only manipulates
            // `Cell` and atomic fields (no user callbacks, no destructors of user-owned
            // values), so a re-entrant observation during the copy cannot happen today.
            // Clearing first is a defense-in-depth measure: if `copy_from` ever grows a
            // path that re-enters via observe (e.g. through a future callback), the
            // late-set dirty bit will survive past this push and be picked up next time
            // rather than being lost.
            //
            // We use `get` + conditional `set(false)` instead of `replace(false)`.
            // `replace` writes unconditionally, which would dirty every idle pair's
            // cache line every push tick - defeating the optimization.
            pair.dirty.set(false);
            pair.global.copy_from(&pair.local);
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

    /// `true` when the local bag has received at least one data-changing observation
    /// since the most recent `copy_from`. Written from the bag's `insert` path via
    /// the back-pointer installed by `connect_external_dirty` on registration, and
    /// cleared by `MetricsPusher::push` immediately before each `copy_from`.
    ///
    /// `Drop` clears the bag's back-pointer so that the bag never references a freed
    /// `Cell<bool>`. This requires the pair to live behind a stable address (an
    /// `Rc`), which is why `MetricsPusher::push_registry` is `Vec<Rc<...>>`.
    dirty: Cell<bool>,
}

impl Drop for LocalGlobalPair {
    fn drop(&mut self) {
        // Custom `Drop` runs before the fields are released, so `self.local` is
        // guaranteed alive here. `disconnect_external_dirty` clears the bag's
        // back-pointer to `None`, so after this returns and `self.dirty` is
        // destructed, the bag no longer references the freed `Cell<bool>`.
        //
        // Any subsequent observe on a still-living orphaned bag (e.g. an
        // `Event<Push>` that outlives its `MetricsPusher`) sees `external_dirty
        // == None` and the pointer-set step becomes a safe no-op.
        let ptr = NonNull::from(&self.dirty);
        self.local.disconnect_external_dirty(ptr);
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
    push_registry: Rc<RefCell<Vec<Rc<LocalGlobalPair>>>>,
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
        //
        // We let this fallible step run BEFORE connecting the bag's back-pointer so that
        // a panic here cannot leave the bag with a dangling pointer to a `Cell<bool>`
        // that the unwound caller never gets to drop.
        LOCAL_REGISTRY.with_borrow(|r| r.register(name, Arc::clone(&global)));

        // Capture `source.count()` before moving `source` into the pair. Used to
        // preserve the existing "pre-existing local data is published on first push"
        // behavior: if the bag was observed before being registered with the pusher,
        // mark the pair dirty so the first push copies that pre-existing data.
        let initial_dirty = source.count() > 0;

        let pair = Rc::new(LocalGlobalPair {
            local: source,
            global,
            dirty: Cell::new(initial_dirty),
        });

        let dirty_ptr = NonNull::from(&pair.dirty);

        // SAFETY: `pair` lives behind an `Rc` from this point on, so `&pair.dirty`
        // has a stable heap address that does not move when the `Rc` is later cloned
        // into the registry's `Vec` or when that `Vec` later reallocates (the moves
        // only touch the `Rc` struct itself, not the heap-allocated `RcBox` that owns
        // the `LocalGlobalPair`). Unlike `Box`, `Rc` does not carry `Unique`/`noalias`
        // semantics under Stacked Borrows, so those moves do not retag the pointee and
        // the raw pointer derived here remains valid for the full lifetime of the pair.
        //
        // `LocalGlobalPair::drop` calls `disconnect_external_dirty(dirty_ptr)` before
        // `self.dirty` is destructed, so the bag never observes a freed pointee.
        //
        // The one-bag-one-pair invariant holds: the bag's `Rc` was just moved into
        // this pair, and the pre-registration is consumed (`self`) by this single
        // call, so we are the only caller that can connect this bag.
        unsafe {
            pair.local.connect_external_dirty(dirty_ptr);
        }

        self.push_registry.borrow_mut().push(pair);
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

    #[test]
    fn observe_after_pusher_drop_is_safe_noop() {
        // The bag may outlive its pusher (e.g. when an `Event<Push>` keeps an `Rc`
        // strand alive). Once the pusher (and therefore its pair) drops, the bag's
        // back-pointer is cleared and further observations must be a safe no-op
        // rather than a use-after-free.
        let local = Rc::new(ObservationBag::new(&[]));

        let pusher = MetricsPusher::new();
        let pre_registration = pusher.pre_register();
        pre_registration.register("orphaned_event_test".into(), Rc::clone(&local));

        // Drop the pusher while the bag is still alive (via our `Rc::clone`).
        drop(pusher);

        // Subsequent observations must be safe. We do not need to assert anything
        // beyond "this does not crash or trip Miri" - the bag is no longer connected
        // to any pusher, so the dirty back-pointer step becomes a no-op.
        local.insert(1, 1);
        local.insert(42, 5);

        // The bag itself still records observations locally even when orphaned.
        assert_eq!(local.count(), 6);
    }

    #[test]
    fn pair_drop_clears_bag_back_pointer() {
        // Verifies that `LocalGlobalPair::Drop` actually runs the
        // `disconnect_external_dirty` step and that the disconnect actually
        // clears the bag's back-pointer. Without this assertion, mutations that
        // turn the `Drop` body or the disconnect logic into a no-op would pass
        // silently: the bag would retain a dangling pointer, but writes to the
        // freed cell would not observably fail in unit tests (the allocator may
        // not have reused the memory yet, and the bag still records the
        // observation locally).
        let local = Rc::new(ObservationBag::new(&[]));
        assert!(
            !local.external_dirty_is_connected(),
            "fresh bag must not be connected to any pair",
        );

        let pusher = MetricsPusher::new();
        pusher
            .pre_register()
            .register("pair_drop_clears_back_pointer".into(), Rc::clone(&local));
        assert!(
            local.external_dirty_is_connected(),
            "registration must install the back-pointer",
        );

        drop(pusher);

        // Drop must have called `disconnect_external_dirty` with the pair's
        // matching pointer, restoring the bag to the disconnected state.
        assert!(
            !local.external_dirty_is_connected(),
            "pair drop must clear the bag's back-pointer",
        );
    }
}
