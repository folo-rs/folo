use std::collections::{HashMap, hash_map};
use std::ops::Deref;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::thread::{self, ThreadId};

use simple_mermaid::mermaid;

use crate::{BuildThreadIdHasher, ERR_POISONED_LOCK};

/// A wrapper that manages linked instances of `T`, ensuring that only one
/// instance of `T` is created per thread.
///
/// This is similar to the [`linked::thread_local_rc!` macro][1], with the main difference
/// being that this type operates entirely at runtime using dynamic storage and does
/// not require a static variable to be defined.
///
/// # Usage
///
/// Create an instance of `InstancePerThread` and provide it the initial instance of a linked
/// object `T`. Any instance of `T` accessed through the same `InstancePerThread` or a clone of it
/// will be linked to the same family.
#[ doc=mermaid!( "../doc/instance_per_thread.mermaid") ]
///
/// To access the current thread's instance of `T`, you must first obtain a
/// [`Ref<T>`][Ref] by calling [`.acquire()`][Self::acquire]. Then you can
/// access the `T` within by simply dereferencing via the `Deref<Target = T>` trait.
///
/// `Ref<T>` is a thread-isolated type, meaning you cannot move it to a different
/// thread nor access it from a different thread. To access linked instances on other threads,
/// you must transfer the `InstancePerThread<T>` instance across threads and obtain a new `Ref<T>`
/// on the destination thread.
///
/// # Resource management
///
/// A thread-specific instance of `T` is dropped when the last `Ref` on that thread
/// is dropped, similar to how `Rc<T>` would behave. If a new `Ref` is later obtained,
/// it is initialized with a new instance of the linked object.
///
/// It is important to emphasize that this means if you only acquire temporary `Ref`
/// objects then you will get a new instance of `T` every time. The performance impact of
/// this depends on how `T` works internally but you are recommended to keep `Ref`
/// instances around for reuse when possible.
///
/// # `Ref` storage
///
/// `Ref` is a thread-isolated type, which means you cannot store it in places that require types
/// to be thread-mobile. For example, in web framework request handlers the compiler might not
/// permit you to let a `Ref` live across an `await`, depending on the web framework, the async
/// task runtime used and its specific configuration.
///
/// Consider using `InstancePerThreadSync<T>` if you need a thread-mobile variant of `Ref`.
///
/// [1]: crate::thread_local_rc
#[derive(Debug)]
pub struct InstancePerThread<T>
where
    T: linked::Object,
{
    family: FamilyStateReference<T>,
}

