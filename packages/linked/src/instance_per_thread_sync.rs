use std::collections::{HashMap, hash_map};
use std::ops::Deref;
use std::sync::{Arc, RwLock};
use std::thread::{self, ThreadId};

use simple_mermaid::mermaid;

use crate::{BuildThreadIdHasher, ERR_POISONED_LOCK};

/// A wrapper that manages linked instances of `T`, ensuring that only one
/// instance of `T` is created per thread.
///
/// Requires `T: Send + Sync`.
///
/// This is similar to the [`linked::thread_local_arc!` macro][1], with the main difference
/// being that this type operates entirely at runtime using dynamic storage and does
/// not require a static variable to be defined.
///
/// # Usage
///
/// Create an instance of `InstancePerThreadSync` and provide it the initial instance of a linked
/// object `T`. Any instance of `T` accessed through the same `InstancePerThreadSync` or a clone
/// of it will be linked to the same family.
#[ doc=mermaid!( "../doc/instance_per_thread_sync.mermaid") ]
///
/// To access the current thread's instance of `T`, you must first obtain a
/// [`RefSync<T>`][RefSync] by calling [`.acquire()`][Self::acquire]. Then you can
/// access the `T` within by simply dereferencing via the `Deref<Target = T>` trait.
///
/// `RefSync<T>` is a thread-aligned type, meaning you can move it to a different thread and even
/// access it across threads but it will still reference the shared instance of `T` from the
/// original thread.
///
/// # Resource management
///
/// A thread-specific instance of `T` is dropped when the last `RefSync` aligned to that thread
/// is dropped, similar to how `Arc<T>` would behave. If a new `RefSync` is later obtained,
/// it is initialized with a new instance of the linked object.
///
/// It is important to emphasize that this means if you only acquire temporary `RefSync`
/// instances then you will get a new instance of `T` every time. The performance impact of
/// this depends on how `T` works internally but you are recommended to keep `RefSync`
/// instances around for reuse when possible.
///
/// [1]: crate::thread_local_arc
#[derive(Debug)]
pub struct InstancePerThreadSync<T>
where
    T: linked::Object + Send + Sync,
{
    family: FamilyStateReference<T>,
}

