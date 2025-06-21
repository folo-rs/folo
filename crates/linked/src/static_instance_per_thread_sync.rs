// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use std::sync::Arc;
use std::thread::LocalKey;

/// This is the real type of variables wrapped in the [`linked::thread_local_arc!` macro][1].
/// See macro documentation for more details.
///
/// Instances of this type are created by the [`linked::thread_local_arc!` macro][1],
/// never directly by user code, which can call `.with()` or `.to_arc()`
/// to work with or obtain a linked instance of `T`.
///
/// [1]: [crate::thread_local_arc]
#[derive(Debug)]
pub struct StaticInstancePerThreadSync<T>
where
    T: linked::Object + Send + Sync,
{
    get_storage: fn() -> &'static LocalKey<Arc<T>>,
}

impl<T> StaticInstancePerThreadSync<T>
where
    T: linked::Object + Send + Sync,
{
    /// Note: this function exists to serve the inner workings of the
    /// `linked::thread_local_arc!` macro and should not be used directly.
    /// It is not part of the public API and may be removed or changed at any time.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(get_storage: fn() -> &'static LocalKey<Arc<T>>) -> Self {
        Self { get_storage }
    }

    /// Executes a closure with the current thread's linked instance from
    /// the object family referenced by the static variable.
    ///
    /// # Performance
    ///
    /// For repeated access to the current thread's linked instance, prefer reusing an `Arc<T>`
    /// obtained from `.to_arc()`.
    ///
    /// If your code is not in a situation where it can reuse an existing `Arc<T>`, this method is
    /// the optimal way to access the current thread's linked instance of `T`.
    #[inline]
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Arc<T>) -> R,
    {
        (self.get_storage)().with(f)
    }

    /// Gets an `Arc<T>` to the current thread's linked instance from
    /// the object family referenced by the static variable.
    ///
    /// The instance behind this `Arc` is the same one accessed by all other calls through the static
    /// variable on this thread. Note that it is still possible to create multiple instances on a
    /// single thread, e.g. by cloning the `T` within. The "one instance per thread" logic only
    /// applies when the instances are accessed through the static variable.
    ///
    /// # Performance
    ///
    /// This function merely clones an `Arc`, which is relatively fast but still more work than
    /// doing nothing. The most efficient way to access the current thread's linked instance is
    /// to reuse the `Arc<T>` returned from this method.
    ///
    /// If you are not in a situation where you can reuse the `Arc<T>` and a shared reference is
    /// satisfactory, prefer calling [`.with()`][Self::with] instead, which does not create an
    /// `Arc` and thereby saves a few nanoseconds.
    #[must_use]
    #[inline]
    pub fn to_arc(&self) -> Arc<T> {
        (self.get_storage)().with(Arc::clone)
    }
}

