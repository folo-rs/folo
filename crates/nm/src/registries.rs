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
pub(crate) struct LocalEventRegistry {
    observation_bags: RefCell<ObservationBagMap>,
    thread_id: ThreadId,

    _single_threaded: PhantomData<*const ()>,
}

impl LocalEventRegistry {
    fn new() -> Self {
        Self {
            observation_bags: RefCell::new(HashMap::new()),
            thread_id: thread::current().id(),
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

        GLOBAL_REGISTRY.register(self.thread_id, name, observation_bag);
    }
}

impl Drop for LocalEventRegistry {
    fn drop(&mut self) {
        GLOBAL_REGISTRY.unregister_thread(thread::current().id());
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
    pub(crate) static LOCAL_REGISTRY: RefCell<LocalEventRegistry> = RefCell::new(LocalEventRegistry::new());
}

pub(crate) static GLOBAL_REGISTRY: LazyLock<GlobalEventRegistry> =
    LazyLock::new(GlobalEventRegistry::new);