impl<T> InstancePerThreadSync<T>
where
    T: linked::Object + Send + Sync,
{
    /// Creates a new `InstancePerThreadSync` with an existing instance of `T`.
    ///
    /// Any further access of `T` instances via the `InstancePerThreadSync` (or its clones)
    /// will return instances of `T` from the same family.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "intentional needless consume to encourage all access to go via InstancePerThreadSync<T>"
    )]
    #[must_use]
    pub fn new(inner: T) -> Self {
        let family = FamilyStateReference::new(inner.family());

        Self { family }
    }

    /// Returns a `RefSync<T>` that can be used to access the current thread's instance of `T`.
    ///
    /// Creating multiple concurrent `RefSync<T>` instances from the same `InstancePerThreadSync<T>`
    /// on the same thread is allowed. Every `RefSync<T>` instance will reference the same
    /// instance of `T` per thread.
    ///
    /// There are no constraints on the lifetime of the returned `RefSync<T>`. It is a
    /// thread-aligned type, so you can move it across threads and access it from a different
    /// thread but it will continue to reference the `T` instance of the original thread.
    ///
    /// # Example
    ///
    /// ```
    /// # use std::sync::atomic::{self, AtomicUsize};
    /// #
    /// # #[linked::object]
    /// # struct Thing {
    /// #     local_value: AtomicUsize,
    /// # }
    /// #
    /// # impl Thing {
    /// #     pub fn new() -> Self {
    /// #         linked::new!(Self { local_value: AtomicUsize::new(0) })
    /// #     }
    /// #
    /// #     pub fn increment(&self) {
    /// #        self.local_value.fetch_add(1, atomic::Ordering::Relaxed);
    /// #     }
    /// #
    /// #     pub fn local_value(&self) -> usize {
    /// #         self.local_value.load(atomic::Ordering::Relaxed)
    /// #     }
    /// # }
    /// #
    /// use linked::InstancePerThreadSync;
    ///
    /// let linked_thing = InstancePerThreadSync::new(Thing::new());
    ///
    /// let thing = linked_thing.acquire();
    /// thing.increment();
    /// assert_eq!(thing.local_value(), 1);
    /// ```
    ///
    /// # Efficiency
    ///
    /// Reuse the returned `RefSync<T>` when possible. Every call to this function has
    /// some overhead, especially if there are no other `RefSync<T>` instances from the
    /// same family active on the current thread.
    ///
    /// # Instance lifecycle
    ///
    /// A thread-specific instance of `T` is dropped when the last `RefSync` created on that
    /// thread is dropped. If a new `RefSync` is later obtained, it is initialized
    /// with a new linked instance of `T` linked to the same family as the
    /// originating `InstancePerThreadSync<T>`.
    ///
    /// ```
    /// # use std::sync::atomic::{self, AtomicUsize};
    /// #
    /// # #[linked::object]
    /// # struct Thing {
    /// #     local_value: AtomicUsize,
    /// # }
    /// #
    /// # impl Thing {
    /// #     pub fn new() -> Self {
    /// #         linked::new!(Self { local_value: AtomicUsize::new(0) })
    /// #     }
    /// #
    /// #     pub fn increment(&self) {
    /// #        self.local_value.fetch_add(1, atomic::Ordering::Relaxed);
    /// #     }
    /// #
    /// #     pub fn local_value(&self) -> usize {
    /// #         self.local_value.load(atomic::Ordering::Relaxed)
    /// #     }
    /// # }
    /// #
    /// use linked::InstancePerThreadSync;
    ///
    /// let linked_thing = InstancePerThreadSync::new(Thing::new());
    ///
    /// let thing = linked_thing.acquire();
    /// thing.increment();
    /// assert_eq!(thing.local_value(), 1);
    ///
    /// drop(thing);
    ///
    /// // Dropping the only acquired instance above will have reset the thread-local state.
    /// let thing = linked_thing.acquire();
    /// assert_eq!(thing.local_value(), 0);
    /// ```
    ///
    /// To minimize the effort spent on re-creating the thread-local state, ensure that you reuse
    /// the `RefSync<T>` instances as much as possible.
    #[must_use]
    pub fn acquire(&self) -> RefSync<T> {
        let inner = self.family.current_thread_instance();

        RefSync {
            inner,
            family: self.family.clone(),
        }
    }
}

impl<T> Clone for InstancePerThreadSync<T>
where
    T: linked::Object + Send + Sync,
{
    #[inline]
    fn clone(&self) -> Self {
        Self {
            family: self.family.clone(),
        }
    }
}

/// An acquired thread-local instance of a linked object of type `T`,
/// implementing `Deref<Target = T>`.
///
/// For details, see [`InstancePerThreadSync<T>`][InstancePerThreadSync] which is the type used
/// to create instances of `RefSync<T>`.
#[derive(Debug)]
pub struct RefSync<T>
where
    T: linked::Object + Send + Sync,
{
    // We really are just a wrapper around an Arc<T>. The only other duty we have
    // is to clean up the thread-local instance when the last `RefSync` is dropped.
    inner: Arc<T>,
    family: FamilyStateReference<T>,
}

impl<T> Deref for RefSync<T>
where
    T: linked::Object + Send + Sync,
{
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Clone for RefSync<T>
where
    T: linked::Object + Send + Sync,
{
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            family: self.family.clone(),
        }
    }
}

impl<T> Drop for RefSync<T>
where
    T: linked::Object + Send + Sync,
{
    fn drop(&mut self) {
        // If we were the last RefSync on this thread then we need to drop the thread-local
        // state for this thread. Note that there are 2 references - ourselves and the family state.
        if Arc::strong_count(&self.inner) != 2 {
            // No - there is another RefSync, so we do not need to clean up.
            return;
        }

        self.family.clear_current_thread_instance();

        // `self.inner` is now the last reference to the current thread's instance of T
        // and this instance will be dropped once this function returns and drops the last `Arc<T>`.
    }
}

/// One reference to the state of a specific family of per-thread linked objects.
/// This can be used to retrieve and/or initialize the current thread's instance.
#[derive(Debug)]
struct FamilyStateReference<T>
where
    T: linked::Object + Send + Sync,
{
    // If a thread needs a new instance, we create it via the family.
    family: linked::Family<T>,

    // The write lock here is only held when initializing the thread-specific state for a thread
    // for the first time, which should generally be rare, especially as user code will also be
    // motivated to reduce those instances because it also means initializing the actual `T` inside.
    // Most access will therefore only need to take a read lock.
    thread_specific: Arc<RwLock<HashMap<ThreadId, ThreadSpecificState<T>, BuildThreadIdHasher>>>,
}