/// Declares that all static variables within the macro body
/// contain thread-local [linked objects][crate].
///
/// A single instance from the same family is maintained per-thread, represented
/// as an `Arc<T>`. This implies `T: Send + Sync`.
///
/// Call [`.with()`][2] to execute a closure with a shared reference to the current thread's
/// instance of the linked object. This is the most efficient way to use the static variable,
/// although the closure style can sometimes be cumbersome.
///
/// Call [`.to_arc()`][1] on the static variable to obtain a thread-specific linked instance
/// of the object, wrapped in an `Arc`. This does not limit you to a closure. Every [`.to_arc()`][1]
/// call returns an `Arc` for the same instance per thread (though you may clone the `T` inside to
/// create additional instances not governed by the mechanics of this macro).
///
/// # Accessing linked instances
///
/// If you are making multiple calls from the same thread, prefer calling [`.to_arc()`][1] and
/// reusing the returned `Arc<T>` for optimal performance.
///
/// If you are not making multiple calls, you may yield optimal performance by calling
/// [`.with()`][2] to execute a closure with a shared reference to the current thread's
/// linked instance.
///
/// # Example
///
/// ```
/// # #[linked::object]
/// # struct TokenCache { }
/// # impl TokenCache { fn with_capacity(capacity: usize) -> Self { linked::new!(Self { } ) } fn get_token(&self) -> usize { 42 } }
/// linked::thread_local_arc!(static TOKEN_CACHE: TokenCache = TokenCache::with_capacity(1000));
///
/// fn do_something() {
///     // `.with()` is the most efficient way to access the instance of the current thread.
///     let token = TOKEN_CACHE.with(|cache| cache.get_token());
/// }
/// ```
///
/// # Dynamic family relationships
///
/// If you need fully `Arc`-style dynamic storage (i.e. not a single static variable) then consider
/// either passing instances of [`InstancePerThreadSync<T>`][6] between threads or using
/// [`Family`][3] to manually control instance creation.
///
/// # Cross-thread usage
///
/// While you can pass the `Arc` returned by [`.to_arc()`][1] between threads, the object within
/// will typically (depending on implementation choices) remain aligned to the original thread it
/// was created on, which may lead to suboptimal performance if you try to use it on a different
/// thread. For optimal multithreaded behavior, call `.with()` or `.to_arc()` on the thread the
/// instance will be used on.
///
/// [1]: StaticInstancePerThreadSync::to_arc
/// [2]: StaticInstancePerThreadSync::with
/// [3]: crate::Family
/// [5]: crate::Object
/// [6]: crate::InstancePerThreadSync
#[macro_export]
macro_rules! thread_local_arc {
    () => {};

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr; $($rest:tt)*) => (
        $crate::thread_local_arc!($(#[$attr])* $vis static $NAME: $t = $e);
        $crate::thread_local_arc!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr) => {
        $crate::__private::paste! {
            $crate::instances!(#[doc(hidden)] static [< $NAME _INITIALIZER >]: $t = $e;);

            ::std::thread_local!(#[doc(hidden)] static [< $NAME _ARC >]: ::std::sync::Arc<$t> = ::std::sync::Arc::new([< $NAME _INITIALIZER >].get()));

            $(#[$attr])* $vis const $NAME: $crate::StaticInstancePerThreadSync<$t> =
                $crate::StaticInstancePerThreadSync::new(move || &[< $NAME _ARC >]);
        }
    };
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{self, AtomicUsize};
    use std::thread;

    #[linked::object]
    struct TokenCache {
        local_value: AtomicUsize,
    }

    impl TokenCache {
        fn new(value: usize) -> Self {
            linked::new!(Self {
                local_value: AtomicUsize::new(value)
            })
        }

        fn value(&self) -> usize {
            self.local_value.load(atomic::Ordering::Relaxed)
        }

        fn increment(&self) {
            self.local_value.fetch_add(1, atomic::Ordering::Relaxed);
        }
    }

    #[test]
    fn smoke_test() {
        linked::thread_local_arc! {
            static BLUE_TOKEN_CACHE: TokenCache = TokenCache::new(1000);
            static YELLOW_TOKEN_CACHE: TokenCache = TokenCache::new(2000);
        }

        assert_eq!(BLUE_TOKEN_CACHE.to_arc().value(), 1000);
        assert_eq!(YELLOW_TOKEN_CACHE.to_arc().value(), 2000);

        BLUE_TOKEN_CACHE.with(|cache| {
            assert_eq!(cache.value(), 1000);
        });
        YELLOW_TOKEN_CACHE.with(|cache| {
            assert_eq!(cache.value(), 2000);
        });

        BLUE_TOKEN_CACHE.to_arc().increment();
        YELLOW_TOKEN_CACHE.to_arc().increment();

        assert_eq!(BLUE_TOKEN_CACHE.to_arc().value(), 1001);
        assert_eq!(YELLOW_TOKEN_CACHE.to_arc().value(), 2001);

        // Another thread gets instances aligned to the other thread.
        thread::spawn(move || {
            assert_eq!(BLUE_TOKEN_CACHE.to_arc().value(), 1000);
            assert_eq!(YELLOW_TOKEN_CACHE.to_arc().value(), 2000);

            BLUE_TOKEN_CACHE.to_arc().increment();
            YELLOW_TOKEN_CACHE.to_arc().increment();

            assert_eq!(BLUE_TOKEN_CACHE.to_arc().value(), 1001);
            assert_eq!(YELLOW_TOKEN_CACHE.to_arc().value(), 2001);
        })
        .join()
        .unwrap();

        assert_eq!(BLUE_TOKEN_CACHE.to_arc().value(), 1001);
        assert_eq!(YELLOW_TOKEN_CACHE.to_arc().value(), 2001);

        // We can move instances to other threads and they stay aligned to the original thread.
        let blue_cache = BLUE_TOKEN_CACHE.to_arc();
        let yellow_cache = YELLOW_TOKEN_CACHE.to_arc();

        thread::spawn(move || {
            assert_eq!(blue_cache.value(), 1001);
            assert_eq!(yellow_cache.value(), 2001);

            blue_cache.increment();
            yellow_cache.increment();

            assert_eq!(blue_cache.value(), 1002);
            assert_eq!(yellow_cache.value(), 2002);
        })
        .join()
        .unwrap();
    }
}
