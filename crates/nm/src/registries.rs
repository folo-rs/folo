use std::{
    cell::RefCell,
    marker::PhantomData,
    sync::{Arc, LazyLock, RwLock},
    thread::{self, ThreadId},
};

use foldhash::{HashMap, HashMapExt};

use crate::{ERR_POISONED_LOCK, EventName, ObservationBag};

type ObservationBagMap = HashMap<EventName, Arc<ObservationBag>>;

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
    fn new(global_registry: &'g GlobalEventRegistry) -> Self {
        Self {
            observation_bags: RefCell::new(HashMap::new()),
            thread_id: thread::current().id(),
            global_registry,
            _single_threaded: PhantomData,
        }
    }

    pub(crate) fn register(&self, name: EventName, observation_bag: Arc<ObservationBag>) {
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
    // We essentially add a thread level to the hierarchy maintained by each local registry.
    // The data in here is duplicated - it is not merely a list of existing registries, since
    // those are single-threaded data types and we want to minimize any locking we perform.
    thread_observation_bags: RwLock<HashMap<ThreadId, RwLock<ObservationBagMap>>>,
}

impl GlobalEventRegistry {
    fn new() -> Self {
        Self {
            thread_observation_bags: RwLock::new(HashMap::new()),
        }
    }

    fn register(&self, thread_id: ThreadId, name: EventName, observation_bag: Arc<ObservationBag>) {
        // Most likely the thread is already registered, so we try being optimistic.
        {
            let threads = self
                .thread_observation_bags
                .read()
                .expect(ERR_POISONED_LOCK);

            if let Some(thread_bags) = threads.get(&thread_id) {
                register_core(thread_id, name, observation_bag, thread_bags);
                return;
            }
        }

        // The thread was not registered. Let's register it now.
        let mut threads = self
            .thread_observation_bags
            .write()
            .expect(ERR_POISONED_LOCK);

        let thread_bags = threads
            .entry(thread_id)
            .or_insert_with(|| RwLock::new(HashMap::new()));

        register_core(thread_id, name, observation_bag, thread_bags);
    }

    fn unregister_thread(&self, thread_id: ThreadId) {
        let mut threads = self
            .thread_observation_bags
            .write()
            .expect(ERR_POISONED_LOCK);

        threads.remove(&thread_id);
    }

    /// Inspects the observation bags of all threads via a callback.
    ///
    /// This takes read locks, so the callback must not attempt to perform any operations
    /// that may want to register new events, under threat of deadlock.
    pub(crate) fn inspect(&self, mut f: impl FnMut(&ThreadId, &ObservationBagMap)) {
        let threads = self
            .thread_observation_bags
            .read()
            .expect(ERR_POISONED_LOCK);

        for (thread_id, thread_bags) in threads.iter() {
            let bags = thread_bags.read().expect(ERR_POISONED_LOCK);
            f(thread_id, &bags);
        }
    }
}

fn register_core(
    thread_id: ThreadId,
    name: EventName,
    observation_bag: Arc<ObservationBag>,
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
        let observations = Arc::new(ObservationBag::new(&[]));

        let global_registry = GlobalEventRegistry::new();
        let local_registry = LocalEventRegistry::new(&global_registry);

        local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));

        assert_eq!(local_registry.observation_bags.borrow().len(), 1);
        assert!(
            global_registry
                .thread_observation_bags
                .read()
                .expect(ERR_POISONED_LOCK)
                .contains_key(&local_registry.thread_id)
        );
        assert!(
            global_registry
                .thread_observation_bags
                .read()
                .expect(ERR_POISONED_LOCK)
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
            global_registry
                .thread_observation_bags
                .read()
                .expect(ERR_POISONED_LOCK)
                .get(&thread_id)
                .is_none()
        );
    }

    #[test]
    #[should_panic]
    fn duplicate_registration_panics() {
        let observations = Arc::new(ObservationBag::new(&[]));

        let global_registry = GlobalEventRegistry::new();
        let local_registry = LocalEventRegistry::new(&global_registry);

        local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));
        local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&observations));
    }

    #[test]
    fn inspect_global_inspects_all() {
        let thread1_observations = Arc::new(ObservationBag::new(&[]));

        let global_registry = GlobalEventRegistry::new();

        let thread1_local_registry = LocalEventRegistry::new(&global_registry);
        thread1_local_registry.register(TEST_EVENT_NAME.into(), Arc::clone(&thread1_observations));

        let thread1_id = thread::current().id();

        thread::scope(|s| {
            // Now let's switch to a new thread, register the event there, and inspect.
            // We expect to see the observation bags of both threads when inspecting.
            s.spawn(|| {
                let thread2_observations = Arc::new(ObservationBag::new(&[]));

                let thread2_local_registry = LocalEventRegistry::new(&global_registry);
                thread2_local_registry
                    .register(TEST_EVENT_NAME.into(), Arc::clone(&thread2_observations));

                let thread2_id = thread::current().id();

                let mut seen_thread_ids = Vec::new();

                global_registry.inspect(|thread_id, observation_bags| {
                    seen_thread_ids.push(*thread_id);

                    assert!(observation_bags.contains_key(TEST_EVENT_NAME));
                    assert_eq!(observation_bags.len(), 1);
                });

                assert_eq!(seen_thread_ids.len(), 2);
                assert!(seen_thread_ids.contains(&thread1_id));
                assert!(seen_thread_ids.contains(&thread2_id));
            })
            .join()
            .unwrap();
        });
    }
}
