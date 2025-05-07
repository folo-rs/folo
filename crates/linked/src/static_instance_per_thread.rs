// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use std::{rc::Rc, thread::LocalKey};

/// This is the real type of variables wrapped in the [`linked::thread_local_rc!` macro][1].
/// See macro documentation for more details.
///
/// Instances of this type are created by the [`linked::thread_local_rc!` macro][1],
/// never directly by user code, which can call `.with()` or `.to_rc()`
/// to work with or obtain a linked instance of `T`.
///
/// [1]: [crate::thread_local_rc]
#[derive(Debug)]
pub struct StaticInstancePerThread<T>
where
    T: linked::Object,
{
    get_storage: fn() -> &'static LocalKey<Rc<T>>,
}

impl<T> StaticInstancePerThread<T>
where
    T: linked::Object,
{
    /// Note: this function exists to serve the inner workings of the
    /// `linked::thread_local_rc!` macro and should not be used directly.
    /// It is not part of the public API and may be removed or changed at any time.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(get_storage: fn() -> &'static LocalKey<Rc<T>>) -> Self {
        Self { get_storage }
    }

    /// Executes a closure with the current thread's linked instance from
    /// the object family referenced by the static variable.
    ///
    /// # Performance
    ///
    /// For repeated access to the current thread's linked instance, prefer reusing an `Rc<T>`
    /// obtained from `.to_rc()`.
    ///
    /// If your code is not in a situation where it can reuse an existing `Rc<T>`, this method is
    /// the optimal way to access the current thread's linked instance of `T`.
    #[inline]
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Rc<T>) -> R,
    {
        (self.get_storage)().with(f)
    }

    /// Gets an `Rc<T>` to the current thread's linked instance from
    /// the object family referenced by the static variable.
    ///
    /// The instance behind this `Rc` is the same one accessed by all other calls through the static
    /// variable on this thread. Note that it is still possible to create multiple instances on a
    /// single thread, e.g. by cloning the `T` within. The "one instance per thread" logic only
    /// applies when the instances are accessed through the static variable.
    ///
    /// # Performance
    ///
    /// This function merely clones an `Rc`, which is relatively fast but still more work than
    /// doing nothing. The most efficient way to access the current thread's linked instance is
    /// to reuse the `Rc<T>` returned from this method.
    ///
    /// If you are not in a situation where you can reuse the `Rc<T>` and a shared reference is
    /// satisfactory, prefer calling [`.with()`][Self::with] instead, which does not create an
    /// `Rc` and thereby saves a few nanoseconds.
    #[must_use]
    #[inline]
    pub fn to_rc(&self) -> Rc<T> {
        (self.get_storage)().with(Rc::clone)
    }
}

