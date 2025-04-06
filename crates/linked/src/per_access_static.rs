// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::hash_map;
use std::sync::{LazyLock, RwLock};

use hash_hasher::HashedMap;

use crate::{ERR_POISONED_LOCK, Handle};

/// This is the real type of variables wrapped in the [`linked::instance_per_access!` macro][1].
/// See macro documentation for more details.
///
/// Instances of this type are created by the [`linked::instance_per_access!` macro][1],
/// never directly by user code. User code will simply call `.get()` on this type when it
/// wants to obtain an instance of the linked object `T`.
///
/// [1]: [crate::instance_per_access]
#[derive(Debug)]
pub struct PerAccessStatic<T>
where
    T: linked::Object,
{
    /// A function we can call to obtain the lookup key for the family of linked objects.
    ///
    /// The family key is a `TypeId` because the expectation is that a unique empty type is
    /// generated for each static variable in a `linked::instance_per_access!` block, to be used
    /// as the family key for all instances linked to each other via that static variable.
    family_key_provider: fn() -> TypeId,

    /// Used to create the first instance of the linked object family. May be called
    /// concurrently multiple times due to optimistic concurrency control, so it must
    /// be idempotent and returned instances must be functionally equivalent. Even though
    /// this may be called multiple times, only one return value will ever be exposed to
    /// user code, with the others being dropped shortly after creation.
    first_instance_provider: fn() -> T,
}

impl<T> PerAccessStatic<T>
where
    T: linked::Object,
{
    /// Note: this function exists to serve the inner workings of the
    /// `linked::instance_per_access!` macro and should not be used directly.
    /// It is not part of the public API and may be removed or changed at any time.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        family_key_provider: fn() -> TypeId,
        first_instance_provider: fn() -> T,
    ) -> Self {
        Self {
            family_key_provider,
            first_instance_provider,
        }
    }

    /// Gets a new `T` instance from the family of linked objects.
    ///
    /// # Performance
    ///
    /// This creates a new instance of `T` on every call so caching the return value is
    /// performance-critical.
    ///
    /// Consider using the [`linked::instance_per_thread!` macro][1] if you want to
    /// maintain only one instance per thread and return shared references to it.
    ///
    /// [1]: [crate::instance_per_thread]
    #[must_use]
    pub fn get(&self) -> T {
        if let Some(instance) = self.new_from_local_registry() {
            return instance;
        }

        // TODO: This global registry step feels too smeared out.
        // Can we draw it together into one step under one lock?
        self.try_initialize_global_registry(self.first_instance_provider);

        let handle = self
            .get_handle_global()
            .expect("we just initialized it, the handle must exist");

        self.set_local(handle);

        // We can now be certain the local registry has the value.
        self.new_from_local_registry()
            .expect("we just set the value, it must be there")
    }

    fn set_local(&self, value: Handle<T>) {
        LOCAL_REGISTRY.with(|local_registry| {
            local_registry
                .borrow_mut()
                .insert((self.family_key_provider)(), Box::new(value));
        });
    }

    fn get_handle_global(&self) -> Option<Handle<T>> {
        GLOBAL_REGISTRY
            .read()
            .expect(ERR_POISONED_LOCK)
            .get(&(self.family_key_provider)())
            .and_then(|w| w.downcast_ref::<Handle<T>>())
            .cloned()
    }

    /// Attempts to register the object family in the global registry (if not already registered).
    ///
    /// This may use the provided provider to create a "first" instance of `T` even if another
    /// "first" instance has been created already - optimistic concurrency is used to throw
    /// away the extra instance if this proves necessary. Type-level documented requirements
    /// give us the right to do this - the "first" instances must be functionally equivalent.
    ///
    /// # Performance
    ///
    /// This is a slightly expensive operation, so should be avoided on the hot path. We only do
    /// this once per family per thread.
    fn try_initialize_global_registry<FIP>(&self, first_instance_provider: FIP)
    where
        FIP: FnOnce() -> T,
    {
        // We do not today make use of our right to create a "first" instance of `T` even when
        // we do not need it. This is a potential future optimization if it proves valuable.

        let mut global_registry = GLOBAL_REGISTRY.write().expect(ERR_POISONED_LOCK);

        // TODO: We are repeatedly acquiring the family key here and in sibling functions.
        // Perhaps a trivial cost but explore the value of eliminating the duplicate access.
        let family_key = (self.family_key_provider)();
        let entry = global_registry.entry(family_key);

        match entry {
            hash_map::Entry::Occupied(_) => (),
            hash_map::Entry::Vacant(entry) => {
                // TODO: We create an instance here, only to immediately transform it back to
                // a handle. Can we skip the middle step and just create a handle?
                let first_instance = first_instance_provider();
                entry.insert(Box::new(first_instance.handle()));
            }
        }
    }

    // Attempts to obtain a new instance of `T` using the current thread's family registry,
    // returning `None` if the linked variable has not yet been seen by this thread and is
    // therefore not present in the local registry.
    fn new_from_local_registry(&self) -> Option<T> {
        LOCAL_REGISTRY.with_borrow(|registry| {
            let family_key = (self.family_key_provider)();

            registry
                .get(&family_key)
                .and_then(|w| w.downcast_ref::<Handle<T>>())
                .map(|handle| handle.clone().into())
        })
    }
}

