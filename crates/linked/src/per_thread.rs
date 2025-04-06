use std::{
    collections::{HashMap, hash_map},
    ops::Deref,
    rc::Rc,
    sync::{Arc, RwLock},
    thread::{self, ThreadId},
};

use simple_mermaid::mermaid;

use crate::{BuildThreadIdHasher, ERR_POISONED_LOCK};

/// A wrapper that manages instances of linked objects of type `T`, ensuring that only one
/// instance of `T` is created per thread.
///
/// This is a conceptual equivalent of the [`linked::instance_per_thread!` macro][1], with the main
/// difference being that this type operates entirely at runtime using dynamic storage and does
/// not require a static variable to be defined.
///
/// # Usage
///
/// Create an instance of `PerThread` and provide it an initial instance of a linked object `T`.
/// This initial instance will be used to create additional instances on demand. Any instance
/// of `T` retrieved through the same `PerThread` or a clone will be linked to the same family
/// of `T` instances.
///
#[ doc=mermaid!( "../doc/per_thread.mermaid") ]
///
/// To access the current thread's instance of `T`, you must first obtain a
/// [`ThreadLocal<T>`][ThreadLocal] which works in a manner similar to `Rc<T>`, allowing you to
/// reference the value within. You can obtain a [`ThreadLocal<T>`][ThreadLocal] by calling
/// [`PerThread::local()`][Self::local].
///
/// Once you have a [`ThreadLocal<T>`][ThreadLocal], you can access the `T` within by simply
/// dereferencing via the `Deref<Target = T>` trait.
///
/// # Long-lived thread-specific instances
///
/// Note that the `ThreadLocal` type is `!Send`, which means you cannot store it in places that
/// need to be thread-mobile. For example, in web framework request handlers the compiler might
/// not permit you to let a `ThreadLocal` live across an `await`, depending on the web framework,
/// the async task runtime used and its specific configuration.
///
/// # Resource management
///
/// A thread-specific instance of `T` is dropped when the last `ThreadLocal` on that thread is
/// dropped. If a new `ThreadLocal` is later obtained, it is initialized with a new instance
/// of the linked object.
///
/// It is important to emphasize that this means if you only create temporary `ThreadLocal`
/// instances then you will get a new instance of `T` every time. The performance impact of
/// this depends on how `T` works internally but you are recommended to keep `ThreadLocal`
/// instances around for reuse when possible.
///
/// # Advanced scenarios
///
/// Use of `PerThread` does not close the door on other ways to use linked objects.
/// For example, you always have the possibility of manually taking the `T` and creating
/// additional clones of it to break out the one-per-thread limitation. The `PerThread` type
/// only controls what happens through the `PerThread` type.
///
/// [1]: crate::instance_per_thread
#[derive(Debug)]
pub struct PerThread<T>
where
    T: linked::Object,
{
    family: FamilyStateReference<T>,
}

impl<T> PerThread<T>
where
    T: linked::Object,
{
    /// Creates a new `PerThread` with an existing instance of `T`. Any further access to the `T`
    /// via the `PerThread` (or its clones) will return instances of `T` from the same family.
    pub fn new(inner: T) -> Self {
        let family = FamilyStateReference::new(inner.handle());

        Self { family }
    }

    /// Returns a `ThreadLocal<T>` that can be used to efficiently access the current
    /// thread's `T` instance.
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
    /// let per_thread_thing = linked::PerThread::new(Thing::new());
    ///
    /// let local_thing = per_thread_thing.local();
    /// local_thing.increment();
    /// assert_eq!(local_thing.local_value(), 1);
    /// ```
    ///
    /// # Efficiency
    ///
    /// Reuse the returned instance as much as possible. Every call to this function has some
    /// overhead, especially if there are no other `ThreadLocal<T>` instances from the same family
    /// active on the current thread.
    ///
    /// # Instance lifecycle
    ///
    /// A thread-specific instance of `T` is dropped when the last `ThreadLocal` on that thread is
    /// dropped. If a new `ThreadLocal` is later obtained, it is initialized with a new instance
    /// of the linked object.
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
    /// let per_thread_thing = linked::PerThread::new(Thing::new());
    ///
    /// let local_thing = per_thread_thing.local();
    /// local_thing.increment();
    /// assert_eq!(local_thing.local_value(), 1);
    ///
    /// drop(local_thing);
    ///
    /// // Dropping the only thread-local instance above will have reset the thread-local state.
    /// let local_thing = per_thread_thing.local();
    /// assert_eq!(local_thing.local_value(), 0);
    /// ```
    ///
    /// To minimize the effort spent on re-creating the thread-local state, ensure that you reuse
    /// the `ThreadLocal<T>` instances as much as possible.
    ///
    /// # Thread safety
    ///
    /// The returned value is single-threaded and cannot be moved or used across threads. For
    /// transfer across threads, you need to preserve and share/send a `PerThread<T>` instance.
    pub fn local(&self) -> ThreadLocal<T> {
        let inner = self.family.current_thread_instance();

        ThreadLocal {
            inner,
            family: self.family.clone(),
        }
    }
}

impl<T> Clone for PerThread<T>
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