/// Declares that all static variables within the macro body contain thread-local
/// [linked objects][crate].
///
/// A single instance from the same family is maintained per-thread, represented
/// as an `Rc<T>`.
///
/// Call [`.with()`][2] to execute a closure with a shared reference to the current thread's
/// instance of the linked object. This is the most efficient way to use the static variable,
/// although the closure style can sometimes be cumbersome.
///
/// Call [`.to_rc()`][1] on the static variable to obtain a thread-specific linked instance
/// of the object, wrapped in an `Rc`. This does not limit you to a closure. Every [`.to_rc()`][1]
/// call returns an `Rc` for the same instance per thread (though you may clone the `T` inside to
/// create additional instances not governed by the mechanics of this macro).
///
/// # Accessing linked instances
///
/// If you are making multiple calls from the same thread, prefer calling [`.to_rc()`][1] and
/// reusing the returned `Rc<T>` for optimal performance.
///
/// If you are not making multiple calls or are not in a situation where you can store an `Rc<T>`,
/// you may yield optimal performance by calling [`.with()`][2] to execute a closure with a shared
/// reference to the current thread's linked instance.
///
/// # Example
///
/// ```
/// # #[linked::object]
/// # struct TokenCache { }
/// # impl TokenCache { fn with_capacity(capacity: usize) -> Self { linked::new!(Self { } ) } fn get_token(&self) -> usize { 42 } }
/// linked::thread_local_rc!(static TOKEN_CACHE: TokenCache = TokenCache::with_capacity(1000));
///
/// fn do_something() {
///     // `.with()` is the most efficient way to access the instance of the current thread.
///     let token = TOKEN_CACHE.with(|cache| cache.get_token());
/// }
/// ```
/// 
/// # Dynamic family relationships
///
/// If you need fully `Rc`-style dynamic storage (i.e. not a single static variable) then consider
/// either passing instances of [`InstancePerThread<T>`][4] between threads or using
/// [`Family`][3] to manually control instance creation.
///
/// [1]: StaticInstancePerThread::to_rc
/// [2]: StaticInstancePerThread::with
/// [3]: crate::Family
/// [4]: crate::InstancePerThread
#[macro_export]
macro_rules! thread_local_rc {
    () => {};

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr; $($rest:tt)*) => (
        $crate::thread_local_rc!($(#[$attr])* $vis static $NAME: $t = $e);
        $crate::thread_local_rc!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr) => {
        $crate::__private::paste! {
            $crate::instances!(#[doc(hidden)] static [< $NAME _INITIALIZER >]: $t = $e;);

            ::std::thread_local!(#[doc(hidden)] static [< $NAME _RC >]: ::std::rc::Rc<$t> = ::std::rc::Rc::new([< $NAME _INITIALIZER >].get()));

            $(#[$attr])* $vis const $NAME: $crate::StaticInstancePerThread<$t> =
                $crate::StaticInstancePerThread::new(move || &[< $NAME _RC >]);
        }
    };
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, thread};

    #[linked::object]
    struct TokenCache {
        local_value: Cell<usize>,
    }

    impl TokenCache {
        fn new(value: usize) -> Self {
            linked::new!(Self {
                local_value: Cell::new(value)
            })
        }

        fn value(&self) -> usize {
            self.local_value.get()
        }

        fn increment(&self) {
            self.local_value
                .set(self.local_value.get().saturating_add(1));
        }
    }

    #[test]
    fn smoke_test() {
        linked::thread_local_rc! {
            static BLUE_TOKEN_CACHE: TokenCache = TokenCache::new(1000);
            static YELLOW_TOKEN_CACHE: TokenCache = TokenCache::new(2000);
        }

        assert_eq!(BLUE_TOKEN_CACHE.to_rc().value(), 1000);
        assert_eq!(YELLOW_TOKEN_CACHE.to_rc().value(), 2000);

        BLUE_TOKEN_CACHE.with(|cache| {
            assert_eq!(cache.value(), 1000);
        });
        YELLOW_TOKEN_CACHE.with(|cache| {
            assert_eq!(cache.value(), 2000);
        });

        BLUE_TOKEN_CACHE.to_rc().increment();
        YELLOW_TOKEN_CACHE.to_rc().increment();

        assert_eq!(BLUE_TOKEN_CACHE.to_rc().value(), 1001);
        assert_eq!(YELLOW_TOKEN_CACHE.to_rc().value(), 2001);

        thread::spawn(move || {
            assert_eq!(BLUE_TOKEN_CACHE.to_rc().value(), 1000);
            assert_eq!(YELLOW_TOKEN_CACHE.to_rc().value(), 2000);

            BLUE_TOKEN_CACHE.to_rc().increment();
            YELLOW_TOKEN_CACHE.to_rc().increment();

            assert_eq!(BLUE_TOKEN_CACHE.to_rc().value(), 1001);
            assert_eq!(YELLOW_TOKEN_CACHE.to_rc().value(), 2001);
        })
        .join()
        .unwrap();

        assert_eq!(BLUE_TOKEN_CACHE.to_rc().value(), 1001);
        assert_eq!(YELLOW_TOKEN_CACHE.to_rc().value(), 2001);
    }
}