impl<T> FamilyStateReference<T>
where
    T: linked::Object + Send + Sync,
{
    #[must_use]
    fn new(family: linked::Family<T>) -> Self {
        Self {
            family,
            thread_specific: Arc::new(RwLock::new(HashMap::with_hasher(BuildThreadIdHasher))),
        }
    }

    /// Returns the `Arc<T>` for the current thread, creating it if necessary.
    #[must_use]
    fn current_thread_instance(&self) -> Arc<T> {
        let thread_id = thread::current().id();

        // First, an optimistic pass - let us assume it is already initialized for our thread.
        {
            let map = self.thread_specific.read().expect(ERR_POISONED_LOCK);

            if let Some(state) = map.get(&thread_id) {
                return state.clone_instance();
            }
        }

        // The state for the current thread is not yet initialized. Let us initialize!
        // Note that we create this instance outside any locks, both to reduce the
        // lock durations but also because cloning a linked object may execute arbitrary code,
        // including potentially code that tries to grab the same lock.
        let instance: Arc<T> = Arc::new(self.family.clone().into());

        // Let us add the new instance to the map.
        let mut map = self.thread_specific.write().expect(ERR_POISONED_LOCK);

        // In some wild corner cases, it is perhaps possible that the arbitrary code in the
        // linked object clone logic may already have filled the map with our value? It is
        // a bit of a stretch of imagination but let us accept the possibility to be thorough.
        match map.entry(thread_id) {
            hash_map::Entry::Occupied(occupied_entry) => {
                // There already is something in the entry. That is fine, we just ignore the
                // new instance we created and pretend we are on the optimistic path.
                let state = occupied_entry.get();

                state.clone_instance()
            }
            hash_map::Entry::Vacant(vacant_entry) => {
                // We are the first thread to create an instance. Let us insert it.
                let state = ThreadSpecificState::new(Arc::clone(&instance));
                vacant_entry.insert(state);

                instance
            }
        }
    }

    fn clear_current_thread_instance(&self) {
        // We need to clear the thread-specific state for this thread.
        let thread_id = thread::current().id();

        let mut map = self.thread_specific.write().expect(ERR_POISONED_LOCK);
        map.remove(&thread_id);
    }
}

impl<T> Clone for FamilyStateReference<T>
where
    T: linked::Object + Send + Sync,
{
    fn clone(&self) -> Self {
        Self {
            family: self.family.clone(),
            thread_specific: Arc::clone(&self.thread_specific),
        }
    }
}

impl<T> Drop for FamilyStateReference<T>
where
    T: linked::Object + Send + Sync,
{
    #[cfg_attr(test, mutants::skip)] // This is just a sanity check, no functional behavior.
    fn drop(&mut self) {
        // If we are the last reference to the family state, this will drop the thread-specific map.
        // We need to ensure that the thread-specific state is empty before we drop the map.
        // This is a sanity check - if this fails, we have a defect somewhere in our code.

        if Arc::strong_count(&self.thread_specific) > 1 {
            // We are not the last reference to the family state,
            // so no state dropping will occur - having state in the map is fine.
            return;
        }

        if thread::panicking() {
            // If we are already panicking, there is no point in asserting anything,
            // as another panic may disrupt the handling of the original panic.
            return;
        }

        let map = self.thread_specific.read().expect(ERR_POISONED_LOCK);
        assert!(
            map.is_empty(),
            "thread-specific state map was not empty on drop - internal logic error"
        );
    }
}

/// Holds the thread-specific state for a specific family of per-thread linked objects.
#[derive(Debug)]
struct ThreadSpecificState<T>
where
    T: linked::Object + Send + Sync,
{
    instance: Arc<T>,
}