/// A thread-local instance of a linked object of type `T`. This acts in a manner similar to
/// `Rc<T>` for a type `T` that implements the [linked object pattern][crate].
///
/// For details, see [`PerThread<T>`][PerThread] which is the type used to create instances
/// of `ThreadLocal<T>`.
#[derive(Debug)]
pub struct ThreadLocal<T>
where
    T: linked::Object,
{
    // We really are just a wrapper around an Rc<T>. The only other duty we have
    // is to clean up the thread-local instance when the last ThreadLocal is dropped.
    inner: Rc<T>,
    family: FamilyStateReference<T>,
}

impl<T> Deref for ThreadLocal<T>
where
    T: linked::Object,
{
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Clone for ThreadLocal<T>
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

impl<T> Drop for ThreadLocal<T>
where
    T: linked::Object,
{
    fn drop(&mut self) {
        // If we were the last ThreadLocal on this thread then we need to drop the thread-local
        // state for this thread. Note that there are 2 references - ourselves and the family state.
        if Rc::strong_count(&self.inner) != 2 {
            // No - there is another ThreadLocal, so we do not need to clean up.
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
    // If a thread needs a new instance, we create it via this handle.
    handle: linked::Handle<T>,

    // We store the state of each thread here. See safety comments on ThreadSpecificState!
    // NB! While it is legal to manipulate the HashMap from any thread, including to move
    // the values, calling actual functions on a value is only valid from the thread in the key.
    //
    // To ensure safety, we must also ensure that all values are removed from here before the map
    // is dropped, because each value must be dropped on the thread that created it and dropping is
    // logic executed on that thread-specific value!
    //
    // This is done in the `ThreadLocal` destructor. By the time this map is dropped, it must be
    // empty, which we assert in our own drop().
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
    fn new(handle: linked::Handle<T>) -> Self {
        Self {
            handle,
            thread_specific: Arc::new(RwLock::new(HashMap::with_hasher(BuildThreadIdHasher))),
        }
    }

    /// Returns the `Rc<T>` for the current thread, creating it if necessary.
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
        let instance: Rc<T> = Rc::new(self.handle.clone().into());

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
            handle: self.handle.clone(),
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
    unsafe fn clone_instance(&self) -> Rc<T> {
        Rc::clone(&self.instance)
    }
}

// SAFETY: See comments on type.
unsafe impl<T> Sync for ThreadSpecificState<T> where T: linked::Object {}
// SAFETY: See comments on type.
unsafe impl<T> Send for ThreadSpecificState<T> where T: linked::Object {}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        sync::{Arc, Mutex},
        thread,
    };

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
        let per_thread = PerThread::new(TokenCache::new());

        let thread_local1 = per_thread.local();
        thread_local1.increment();

        assert_eq!(thread_local1.local_value(), 1);
        assert_eq!(thread_local1.shared_value(), 1);

        // This must refer to the same instance.
        let thread_local2 = per_thread.local();

        assert_eq!(thread_local2.local_value(), 1);
        assert_eq!(thread_local2.shared_value(), 1);

        thread_local2.increment();

        assert_eq!(thread_local1.local_value(), 2);
        assert_eq!(thread_local1.shared_value(), 2);

        thread::spawn(move || {
            // You can move PerThread across threads.
            let thread_local3 = per_thread.local();

            // This is a different thread's instance, so the local value is fresh.
            assert_eq!(thread_local3.local_value(), 0);
            assert_eq!(thread_local3.shared_value(), 2);

            thread_local3.increment();

            assert_eq!(thread_local3.local_value(), 1);
            assert_eq!(thread_local3.shared_value(), 3);

            // You can clone this and every clone works the same.
            let per_thread_clone = per_thread.clone();

            let thread_local4 = per_thread_clone.local();

            assert_eq!(thread_local4.local_value(), 1);
            assert_eq!(thread_local4.shared_value(), 3);

            thread::spawn(move || {
                let thread_local5 = per_thread_clone.local();

                // This is a different thread's instance, so the local value is fresh.
                assert_eq!(thread_local5.local_value(), 0);
                assert_eq!(thread_local5.shared_value(), 3);

                thread_local5.increment();

                assert_eq!(thread_local5.local_value(), 1);
                assert_eq!(thread_local5.shared_value(), 4);
            })
            .join()
            .unwrap();
        })
        .join()
        .unwrap();

        assert_eq!(thread_local1.local_value(), 2);
        assert_eq!(thread_local1.shared_value(), 4);
    }

    #[test]
    fn thread_state_dropped_on_last_thread_local_drop() {
        let per_thread = PerThread::new(TokenCache::new());

        let local = per_thread.local();
        local.increment();

        assert_eq!(local.local_value(), 1);

        // This will drop the local state.
        drop(local);

        // We get a fresh instance now, initialized from scratch for this thread.
        let local = per_thread.local();
        assert_eq!(local.local_value(), 0);
    }

    #[test]
    fn thread_state_dropped_on_thread_exit() {
        // At the start, no thread-specific state has been created. The link embedded into the
        // PerThread holds one reference to the inner shared value of the TokenCache.
        let per_thread = PerThread::new(TokenCache::new());

        let local = per_thread.local();

        // We now have two references to the inner shared value - the link + this fn.
        assert_eq!(Arc::strong_count(&local.shared_value), 2);

        let per_thread_clone = per_thread.clone();

        thread::spawn(move || {
            let local = per_thread_clone.local();

            assert_eq!(Arc::strong_count(&local.shared_value), 3);
        })
        .join()
        .unwrap();

        // Should be back to 2 here - the thread-local state was dropped when the thread exited.
        assert_eq!(Arc::strong_count(&local.shared_value), 2);
    }
}
