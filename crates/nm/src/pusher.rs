use std::{cell::RefCell, marker::PhantomData, rc::Rc, sync::Arc};

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

impl MetricsPusher {
    /// Creates a new `MetricsPusher` instance.
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
    pub fn push(&self) {
        for pair in self.push_registry.borrow().iter() {
            let local_snapshot = pair.local.snapshot();
            pair.global.replace(&local_snapshot);
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
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