impl<T> ThreadSpecificState<T>
where
    T: linked::Object + Send + Sync,
{
    /// Creates a new `ThreadSpecificState` with the given `Arc<T>`.
    #[must_use]
    fn new(instance: Arc<T>) -> Self {
        Self { instance }
    }

    /// Returns the `Arc<T>` for this thread.
    #[must_use]
    fn clone_instance(&self) -> Arc<T> {
        Arc::clone(&self.instance)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::atomic::{self, AtomicUsize};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use super::*;

    #[linked::object]
    struct TokenCache {
        shared_value: Arc<Mutex<usize>>,
        local_value: AtomicUsize,
    }

    impl TokenCache {
        fn new() -> Self {
            let shared_value = Arc::new(Mutex::new(0));

            linked::new!(Self {
                shared_value: Arc::clone(&shared_value),
                local_value: AtomicUsize::new(0),
            })
        }

        fn increment(&self) {
            self.local_value.fetch_add(1, atomic::Ordering::Relaxed);

            let mut shared_value = self.shared_value.lock().unwrap();
            *shared_value = shared_value.wrapping_add(1);
        }

        fn local_value(&self) -> usize {
            self.local_value.load(atomic::Ordering::Relaxed)
        }

        fn shared_value(&self) -> usize {
            *self.shared_value.lock().unwrap()
        }
    }

    #[test]
    fn per_thread_smoke_test() {
        let linked_cache = InstancePerThreadSync::new(TokenCache::new());

        let cache1 = linked_cache.acquire();
        cache1.increment();

        assert_eq!(cache1.local_value(), 1);
        assert_eq!(cache1.shared_value(), 1);

        // This must refer to the same instance.
        let cache2 = linked_cache.acquire();

        assert_eq!(cache2.local_value(), 1);
        assert_eq!(cache2.shared_value(), 1);

        cache2.increment();

        assert_eq!(cache1.local_value(), 2);
        assert_eq!(cache1.shared_value(), 2);

        thread::spawn(move || {
            // You can move InstancePerThreadSync across threads.
            let cache3 = linked_cache.acquire();

            // This is a different thread's instance, so the local value is fresh.
            assert_eq!(cache3.local_value(), 0);
            assert_eq!(cache3.shared_value(), 2);

            cache3.increment();

            assert_eq!(cache3.local_value(), 1);
            assert_eq!(cache3.shared_value(), 3);

            // You can clone this and every clone works the same.
            let thread_aligned_clone = linked_cache.clone();

            let cache4 = thread_aligned_clone.acquire();

            assert_eq!(cache4.local_value(), 1);
            assert_eq!(cache4.shared_value(), 3);

            // Every InstancePerThreadSync instance from the same family is equivalent.
            let cache5 = linked_cache.acquire();

            assert_eq!(cache5.local_value(), 1);
            assert_eq!(cache5.shared_value(), 3);

            thread::spawn(move || {
                let cache6 = thread_aligned_clone.acquire();

                // This is a different thread's instance, so the local value is fresh.
                assert_eq!(cache6.local_value(), 0);
                assert_eq!(cache6.shared_value(), 3);

                cache6.increment();

                assert_eq!(cache6.local_value(), 1);
                assert_eq!(cache6.shared_value(), 4);
            })
            .join()
            .unwrap();
        })
        .join()
        .unwrap();

        assert_eq!(cache1.local_value(), 2);
        assert_eq!(cache1.shared_value(), 4);
    }

    #[test]
    fn thread_state_dropped_on_last_thread_aligned_drop() {
        let linked_cache = InstancePerThreadSync::new(TokenCache::new());

        let cache = linked_cache.acquire();
        cache.increment();

        assert_eq!(cache.local_value(), 1);

        // This will drop the local state.
        drop(cache);

        // We get a fresh instance now, initialized from scratch for this thread.
        let cache = linked_cache.acquire();
        assert_eq!(cache.local_value(), 0);
    }

    #[test]
    fn thread_state_dropped_on_thread_exit() {
        // At the start, no thread-specific state has been created. The link embedded into the
        // InstancePerThreadSync holds one reference to the inner shared value of the TokenCache.
        let linked_cache = InstancePerThreadSync::new(TokenCache::new());

        let cache = linked_cache.acquire();

        // We now have two references to the inner shared value - the link + this fn.
        assert_eq!(Arc::strong_count(&cache.shared_value), 2);

        thread::spawn(move || {
            let cache = linked_cache.acquire();

            assert_eq!(Arc::strong_count(&cache.shared_value), 3);
        })
        .join()
        .unwrap();

        // Should be back to 2 here - the thread-local state was dropped when the thread exited.
        assert_eq!(Arc::strong_count(&cache.shared_value), 2);
    }
}