impl<T> InstancePerThread<T>
where
    T: linked::Object,
{
    /// Creates a new `InstancePerThread` with an existing instance of `T`.
    ///
    /// Any further access of `T` instances via the `InstancePerThread` (or its clones) will return
    /// instances of `T` from the same family.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "intentional needless consume to encourage all access to go via InstancePerThread<T>"
    )]
    #[must_use]
    pub fn new(inner: T) -> Self {
        let family = FamilyStateReference::new(inner.family());

        Self { family }
    }

    /// Returns a `Ref<T>` that can be used to access the current thread's instance of `T`.
    ///
    /// Creating multiple concurrent `Ref<T>` instances from the same `InstancePerThread<T>`
    /// on the same thread is allowed. Every `Ref<T>` instance will reference the same
    /// instance of `T` per thread.
    ///
    /// There are no constraints on the lifetime of the returned `Ref<T>` but it is a
    /// thread-isolated type and cannot be moved across threads or accessed from a different thread,
    /// which may impose some limits.
    ///
    /// # Example
    ///
    /// ```
    /// # use std::cell::Cell;
    /// #
    /// # #[linked::object]
    /// # struct Thing {
    /// #     local_value: Cell<usize>,
    /// # }
    /// #
    /// # impl Thing {
    /// #     pub fn new() -> Self {
    /// #         linked::new!(Self { local_value: Cell::new(0) })
    /// #     }
    /// #
    /// #     pub fn increment(&self) {
    /// #        self.local_value.set(self.local_value.get() + 1);
    /// #     }
    /// #
    /// #     pub fn local_value(&self) -> usize {
    /// #         self.local_value.get()
    /// #     }
    /// # }
    /// #
    /// use linked::InstancePerThread;
    ///
    /// let linked_thing = InstancePerThread::new(Thing::new());
    ///
    /// let thing = linked_thing.acquire();
    /// thing.increment();
    /// assert_eq!(thing.local_value(), 1);
    /// ```
    ///
    /// # Efficiency
    ///
    /// Reuse the returned `Ref<T>` when possible. Every call to this function has
    /// some overhead, especially if there are no other `Ref<T>` instances from the
    /// same family active on the current thread.
    ///
    /// # Instance lifecycle
    ///
    /// A thread-specific instance of `T` is dropped when the last `Ref` on that
    /// thread is dropped. If a new `Ref` is later obtained, it is initialized
    /// with a new linked instance of `T` linked to the same family as the
    /// originating `InstancePerThread<T>`.
    ///
    /// ```
    /// # use std::cell::Cell;
    /// #
    /// # #[linked::object]
    /// # struct Thing {
    /// #     local_value: Cell<usize>,
    /// # }
    /// #
    /// # impl Thing {
    /// #     pub fn new() -> Self {
    /// #         linked::new!(Self { local_value: Cell::new(0) })
    /// #     }
    /// #
    /// #     pub fn increment(&self) {
    /// #        self.local_value.set(self.local_value.get() + 1);
    /// #     }
    /// #
    /// #     pub fn local_value(&self) -> usize {
    /// #         self.local_value.get()
    /// #     }
    /// # }
    /// #
    /// use linked::InstancePerThread;
    ///
    /// let linked_thing = InstancePerThread::new(Thing::new());
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
    /// the `Ref<T>` instances as much as possible.
    ///
    /// # Thread safety
    ///
    /// The returned value is single-threaded and cannot be moved or used across threads. To extend
    /// the linked object family across threads, transfer `InstancePerThread<T>` instances across threads.
    /// You can obtain additional `InstancePerThread<T>` instances by cloning the original. Every clone
    /// is equivalent.
    #[must_use]
    pub fn acquire(&self) -> Ref<T> {
        let inner = self.family.current_thread_instance();

        Ref {
            inner,
            family: self.family.clone(),
        }
    }
}

impl<T> Clone for InstancePerThread<T>
where
    T: linked::Object,
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
/// For details, see [`InstancePerThread<T>`][InstancePerThread] which is the type used
/// to create instances of `Ref<T>`.
#[derive(Debug)]
pub struct Ref<T>
where
    T: linked::Object,
{
    // We really are just a wrapper around an Rc<T>. The only other duty we have
    // is to clean up the thread-local instance when the last `Ref` is dropped.
    inner: Rc<T>,
    family: FamilyStateReference<T>,
}

impl<T> Deref for Ref<T>
where
    T: linked::Object,
{
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Clone for Ref<T>
where
    T: linked::Object,
{
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
            family: self.family.clone(),
        }
    }
}

impl<T> Drop for Ref<T>
where
    T: linked::Object,
{
    fn drop(&mut self) {
        // If we were the last Ref on this thread then we need to drop the thread-local
        // state for this thread. Note that there are 2 references - ourselves and the family state.
        if Rc::strong_count(&self.inner) != 2 {
            // No - there is another Ref, so we do not need to clean up.
            return;
        }

        self.family.clear_current_thread_instance();

        // `self.inner` is now the last reference to the current thread's instance of T
        // and this instance will be dropped once this function returns and drops the last `Rc<T>`.
    }
}

