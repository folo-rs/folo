// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use std::{rc::Rc, thread::LocalKey};

/// This is the real type of variables wrapped in the [`linked::instance_per_thread!` macro][1].
/// See macro documentation for more details.
///
/// Instances of this type are created by the [`linked::instance_per_thread!` macro][1],
/// never directly by user code. User code will simply call `.with()` or `.get()` on this
/// type when it wants to work with or obtain an instance of the linked object `T`.
///
/// [1]: [crate::instance_per_thread]
#[derive(Debug)]
pub struct PerThreadProvider<T>
where
    T: 'static,
{
    get_storage: fn() -> &'static LocalKey<Rc<T>>,
}

impl<T> PerThreadProvider<T>
where
    T: 'static,
{
    /// Note: this function exists to serve the inner workings of the
    /// `linked::instance_per_thread!` macro and should not be used directly.
    /// It is not part of the public API and may be removed or changed at any time.
    #[doc(hidden)]
    pub const fn new(get_storage: fn() -> &'static LocalKey<Rc<T>>) -> Self {
        Self { get_storage }
    }

    /// Gets an `Rc` to the current thread's instance of the linked object.
    /// 
    /// # Performance
    /// 
    /// This function merely clones an `Rc`, which is relatively fast but still more work than
    /// doing nothing. If all you need is to execute some logic on the inner type `T`, you may
    /// want to use `.with()` instead, which does not create the `Rc` and saves a few nanoseconds.
    pub fn get(&self) -> Rc<T> {
        (self.get_storage)().with(Rc::clone)
    }

    /// Executes a closure with the current thread's instance of the linked object.
    /// 
    /// # Performance
    /// 
    /// This is typically the most efficient way to access the current thread's instance of the
    /// linked object.
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Rc<T>) -> R,
    {
        (self.get_storage)().with(f)
    }
}

/// Declares that all static variables within the macro body contain [linked objects][crate],
/// with a single instance from the same family maintained per-thread (at least by this
/// macro - user code may still create additional instances via cloning).
///
/// Call `.with()` to execute a closure with a shared reference to the linked object. This is the
/// most efficient way to use the value, although the closure style is not always appropriate.
///
/// Call `.get()` on the static variable to obtain a thread-specific linked instance of the type
/// within, wrapped in an `Rc`. This does not limit you to a closure. Every call to `.get()`
/// returns an `Rc` for the same instance per thread (though you may clone the `T` inside to
/// create additional instances not governed by the mechanics of this macro).
///
/// The returned instance still works the same as any linked object - you can `.clone()` it to
/// create additional linked instances without having to go through the static variables, and
/// you can obtain a handle to the linked object family via `.handle()`, which you can pass to
/// another thread to allow it to create  instances from the same object family.
///
/// # Dynamic family relationships
///
/// If you need `Arc`-style dynamic multithreaded storage (i.e. not a single static variable),
/// pass instances of [`Handle<T>`][crate::Handle] instead of (or in addition to) using this
/// macro. You can obtain a `Handle` from any linked object via the `.handle()` method, even
/// if the instance of the linked object originally came from a static variable.
///
/// # Example
///
/// ```
/// # #[linked::object]
/// # struct TokenCache { }
/// # impl TokenCache { fn with_capacity(capacity: usize) -> Self { linked::new!(Self { } ) } fn get_token(&self) -> usize { 42 } }
/// linked::instance_per_thread!(static TOKEN_CACHE: TokenCache = TokenCache::with_capacity(1000));
///
/// fn do_something() {
///     // `.with()` is the most efficient way to access the instance of the current thread.
///     let token = TOKEN_CACHE.with(|cache| cache.get_token());
/// }
/// ```
#[macro_export]
macro_rules! instance_per_thread {
    () => {};

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr; $($rest:tt)*) => (
        ::linked::instance_per_thread!($(#[$attr])* $vis static $NAME: $t = $e);
        ::linked::instance_per_thread!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr) => {
        ::linked::__private::paste! {
            ::linked::instance_per_access!(#[doc(hidden)] static [< $NAME _INITIALIZER >]: $t = $e;);

            thread_local!(#[doc(hidden)] static [< $NAME _RC >]: ::std::rc::Rc<$t> = ::std::rc::Rc::new([< $NAME _INITIALIZER >].get()));

            $(#[$attr])* $vis const $NAME: ::linked::PerThreadProvider<$t> =
                ::linked::PerThreadProvider::new(move || &[< $NAME _RC >]);
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
        linked::instance_per_thread! {
            static BLUE_TOKEN_CACHE: TokenCache = TokenCache::new(1000);
            static YELLOW_TOKEN_CACHE: TokenCache = TokenCache::new(2000);
        }

        assert_eq!(BLUE_TOKEN_CACHE.get().value(), 1000);
        assert_eq!(YELLOW_TOKEN_CACHE.get().value(), 2000);

        BLUE_TOKEN_CACHE.with(|cache| {
            assert_eq!(cache.value(), 1000);
        });
        YELLOW_TOKEN_CACHE.with(|cache| {
            assert_eq!(cache.value(), 2000);
        });

        BLUE_TOKEN_CACHE.get().increment();
        YELLOW_TOKEN_CACHE.get().increment();

        assert_eq!(BLUE_TOKEN_CACHE.get().value(), 1001);
        assert_eq!(YELLOW_TOKEN_CACHE.get().value(), 2001);

        thread::spawn(move || {
            assert_eq!(BLUE_TOKEN_CACHE.get().value(), 1000);
            assert_eq!(YELLOW_TOKEN_CACHE.get().value(), 2000);

            BLUE_TOKEN_CACHE.get().increment();
            YELLOW_TOKEN_CACHE.get().increment();

            assert_eq!(BLUE_TOKEN_CACHE.get().value(), 1001);
            assert_eq!(YELLOW_TOKEN_CACHE.get().value(), 2001);
        })
        .join()
        .unwrap();

        assert_eq!(BLUE_TOKEN_CACHE.get().value(), 1001);
        assert_eq!(YELLOW_TOKEN_CACHE.get().value(), 2001);
    }
}
