use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::{Arc, LazyLock, RwLock};
use std::thread::{self, ThreadId};

use foldhash::{HashMap, HashMapExt};

use crate::{ERR_POISONED_LOCK, EventName, ObservationBagSync, Observations};

type ObservationBagMap = HashMap<EventName, Arc<ObservationBagSync>>;

/// Keeps track of the events registered on a single thread, for local access only.
///
/// Facilitates event registration and unregistration from the global registry. An
/// event is automatically registered globally for as long as it is registered locally.
#[derive(Debug)]
pub(crate) struct LocalEventRegistry<'g> {
    observation_bags: RefCell<ObservationBagMap>,
    thread_id: ThreadId,
    global_registry: &'g GlobalEventRegistry,

    _single_threaded: PhantomData<*const ()>,
}

impl<'g> LocalEventRegistry<'g> {
    pub(crate) fn new(global_registry: &'g GlobalEventRegistry) -> Self {
        Self {
            observation_bags: RefCell::new(HashMap::new()),
            thread_id: thread::current().id(),
            global_registry,
            _single_threaded: PhantomData,
        }
    }

    pub(crate) fn register(&self, name: EventName, observation_bag: Arc<ObservationBagSync>) {
        let previous = self
            .observation_bags
            .borrow_mut()
            .insert(name.clone(), Arc::clone(&observation_bag));

        assert!(
            previous.is_none(),
            "duplicate registration of event {name} in local registry for thread {:?}",
            self.thread_id
        );

        self.global_registry
            .register(self.thread_id, name, observation_bag);
    }

    /// The count of events registered in the local registry.
    #[cfg(test)]
    pub(crate) fn event_count(&self) -> usize {
        self.observation_bags.borrow().len()
    }
}

impl Drop for LocalEventRegistry<'_> {
    fn drop(&mut self) {
        self.global_registry
            .unregister_thread(thread::current().id());
    }
}

/// Keeps track fo the events registered on all threads.
///
/// This is typically used for collecting and reporting on metrics data from the entire process.
#[derive(Debug)]
pub(crate) struct GlobalEventRegistry {
    state: RwLock<GlobalObservationBagsState>,
}

#[derive(Debug)]
struct GlobalObservationBagsState {
    // We essentially add a thread level to the hierarchy maintained by each local registry.
    // The data in here is duplicated - it is not merely a list of existing registries, since
    // those are single-threaded data types and we want to minimize any locking we perform.
    thread_observation_bags: HashMap<ThreadId, RwLock<ObservationBagMap>>,

    // If a thread is unregistered, its observations are merged into this map. It is normal
    // for thread to go away but this should not cause data loss - observations made on
    // past threads remain valid until end of the process.
    archived_observation_bags: ObservationBagMap,
}

impl GlobalEventRegistry {
    pub(crate) fn new() -> Self {
        Self {
            state: RwLock::new(GlobalObservationBagsState {
                thread_observation_bags: HashMap::new(),
                archived_observation_bags: HashMap::new(),
            }),
        }
    }

    fn register(
        &self,
        thread_id: ThreadId,
        name: EventName,
        observation_bag: Arc<ObservationBagSync>,
    ) {
        // Most likely the thread is already registered, so we try being optimistic.
        {
            let state = self.state.read().expect(ERR_POISONED_LOCK);

            if let Some(thread_bags) = state.thread_observation_bags.get(&thread_id) {
                register_core(thread_id, name, observation_bag, thread_bags);
                return;
            }
        }

        // The thread was not registered. Let's register it now.
        let mut state = self.state.write().expect(ERR_POISONED_LOCK);

        let thread_bags = state
            .thread_observation_bags
            .entry(thread_id)
            .or_insert_with(|| RwLock::new(HashMap::new()));

        register_core(thread_id, name, observation_bag, thread_bags);
    }

    fn unregister_thread(&self, thread_id: ThreadId) {
        let mut state = self.state.write().expect(ERR_POISONED_LOCK);

        // After removing the data of the unregistered thread, we need to
        // merge its observations into the archived observation bags.
        if let Some(removed_bags) = state.thread_observation_bags.remove(&thread_id) {
            let bags = removed_bags.read().expect(ERR_POISONED_LOCK);

            for (name, observation_bag) in bags.iter() {
                let archived_bag = state
                    .archived_observation_bags
                    .entry(name.clone())
                    .or_insert_with(|| {
                        Arc::new(ObservationBagSync::new(observation_bag.bucket_magnitudes()))
                    });

                archived_bag.merge_from(observation_bag);
            }
        }
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

        if !state.archived_observation_bags.is_empty() {
            f(&state.archived_observation_bags);
        }
    }
}

fn register_core(
    thread_id: ThreadId,
    name: EventName,
    observation_bag: Arc<ObservationBagSync>,
    thread_bags: &RwLock<ObservationBagMap>,
) {
    let mut bags = thread_bags.write().expect(ERR_POISONED_LOCK);

    let previous = bags.insert(name, observation_bag);

    assert!(
        previous.is_none(),
        "duplicate event registration in local registry for thread {thread_id:?}",
    );
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
mod tests {
    use super::*;

    const TEST_EVENT_NAME: &str = "test_event";

    #[test]
    fn register_unregister_smoke_test() {
        let observations = Arc::new(ObservationBagSync::new(&[]));

        let global_registry = GlobalEventRegistry::new();
        let local_registry = LocalEventRegistry::new(&global_registry);

        local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));

        assert_eq!(local_registry.observation_bags.borrow().len(), 1);
        assert!(
            global_registry
                .state
                .read()
                .expect(ERR_POISONED_LOCK)
                .thread_observation_bags
                .contains_key(&local_registry.thread_id)
        );
        assert!(
            global_registry
                .state
                .read()
                .expect(ERR_POISONED_LOCK)
                .thread_observation_bags
                .get(&local_registry.thread_id)
                .unwrap()
                .read()
                .expect(ERR_POISONED_LOCK)
                .contains_key(TEST_EVENT_NAME)
        );

        let thread_id = local_registry.thread_id;

        // This should unregister from the global registry, as well.
        drop(local_registry);

        assert!(
            !global_registry
                .state
                .read()
                .expect(ERR_POISONED_LOCK)
                .thread_observation_bags
                .contains_key(&thread_id)
        );
    }

    #[test]
    #[should_panic]
    fn duplicate_registration_panics() {
        let observations = Arc::new(ObservationBagSync::new(&[]));

        let global_registry = GlobalEventRegistry::new();
        let local_registry = LocalEventRegistry::new(&global_registry);

        local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));
        local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));
    }

    #[test]
    fn inspect_global_inspects_all() {
        let thread1_observations = Arc::new(ObservationBagSync::new(&[]));

        let global_registry = GlobalEventRegistry::new();

        let thread1_local_registry = LocalEventRegistry::new(&global_registry);
        thread1_local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&thread1_observations));

        thread::scope(|s| {
            // Now let's switch to a new thread, register the event there, and inspect.
            // We expect to see the observation bags of both threads when inspecting.
            s.spawn(|| {
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
    }

    #[test]
    fn data_remains_after_thread_terminates() {
        let global_registry = GlobalEventRegistry::new();

        // We observe some data on another thread, then verify that we still see it on the
        // entrypoint thread once the other thread terminates.
        thread::scope(|s| {
            s.spawn(|| {
                let observations = Arc::new(ObservationBagSync::new(&[]));

                let local_registry = LocalEventRegistry::new(&global_registry);
                local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));

                // We do not actually need to observe any data, as soon as the event
                // is registered, it becomes visible in the records with a count of 0.
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
    }
}
