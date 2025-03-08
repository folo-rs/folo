use std::{
    any::Any,
    cell::RefCell,
    collections::{HashMap, hash_map},
    ops::Deref,
    rc::Rc,
    sync::atomic::{self, AtomicU64},
};

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
/// dereferencing the instance (e.g. `*thread_local`) via the `Deref<Target = T>` trait.
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
    handle: linked::Handle<T>,
    family_key: u64,
}

impl<T> PerThread<T>
where
    T: linked::Object + 'static,
{
    /// Creates a new `PerThread` with an existing instance of `T`. Any further access to the `T`
    /// via the `PerThread` (or its clones) will return instances of `T` from the same family.
    pub fn new(inner: T) -> Self {
        Self {
            handle: inner.handle(),
            family_key: NEXT_FAMILY_KEY.fetch_add(1, atomic::Ordering::Relaxed),
        }
    }

    /// Returns a `ThreadLocal<T>` that can be used to efficiently access the current
    /// thread's `T` instance.
    ///
    /// # Thread safety
    ///
    /// The returned value is single-threaded and cannot be moved or used across threads. For
    /// transfer across threads, you need to preserve and share/send a `PerThread<T>` instance.
    pub fn local(&self) -> ThreadLocal<T> {
        // Optimistic - if it is already present on this thread, we can just return it.
        if let Some(inner) = LOCAL_REGISTRY.with_borrow(move |registry| {
            registry.get(&self.family_key).map(move |v| {
                Rc::clone(v)
                    .downcast::<T>()
                    .expect("PerThread::local: found wrong type in Rc")
            })
        }) {
            return ThreadLocal {
                inner,
                family_key: self.family_key,
            };
        }

        // Got nothing - we need to create a new instance for the current thread.
        //
        // NB! If we need to create a new instance, we cannot be holding any RefCell borrow
        // because creating a new instance calls out into user code which may hit the same
        // RefCell again.
        let inner: Rc<T> = Rc::new(self.handle.clone().into());

        // We have to put it into the registry now. It is theoretically possible that when we were
        // creating the instance, somehow it already got created & added via some backdoor. That is
        // fine - we check and use an existing value if one already exists. We return the winner.
        let inner =
            LOCAL_REGISTRY.with_borrow_mut(move |registry| match registry.entry(self.family_key) {
                hash_map::Entry::Occupied(occupied_entry) => Rc::clone(occupied_entry.get())
                    .downcast::<T>()
                    .expect("PerThread::local: found wrong type in Rc"),
                hash_map::Entry::Vacant(vacant_entry) => {
                    let clone = Rc::clone(&inner);
                    let as_any: Rc<dyn Any> = clone;
                    vacant_entry.insert(as_any);
                    inner
                }
            });

        ThreadLocal {
            inner,
            family_key: self.family_key,
        }
    }
}

impl<T> Clone for PerThread<T>
where
    T: linked::Object + 'static,
{
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            family_key: self.family_key,
        }
    }
}

#[derive(Debug)]
pub struct ThreadLocal<T>
where
    T: linked::Object + 'static,
{
    /// All `ThreadLocal` instances for the same family share the same `T` via `Rc`.
    /// The last instance cleans up the thread-local storage when dropped. Really, this
    /// cleanup logic is more or less the only reason for `ThreadLocal` to exist.
    inner: Rc<T>,

    family_key: u64,
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
            inner: self.inner.clone(),
            family_key: self.family_key,
        }
    }
}

impl<T> Drop for ThreadLocal<T>
where
    T: linked::Object + 'static,
{
    fn drop(&mut self) {
        // The instance registry also holds a reference to this Rc, so we detect that we need to
        // clean up by checking if the reference count is 2 (us and the registry).
        if Rc::strong_count(&self.inner) != 2 {
            // No - someone else still has a reference, so we do not need to clean up.
            return;
        }

        // We are protected against reentrancy here because remember that there are still TWO
        // references - we are holding the last one until we return from this fn, so even if
        // there is a field of another ThreadLocal that is shortly going to be dropped, it will
        // do so after this block here.
        LOCAL_REGISTRY.with_borrow_mut(|registry| {
            registry.remove(&self.family_key);
        })
    }
}

static NEXT_FAMILY_KEY: AtomicU64 = AtomicU64::new(0);

type HandleRegistry = HashMap<u64, Rc<dyn Any>>;

thread_local! {
    // TODO: This is a bit of a hot shared map. What if we just replace family_key with a family_map?
    static LOCAL_REGISTRY: RefCell<HandleRegistry> = RefCell::new(HandleRegistry::default());
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        sync::{Arc, Mutex},
        thread,
    };

    use super::*;

    #[test]
    fn per_thread_smoke_test_() {
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
}
