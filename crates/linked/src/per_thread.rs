use std::{
    collections::{HashMap, hash_map},
    ops::Deref,
    rc::Rc,
    sync::{Arc, RwLock},
    thread::{self, ThreadId},
};

use crate::ERR_POISONED_LOCK;

/// A wrapper that manages instances of linked objects of type `T`, ensuring that only one
/// instance of `T` is created per thread.
///
/// This is a conceptual equivalent of the [`linked::instance_per_thread!` macro][1], with the main
/// difference being that this type is a runtime wrapper that does not require a static variable.
///
/// # Usage
///
/// Create an instance of `PerThread` and provide it the initial instance of `T` from which other
/// instances in the same family are to be created. Any instance of `T` retrieved through the same
/// `PerThread` or a clone will be linked to the same family of `T`.
///
/// To access the current thread's `T`, you must first obtain a [`ThreadLocal<T>`][ThreadLocal]
/// which works in a similar manner to `Rc<T>`, allowing you to reference the value within. This
/// is done by calling [`PerThread::local()`][Self::local].
///
/// Once you have a [`ThreadLocal<T>`][ThreadLocal], you can access the `T` within by simply
/// dereferencing it via the `Deref<Target = T>` trait.
///
/// # Storage considerations
///
/// Note that the `ThreadLocal` is `!Send`, which means you cannot store it in places that need to
/// be thread-mobile. For example, in web framework request handlers the compiler might not permit
/// you to let a `ThreadLocal` live across an `await`, depending on the web framework.
///
/// Using this type only makes sense if you can store the `ThreadLocal` and reuse it, as creating
/// one is relatively expensive. If you need to create a new `ThreadLocal` every time, you may be
/// better off using alternative mechanisms.
///
/// # Resource management
///
/// Thread-specific state is dropped when the last `ThreadLocal` on that thread is dropped. If a
/// new `ThreadLocal` is later obtained, it is initialized with a new instance of the linked object.
///
/// # Advanced scenarios
///
/// Note that use of `PerThread` does not close the door on other ways to use linked objects.
/// For example, you always have the possibility of manually taking the `T` and creating
/// additional clones of it to break out the one-per-thread limitation. The `PerThread` type
/// only controls what happens through the `PerThread` type.
///
/// [1]: crate::instance_per_thread
#[derive(Debug)]
pub struct PerThread<T>
where
    T: linked::Object + 'static,
{
    family: FamilyStateReference<T>,
}

impl<T> PerThread<T>
where
    T: linked::Object + 'static,
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
    T: linked::Object + 'static,
{
    fn clone(&self) -> Self {
        Self {
            family: self.family.clone(),
        }
    }
}

#[derive(Debug)]
pub struct ThreadLocal<T>
where
    T: linked::Object + 'static,
{
    inner: Rc<T>,
    family: FamilyStateReference<T>,
}

impl<T> Deref for ThreadLocal<T>
where
    T: linked::Object + 'static,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Clone for ThreadLocal<T>
where
    T: linked::Object + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
            family: self.family.clone(),
        }
    }
}

impl<T> Drop for ThreadLocal<T>
where
    T: linked::Object + 'static,
{
    fn drop(&mut self) {
        // If we were the last ThreadLocal on this thread then we need to drop the thread-local
        // state for this thread. Note that there are 2 references - ourselves and the family state.
        if Rc::strong_count(&self.inner) != 2 {
            // No - there is another ThreadLocal, so we do not need to clean up.
            return;
        }

        // We are protected against reentrancy here because remember that there are still TWO
        // references - we are holding the last one until we return from this fn, so even if
        // there is a field of another ThreadLocal that is shortly going to be dropped, it will
        // do so after this block here.
        self.family.clear_current_thread_instance();

        // We are the last reference to the thread-local value in `inner`,
        // which will be dropped once this function returns.
    }
}

/// One reference to the state of a specific family of per-thread linked objects.
/// This can be used to retrieve and/or initialize the current thread's instance.
#[derive(Debug)]
struct FamilyStateReference<T>
where
    T: linked::Object + 'static,
{
    // If a thread needs a new instance, we create it via this handle.
    handle: linked::Handle<T>,

    // We store the state of each thread here. See safety comments on ThreadSpecificState!
    // NB! While it is legal to manipulate the HashMap from any thread, accessing a value
    // is only permitted from the correct thread.
    //
    // To ensure safety, we must also ensure that all values are removed from here before the map
    // is dropped (because each value must be removed and dropped from the thread that created it).
    // This is done in the `ThreadLocal` destructor. By the time this map is dropped, it must be
    // empty (which we verify in our own drop()).
    // TODO: This locking is a bit ugly but if ThreadLocal is cached, perhaps fine. Still, benchmark
    // and compare against a thread-local HashMap shared between all PerThread instances?
    thread_specific: Arc<RwLock<HashMap<ThreadId, ThreadSpecificState<T>>>>,
}

impl<T> FamilyStateReference<T>
where
    T: linked::Object + 'static,
{
    fn new(handle: linked::Handle<T>) -> Self {
        Self {
            handle,
            thread_specific: Arc::new(RwLock::new(HashMap::new())),
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
        // Note that we create this instance outside any locks, both to avoid unnecessary
        // synchronization with other threads but also because cloning a linked object
        // may execute arbitrary code, including potentially code that tries to grab the same lock.
        let instance: Rc<T> = Rc::new(self.handle.clone().into());

        // Let's add the new instance to the map.
        let mut map = self.thread_specific.write().expect(ERR_POISONED_LOCK);

        // In some wild corner cases, it is perhaps possible that the arbitrary code in the
        // linked object clone logic may already have filled the map with our value? It is
        // a bit of a stretch of imagination but let's acknowledge the possibility just in case.
        match map.entry(thread_id) {
            hash_map::Entry::Occupied(occupied_entry) => {
                let state = occupied_entry.get();

                // SAFETY: We must guarantee that we are on the thread that owns
                // the thread-specific state. We are - thread ID lookup led us here.
                unsafe { state.clone_instance() }
            }
            hash_map::Entry::Vacant(vacant_entry) => {
                // We are the first thread to create an instance. Let's insert it.
                let state = ThreadSpecificState {
                    instance: Rc::clone(&instance),
                };
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
    T: linked::Object + 'static,
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
    T: linked::Object + 'static,
{
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
/// potentially even touched (moved) from another thread when resizing the HashMap of all
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
    T: linked::Object + 'static,
{
    instance: Rc<T>,
}

impl<T> ThreadSpecificState<T>
where
    T: linked::Object + 'static,
{
    /// Returns the `Rc<T>` for this thread.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the current thread is the thread for which this
    /// `ThreadSpecificState` was created. This is not enforced by the type system.
    unsafe fn clone_instance(&self) -> Rc<T> {
        Rc::clone(&self.instance)
    }
}

// SAFETY: See comments on type.
unsafe impl<T> Sync for ThreadSpecificState<T> where T: linked::Object + 'static {}
// SAFETY: See comments on type.
unsafe impl<T> Send for ThreadSpecificState<T> where T: linked::Object + 'static {}

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
            self.local_value.set(self.local_value.get() + 1);

            let mut shared_value = self.shared_value.lock().unwrap();
            *shared_value += 1;
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
