use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};
use std::thread::{self, ThreadId};

use crate::{ERR_POISONED_LOCK, EventName, HashMap, ObservationBagSync};

/// Maps each event name to the shared observation storage that records that event within one
/// scope. A scope is either a single thread's live registrations or one compatible slice of the
/// process-wide archive.
type ObservationBagMap = HashMap<EventName, Arc<ObservationBagSync>>;

/// Keeps track of the events registered on a single thread, for local access only.
///
/// Facilitates event registration and unregistration from the global registry. An
/// event is automatically registered globally for as long as it is registered locally.
///
/// The global registry is the sole owner of the per-thread observation bags. This type holds
/// no observation storage of its own; it only forwards registrations to the global registry
/// and, on drop, unregisters this thread from it.
#[derive(Debug)]
pub(crate) struct LocalEventRegistry<'g> {
    thread_id: ThreadId,
    global_registry: &'g GlobalEventRegistry,

    // Registration lifetime is tied to the thread identified above.
    _single_threaded: PhantomData<Rc<()>>,
}

impl<'g> LocalEventRegistry<'g> {
    pub(crate) fn new(global_registry: &'g GlobalEventRegistry) -> Self {
        Self {
            thread_id: thread::current().id(),
            global_registry,
            _single_threaded: PhantomData,
        }
    }

    pub(crate) fn register(&self, name: EventName, observation_bag: Arc<ObservationBagSync>) {
        self.global_registry
            .register(self.thread_id, name, observation_bag);
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(crate) fn event_count(&self) -> usize {
        // The global registry is authoritative for this thread's registrations.
        self.global_registry.thread_event_count(self.thread_id)
    }
}

impl Drop for LocalEventRegistry<'_> {
    fn drop(&mut self) {
        // Use the stored thread id rather than the dropping thread's id: the registry is owned
        // by the thread that created it, and the stored id is the one it registered under.
        self.global_registry.unregister_thread(self.thread_id);
    }
}

/// Keeps track of the events registered on all threads.
///
/// This is typically used for collecting and reporting on metrics data from the entire process.
#[derive(Debug)]
pub(crate) struct GlobalEventRegistry {
    state: RwLock<GlobalObservationBagsState>,
}

/// The mutable state of the global registry, guarded as a unit by the registry's lock.
///
/// It holds the live observation bags of every currently registered thread, keyed by thread,
/// plus archives of observation bags inherited from threads that have terminated. Each archive
/// contains at most one configuration for an event name, allowing incompatible configurations to
/// remain visible to report collection without making thread-local destruction panic.
#[derive(Debug)]
struct GlobalObservationBagsState {
    // Each thread has a separate map so registration and teardown can locate its entries without
    // placing synchronization on the observation hot path.
    thread_observation_bags: HashMap<ThreadId, RwLock<ObservationBagMap>>,

    // If a thread is unregistered, its observations are merged into a compatible archive.
    // It is normal for threads to go away, but this must not cause data loss: observations
    // made on past threads remain valid until the end of the process.
    archived_observation_bags: Vec<ObservationBagMap>,
}

impl GlobalEventRegistry {
    pub(crate) fn new() -> Self {
        Self {
            state: RwLock::new(GlobalObservationBagsState {
                thread_observation_bags: HashMap::default(),
                archived_observation_bags: Vec::new(),
            }),
        }
    }

    fn register(
        &self,
        thread_id: ThreadId,
        name: EventName,
        observation_bag: Arc<ObservationBagSync>,
    ) {
        // A duplicate registration must panic, but the panic must happen after every lock guard
        // has been released: a panic unwinding through a held guard would poison the global lock
        // and, worse, could strike while another thread's teardown is already unwinding. The
        // registration itself is transactional: `register_core` leaves the map untouched when it
        // reports a duplicate, so no partial state escapes.
        let outcome = 'register: {
            // Most likely the thread is already registered, so we try being optimistic.
            {
                let state = self.state.read().expect(ERR_POISONED_LOCK);

                if let Some(thread_bags) = state.thread_observation_bags.get(&thread_id) {
                    break 'register register_core(name, observation_bag, thread_bags);
                }
            }

            // The thread was not registered. Let us register it now.
            let mut state = self.state.write().expect(ERR_POISONED_LOCK);

            let thread_bags = state
                .thread_observation_bags
                .entry(thread_id)
                .or_insert_with(|| RwLock::new(HashMap::default()));

            register_core(name, observation_bag, thread_bags)
        };