/// Declares that all static variables within the macro body contain [linked objects][crate],
/// with each access to this variable returning a new instance from the same family.
///
/// Each [`.get()`][1] on an included static variable returns a new linked object instance,
/// with all instances obtained from the same static variable being part of the same family.
///
/// # Dynamic family relationships
///
/// If you need `Arc`-style dynamic multithreaded storage (i.e. not a single static variable),
/// pass instances of [`Handle<T>`][3] between threads instead of (or in addition to)
/// using this macro. You can obtain a [`Handle<T>`][3] from any linked object via the
/// [`.handle()`][4] method of the [`linked::Object` trait][5], even if the instance of the
/// linked object originally came from a static variable.
///
/// # Example
///
/// ```
/// # #[linked::object]
/// # struct TokenCache { }
/// # impl TokenCache { fn with_capacity(capacity: usize) -> Self { linked::new!(Self { } ) } fn get_token(&self) -> usize { 42 } }
/// linked::instance_per_access!(static TOKEN_CACHE: TokenCache = TokenCache::with_capacity(1000));
///
/// fn do_something() {
///     // `.get()` returns a unique instance of the linked object on every call.
///     let token_cache = TOKEN_CACHE.get();
///
///     let token = token_cache.get_token();
/// }
/// ```
///
/// [1]: PerAccessStatic::get
/// [3]: crate::Handle
/// [4]: crate::Object::handle
/// [5]: crate::Object
#[macro_export]
macro_rules! instance_per_access {
    () => {};

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr; $($rest:tt)*) => (
        $crate::instance_per_access!($(#[$attr])* $vis static $NAME: $t = $e);
        $crate::instance_per_access!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr) => {
        $crate::__private::paste! {
            #[doc(hidden)]
            #[allow(non_camel_case_types, reason = "intentionally uglified macro generated code")]
            struct [<__lookup_key_ $NAME>];

            $(#[$attr])* $vis const $NAME: $crate::PerAccessStatic<$t> =
            $crate::PerAccessStatic::new(
                    ::std::any::TypeId::of::<[<__lookup_key_ $NAME>]>,
                    move || $e);
        }
    };
}

// We use HashedMap which takes the raw value from Hash::hash() and uses it directly as the key.
// This is OK because TypeId already returns a hashed value as its raw value, no need to hash more.
// We also do not care about any hash manipulation because none of this is untrusted user input.
type HandleRegistry = HashedMap<TypeId, Box<dyn Any + Send + Sync>>;

// Global registry that is the ultimate authority where all linked variables are registered.
// The values inside are `Handle<T>` where T may be different for each entry.
static GLOBAL_REGISTRY: LazyLock<RwLock<HandleRegistry>> =
    LazyLock::new(|| RwLock::new(HandleRegistry::default()));

thread_local! {
    // Thread-local registry where we cache any linked variables that have been seen by the current
    // thread. The values inside are `Handle<T>` where T may be different for each entry.
    static LOCAL_REGISTRY: RefCell<HandleRegistry> = RefCell::new(HandleRegistry::default());
}

/// Clears all data stored in the linked variable system from the current thread's point of view.
///
/// This is intended for use in tests only. It is publicly exposed because it may need to be called
/// from integration tests and benchmarks, which cannot access private functions.
#[cfg_attr(test, mutants::skip)] // Test/bench logic, do not waste time mutating.
#[doc(hidden)]
pub fn __private_clear_linked_variables() {
    let mut global_registry = GLOBAL_REGISTRY.write().expect(ERR_POISONED_LOCK);
    global_registry.clear();

    LOCAL_REGISTRY.with(|local_registry| {
        local_registry.borrow_mut().clear();
    });
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use crate::PerAccessStatic;

    #[linked::object]
    struct TokenCache {
        value: Arc<Mutex<usize>>,
    }

    impl TokenCache {
        fn new(value: usize) -> Self {
            #[allow(
                clippy::mutex_atomic,
                reason = "inner type is placeholder, for realistic usage"
            )]
            let value = Arc::new(Mutex::new(value));

            linked::new!(Self {
                value: Arc::clone(&value),
            })
        }

        fn value(&self) -> usize {
            *self.value.lock().unwrap()
        }

        fn increment(&self) {
            let mut writer = self.value.lock().unwrap();
            *writer = writer.saturating_add(1);
        }
    }

    #[test]
    fn linked_lazy() {
        // Here we test the inner logic of the variable! macro without applying the macro.
        // There is a separate test for executing the same logic via the macro itself.
        struct RedKey;
        struct GreenKey;

        const RED_TOKEN_CACHE: PerAccessStatic<TokenCache> =
            PerAccessStatic::new(TypeId::of::<RedKey>, || TokenCache::new(42));
        const GREEN_TOKEN_CACHE: PerAccessStatic<TokenCache> =
            PerAccessStatic::new(TypeId::of::<GreenKey>, || TokenCache::new(99));

        assert_eq!(RED_TOKEN_CACHE.get().value(), 42);
        assert_eq!(GREEN_TOKEN_CACHE.get().value(), 99);

        RED_TOKEN_CACHE.get().increment();
        GREEN_TOKEN_CACHE.get().increment();

        thread::spawn(move || {
            assert_eq!(RED_TOKEN_CACHE.get().value(), 43);
            assert_eq!(GREEN_TOKEN_CACHE.get().value(), 100);

            RED_TOKEN_CACHE.get().increment();
            GREEN_TOKEN_CACHE.get().increment();
        })
        .join()
        .unwrap();

        assert_eq!(RED_TOKEN_CACHE.get().value(), 44);
        assert_eq!(GREEN_TOKEN_CACHE.get().value(), 101);
    }

    #[test]
    fn linked_smoke_test() {
        linked::instance_per_access! {
            static BLUE_TOKEN_CACHE: TokenCache = TokenCache::new(1000);
            static YELLOW_TOKEN_CACHE: TokenCache = TokenCache::new(2000);
        }

        assert_eq!(BLUE_TOKEN_CACHE.get().value(), 1000);
        assert_eq!(YELLOW_TOKEN_CACHE.get().value(), 2000);

        assert_eq!(BLUE_TOKEN_CACHE.get().clone().value(), 1000);
        assert_eq!(YELLOW_TOKEN_CACHE.get().clone().value(), 2000);

        BLUE_TOKEN_CACHE.get().increment();
        YELLOW_TOKEN_CACHE.get().increment();

        thread::spawn(move || {
            assert_eq!(BLUE_TOKEN_CACHE.get().value(), 1001);
            assert_eq!(YELLOW_TOKEN_CACHE.get().value(), 2001);

            BLUE_TOKEN_CACHE.get().increment();
            YELLOW_TOKEN_CACHE.get().increment();
        })
        .join()
        .unwrap();

        assert_eq!(BLUE_TOKEN_CACHE.get().value(), 1002);
        assert_eq!(YELLOW_TOKEN_CACHE.get().value(), 2002);
    }

    #[test]
    fn thread_local_from_linked() {
        linked::instance_per_access!(static LINKED_CACHE: TokenCache = TokenCache::new(1000));
        thread_local!(static LOCAL_CACHE: Rc<TokenCache> = Rc::new(LINKED_CACHE.get()));

        let cache = LOCAL_CACHE.with(Rc::clone);
        assert_eq!(cache.value(), 1000);
        cache.increment();

        thread::spawn(move || {
            let cache = LOCAL_CACHE.with(Rc::clone);
            assert_eq!(cache.value(), 1001);
            cache.increment();
            assert_eq!(cache.value(), 1002);
        })
        .join()
        .unwrap();

        assert_eq!(cache.value(), 1002);

        let cache_rc_clone = LOCAL_CACHE.with(Rc::clone);
        assert_eq!(cache_rc_clone.value(), 1002);
    }
}