/// One reference to the state of a specific family of per-thread linked objects.
/// This can be used to retrieve and/or initialize the current thread's instance.
#[derive(Debug)]
struct FamilyStateReference<T>
where
    T: linked::Object,
{
    // If a thread needs a new instance, we create it via the family.
    family: linked::Family<T>,

    // We store the state of each thread here. See safety comments on ThreadSpecificState!
    // NB! While it is legal to manipulate the HashMap from any thread, including to move
    // the values, calling actual functions on a value is only valid from the thread in the key.
    //
    // To ensure safety, we must also ensure that all values are removed from here before the map
    // is dropped, because each value must be dropped on the thread that created it and dropping is
    // logic executed on that thread-specific value!
    //
    // This is done in the `Ref` destructor. By the time this map is dropped,
    // it must be empty, which we assert in our own drop().
    //
    // The write lock here is only held when initializing the thread-specific state for a thread
    // for the first time, which should generally be rare, especially as user code will also be
    // motivated to reduce those instances because it also means initializing the actual `T` inside.
    // Most access will therefore only need to take a read lock.
    thread_specific: Arc<RwLock<HashMap<ThreadId, ThreadSpecificState<T>, BuildThreadIdHasher>>>,
}

impl<T> FamilyStateReference<T>
where
    T: linked::Object,
{
    #[must_use]
    fn new(family: linked::Family<T>) -> Self {
        Self {
            family,
            thread_specific: Arc::new(RwLock::new(HashMap::with_hasher(BuildThreadIdHasher))),
        }
    }

    /// Returns the `Rc<T>` for the current thread, creating it if necessary.
    #[must_use]
    fn current_thread_instance(&self) -> Rc<T> {
        let thread_id = thread::current().id();

        // First, an optimistic pass - let's assume it is already initialized for our thread.
        {
            let map = self.thread_specific.read().expect(ERR_POISONED_LOCK);

            if let Some(state) = map.get(&thread_id) {
                // SAFETY: We must guarantee that we are on the thread that owns
                // the thread-specific state. We are - thread ID lookup led us here.
                return unsafe { state.clone_instance() };
            }
        }

        // The state for the current thread is not yet initialized. Let's initialize!
        // Note that we create this instance outside any locks, both to reduce the
        // lock durations but also because cloning a linked object may execute arbitrary code,
        // including potentially code that tries to grab the same lock.
        let instance: Rc<T> = Rc::new(self.family.clone().into());

        // Let's add the new instance to the map.
        let mut map = self.thread_specific.write().expect(ERR_POISONED_LOCK);

        // In some wild corner cases, it is perhaps possible that the arbitrary code in the
        // linked object clone logic may already have filled the map with our value? It is
        // a bit of a stretch of imagination but let's accept the possibility to be thorough.
        match map.entry(thread_id) {
            hash_map::Entry::Occupied(occupied_entry) => {
                // There already is something in the entry. That's fine, we just ignore the
                // new instance we created and pretend we are on the optimistic path.
                let state = occupied_entry.get();

                // SAFETY: We must guarantee that we are on the thread that owns
                // the thread-specific state. We are - thread ID lookup led us here.
                unsafe { state.clone_instance() }
            }
            hash_map::Entry::Vacant(vacant_entry) => {
                // We are the first thread to create an instance. Let's insert it.
                // SAFETY: We must guarantee that any further access (taking the Rc or dropping)
                // takes place on the same thread as was used to call this function. We ensure this
                // by the thread ID lookup in the map key - we can only ever directly access map
                // entries owned by the current thread (though we may resize the map from any
                // thread, as it simply moves data in memory).
                let state = unsafe { ThreadSpecificState::new(Rc::clone(&instance)) };
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
    T: linked::Object,
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
    T: linked::Object,
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
///
/// # Safety
///
/// This contains an `Rc`, which is `!Send` and only meant to be accessed from the thread it was
/// created on. Yet the instance of this type itself is visible from multiple threads and
/// potentially even touched (moved) from another thread when resizing the `HashMap` of all
/// instances! How can this be?!
///
/// We take advantage of the fact that an `Rc` is merely a reference to a control block.
/// As long as we never touch the control block from the wrong thread, nobody will ever
/// know we touched the `Rc` from another thread. This allows us to move the Rc around
/// in memory as long as the move itself is synchronized.
///
/// Obviously, this relies on `Rc` implementation details, so we are somewhat at risk of
/// breakage if a future Rust std implementation changes the way `Rc` works but this seems
/// unlikely as this is fairly fundamental to the nature of how smart pointers are created.
///
/// NB! We must not drop the Rc (and by extension this type) from a foreign thread!
#[derive(Debug)]
struct ThreadSpecificState<T>
where
    T: linked::Object,
{
    instance: Rc<T>,
}

impl<T> ThreadSpecificState<T>
where
    T: linked::Object,
{
    /// Creates a new `ThreadSpecificState` with the given `Rc<T>`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that any further access (including dropping) takes place on the
    /// same thread as was used to call this function.
    ///
    /// See type-level safety comments for details.
    #[must_use]
    unsafe fn new(instance: Rc<T>) -> Self {
        Self { instance }
    }

    /// Returns the `Rc<T>` for this thread.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the current thread is the thread for which this
    /// `ThreadSpecificState` was created. This is not enforced by the type system.
    ///
    /// See type-level safety comments for details.
    #[must_use]
    unsafe fn clone_instance(&self) -> Rc<T> {
        Rc::clone(&self.instance)
    }
}

// SAFETY: See comments on type.
unsafe impl<T> Sync for ThreadSpecificState<T> where T: linked::Object {}
// SAFETY: See comments on type.
unsafe impl<T> Send for ThreadSpecificState<T> where T: linked::Object {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use super::*;

    #[linked::object]
    struct TokenCache {
        shared_value: Arc<Mutex<usize>>,
        local_value: Cell<usize>,
    }

    impl TokenCache {
        fn new() -> Self {
            let shared_value = Arc::new(Mutex::new(0));

            linked::new!(Self {
                shared_value: Arc::clone(&shared_value),
                local_value: Cell::new(0),
            })
        }

        fn increment(&self) {
            self.local_value.set(self.local_value.get().wrapping_add(1));

            let mut shared_value = self.shared_value.lock().unwrap();
            *shared_value = shared_value.wrapping_add(1);
        }

        fn local_value(&self) -> usize {
            self.local_value.get()
        }

        fn shared_value(&self) -> usize {
            *self.shared_value.lock().unwrap()
        }
    }

    #[test]
    fn per_thread_smoke_test() {
        let linked_cache = InstancePerThread::new(TokenCache::new());

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
            // You can move InstancePerThread across threads.
            let cache3 = linked_cache.acquire();

            // This is a different thread's instance, so the local value is fresh.
            assert_eq!(cache3.local_value(), 0);
            assert_eq!(cache3.shared_value(), 2);

            cache3.increment();

            assert_eq!(cache3.local_value(), 1);
            assert_eq!(cache3.shared_value(), 3);

            // You can clone this and every clone works the same.
            let thread_local_clone = linked_cache.clone();

            let cache4 = thread_local_clone.acquire();

            assert_eq!(cache4.local_value(), 1);
            assert_eq!(cache4.shared_value(), 3);

            // Every InstancePerThread instance from the same family is equivalent.
            let cache5 = linked_cache.acquire();

            assert_eq!(cache5.local_value(), 1);
            assert_eq!(cache5.shared_value(), 3);

            thread::spawn(move || {
                let cache6 = thread_local_clone.acquire();

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
    fn thread_state_dropped_on_last_thread_local_drop() {
        let linked_cache = InstancePerThread::new(TokenCache::new());

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
        // InstancePerThread holds one reference to the inner shared value of the TokenCache.
        let linked_cache = InstancePerThread::new(TokenCache::new());

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