        if let Err(name) = outcome {
            panic!("duplicate registration of event {name} for thread {thread_id:?}");
        }
    }

    fn unregister_thread(&self, thread_id: ThreadId) {
        // This runs from `LocalEventRegistry::drop`, which may itself be unwinding from a panic.
        // It must therefore neither panic nor poison the global lock. A poisoned lock still holds
        // structurally valid data here (writers only ever complete whole map operations), so we
        // recover the guard rather than propagate. The compatibility check below keeps `merge_from`
        // off its panicking path, so no store here can panic.
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);

        let Some(removed_bags) = state.thread_observation_bags.remove(&thread_id) else {
            return;
        };

        // Take ownership of the removed map so we no longer hold the inner lock while updating the
        // archive. It is normal for a thread to go away, but this must not cause data loss:
        // observations made on past threads remain valid until the end of the process.
        let removed = removed_bags
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);

        for (name, observation_bag) in removed {
            Self::archive_observation_bag(&mut state, name, observation_bag);
        }
    }

    /// Returns the number of events currently registered for the given thread.
    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn thread_event_count(&self, thread_id: ThreadId) -> usize {
        let state = self.state.read().unwrap();

        state
            .thread_observation_bags
            .get(&thread_id)
            .map_or(0, |thread_bags| thread_bags.read().unwrap().len())
    }

    /// Inspects all known observation bags via callback, including those
    /// containing archived data from threads that no longer exist.
    ///
    /// The callback may be called any number of times (including zero) and each call may provide
    /// data for any nonempty set of events (with no, partial or full overlap between events
    /// inspected in different calls).
    ///
    /// This takes read locks, so the callback must not attempt to perform any operations
    /// that may want to register new events, under threat of deadlock.
    pub(crate) fn inspect(&self, mut f: impl FnMut(&ObservationBagMap)) {
        let state = self.state.read().expect(ERR_POISONED_LOCK);

        for thread_bags in state.thread_observation_bags.values() {
            let bags = thread_bags.read().expect(ERR_POISONED_LOCK);

            // We do not want to make a useless callback for an empty map but we know that these
            // maps are lazy-registered, so we know that if it exists, it is non-empty.
            f(&bags);
        }

        for archived_bags in &state.archived_observation_bags {
            f(archived_bags);
        }
    }

    /// Archives one observation bag without panicking on incompatible event configurations.
    ///
    /// Compatible observations are merged to keep the archive compact. An incompatible bag is
    /// retained in a separate map, so report collection can enforce its documented configuration
    /// contract after all destructor-held locks have been released.
    fn archive_observation_bag(
        state: &mut GlobalObservationBagsState,
        name: EventName,
        observation_bag: Arc<ObservationBagSync>,
    ) {
        let compatible_bag = state
            .archived_observation_bags
            .iter()
            .filter_map(|archive| archive.get(name.as_ref()))
            .find(|archived_bag| archived_bag.is_compatible_with(&observation_bag));

        if let Some(archived_bag) = compatible_bag {
            archived_bag.merge_from(&observation_bag);
            return;
        }

        if let Some(archive) = state
            .archived_observation_bags
            .iter_mut()
            .find(|archive| !archive.contains_key(name.as_ref()))
        {
            archive.insert(name, observation_bag);
            return;
        }

        let mut archive = HashMap::default();
        archive.insert(name, observation_bag);
        state.archived_observation_bags.push(archive);
    }
}

/// Inserts one event registration into a thread's observation bag map.
///
/// Returns `Err(name)` when `name` is already registered, handing the name back and leaving the
/// map unchanged so the caller can decide how to react without any partial mutation having
/// occurred. The caller is responsible for raising any duplicate-registration panic, and must do
/// so only after releasing every lock guard.
fn register_core(
    name: EventName,
    observation_bag: Arc<ObservationBagSync>,
    thread_bags: &RwLock<ObservationBagMap>,
) -> Result<(), EventName> {
    let mut bags = thread_bags.write().expect(ERR_POISONED_LOCK);

    if bags.contains_key(&name) {
        return Err(name);
    }

    bags.insert(name, observation_bag);
    Ok(())
}

thread_local! {
    /// The events active on the current thread.
    ///
    /// This is only accessed when creating and collecting metrics,
    /// so it is not on the hot path.
    pub(crate) static LOCAL_REGISTRY: RefCell<LocalEventRegistry<'static>>
        = RefCell::new(LocalEventRegistry::new(&GLOBAL_REGISTRY));
}

pub(crate) static GLOBAL_REGISTRY: LazyLock<GlobalEventRegistry> =
    LazyLock::new(GlobalEventRegistry::new);

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Barrier;

    use testing::{assert_panics, with_watchdog};

    use super::*;
    use crate::{Magnitude, Observations};

    const TEST_EVENT_NAME: &str = "test_event";

    #[test]
    fn unregister_unregistered_thread_is_no_op() {
        let global_registry = GlobalEventRegistry::new();

        // Get a thread ID that was never registered.
        let unregistered_thread_id = thread::current().id();

        global_registry.unregister_thread(unregistered_thread_id);

        let state = global_registry.state.read().unwrap();
        assert!(state.archived_observation_bags.is_empty());
        assert!(state.thread_observation_bags.is_empty());
    }

    #[test]
    fn register_unregister_smoke_test() {
        let observations = Arc::new(ObservationBagSync::new(&[]));

        let global_registry = GlobalEventRegistry::new();
        let local_registry = LocalEventRegistry::new(&global_registry);

        local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));

        // The global registry is the authoritative owner of the registration.
        assert_eq!(local_registry.event_count(), 1);
        assert!(
            global_registry
                .state
                .read()
                .unwrap()
                .thread_observation_bags
                .contains_key(&local_registry.thread_id)
        );
        assert!(
            global_registry
                .state
                .read()
                .unwrap()
                .thread_observation_bags
                .get(&local_registry.thread_id)
                .unwrap()
                .read()
                .unwrap()
                .contains_key(TEST_EVENT_NAME)
        );

        let thread_id = local_registry.thread_id;

        // This should unregister from the global registry, as well.
        drop(local_registry);

        assert!(
            !global_registry
                .state
                .read()
                .unwrap()
                .thread_observation_bags
                .contains_key(&thread_id)
        );
    }

    #[test]
    fn duplicate_registration_panics_without_poisoning_registry() {
        let observations = Arc::new(ObservationBagSync::new(&[]));

        let global_registry = GlobalEventRegistry::new();
        let local_registry = LocalEventRegistry::new(&global_registry);

        local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));
        assert_panics(|| {
            local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));
        });

        local_registry.register("event_after_duplicate".into(), observations);
        assert_eq!(local_registry.event_count(), 2);
    }

    #[test]
    fn inspect_global_inspects_all() {
        with_watchdog(|| {
            let thread1_observations = Arc::new(ObservationBagSync::new(&[]));

            let global_registry = GlobalEventRegistry::new();

            let thread1_local_registry = LocalEventRegistry::new(&global_registry);
            thread1_local_registry
                .register(TEST_EVENT_NAME.into(), Arc::clone(&thread1_observations));

            thread::scope(|scope| {
                // Registering on another thread creates a distinct live bag for the same event.
                scope
                    .spawn(|| {
                        let thread2_observations = Arc::new(ObservationBagSync::new(&[]));

                        let thread2_local_registry = LocalEventRegistry::new(&global_registry);
                        thread2_local_registry
                            .register(TEST_EVENT_NAME.into(), Arc::clone(&thread2_observations));

                        let mut seen_bags: usize = 0;

                        global_registry.inspect(|observation_bags| {
                            seen_bags += observation_bags.len();

                            assert!(observation_bags.contains_key(TEST_EVENT_NAME));
                            assert_eq!(observation_bags.len(), 1);
                        });

                        assert_eq!(seen_bags, 2);
                    })
                    .join()
                    .unwrap();
            });
        });
    }

    #[test]
    fn data_remains_after_thread_terminates() {
        with_watchdog(|| {
            let global_registry = GlobalEventRegistry::new();

            // Register on another thread, then verify that the event remains visible after that
            // thread terminates.
            thread::scope(|scope| {
                scope
                    .spawn(|| {
                        let observations = Arc::new(ObservationBagSync::new(&[]));

                        let local_registry = LocalEventRegistry::new(&global_registry);
                        local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));
                    })
                    .join()
                    .unwrap();
            });

            let mut seen_bags: usize = 0;

            global_registry.inspect(|observation_bags| {
                seen_bags += observation_bags.len();

                assert!(observation_bags.contains_key(TEST_EVENT_NAME));
                assert_eq!(observation_bags.len(), 1);
            });

            assert_eq!(seen_bags, 1);
        });
    }

    #[test]
    fn concurrent_registration_is_visible_and_survives_teardown() {
        with_watchdog(|| {
            // The barrier keeps every registration live until the inspection is complete and
            // releases all workers together, exercising contended registration and teardown.
            const THREADS: usize = 4;

            let global_registry = GlobalEventRegistry::new();
            let checkpoint = Barrier::new(THREADS + 1);

            thread::scope(|scope| {
                for index in 0..THREADS {
                    let global_registry = &global_registry;
                    let checkpoint = &checkpoint;
                    scope.spawn(move || {
                        let observations = Arc::new(ObservationBagSync::new(&[]));
                        let local_registry = LocalEventRegistry::new(global_registry);

                        local_registry
                            .register(format!("concurrent_event_{index}").into(), observations);

                        checkpoint.wait();
                        checkpoint.wait();
                    });
                }

                checkpoint.wait();

                let mut live_events: usize = 0;
                global_registry.inspect(|observation_bags| {
                    live_events += observation_bags.len();
                });
                assert_eq!(live_events, THREADS);

                checkpoint.wait();
            });

            let mut archived_events: usize = 0;
            global_registry.inspect(|observation_bags| {
                archived_events += observation_bags.len();
            });
            assert_eq!(archived_events, THREADS);
        });
    }

    #[test]
    fn incompatible_registrations_do_not_merge_on_teardown() {
        with_watchdog(|| {
            // Incompatible configurations remain distinct in the archive so report collection can
            // enforce its contract without risking a panic in a destructor.
            const EVENT_NAME: &str = "shared_event";
            const FIRST_BUCKETS: &[Magnitude] = &[10];
            const SECOND_BUCKETS: &[Magnitude] = &[10, 20, 30];

            let global_registry = GlobalEventRegistry::new();

            thread::scope(|scope| {
                scope
                    .spawn(|| {
                        let observations = Arc::new(ObservationBagSync::new(FIRST_BUCKETS));
                        observations.insert(5, 1);

                        let local_registry = LocalEventRegistry::new(&global_registry);
                        local_registry.register(EVENT_NAME.into(), observations);
                    })
                    .join()
                    .unwrap();

                scope
                    .spawn(|| {
                        let observations = Arc::new(ObservationBagSync::new(SECOND_BUCKETS));
                        observations.insert(5, 1);

                        let local_registry = LocalEventRegistry::new(&global_registry);
                        local_registry.register(EVENT_NAME.into(), observations);
                    })
                    .join()
                    .unwrap();
            });

            let mut archived = Vec::new();
            global_registry.inspect(|observation_bags| {
                if let Some(observations) = observation_bags.get(EVENT_NAME) {
                    let snapshot = observations.snapshot();
                    archived.push((snapshot.bucket_magnitudes.to_vec(), snapshot.count));
                }
            });
            archived.sort();

            assert_eq!(
                archived,
                [(FIRST_BUCKETS.to_vec(), 1), (SECOND_BUCKETS.to_vec(), 1)]
            );
        });
    }
}
