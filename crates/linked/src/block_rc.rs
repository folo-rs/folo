// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use std::{rc::Rc, thread::LocalKey};

/// Helper type used to implement the logic behind [`linked::variable_ref!`][crate::variable_ref].
#[derive(Debug)]
pub struct VariableByRc<T>
where
    T: 'static,
{
    get_storage: fn() -> &'static LocalKey<Rc<T>>,
}

impl<T> VariableByRc<T>
where
    T: 'static,
{
    /// Note: this function exists to serve the inner workings of the `link!` macro and should not
    /// be used directly. It is not part of the public API and may be removed or changed at any time.
    #[doc(hidden)]
    pub const fn new(get_storage: fn() -> &'static LocalKey<Rc<T>>) -> Self {
        Self { get_storage }
    }

    /// Gets an `Rc` to the current thread's instance of the linked object.
    pub fn get(&self) -> Rc<T> {
        (self.get_storage)().with(Rc::clone)
    }

    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Rc<T>) -> R,
    {
        (self.get_storage)().with(f)
    }
}

/// Declares the static variables within the macro body as containing [linked objects][crate].
///
/// All instances obtained from the same variable are linked to each other, on any thread.
/// For a single thread, static variables in this macro block always access the same instance.
///
/// Call `.with()` to execute a closure with a shared reference to the linked object. This is the
/// most efficient way to use the value, although the closure style is not always appropriate.
///
/// Call `.get()` on the static variable to obtain a thread-specific linked instance of the type
/// within, wrapped in an `Rc`. This does not limit you to a closure. Every call to `.get()` returns
/// an `Rc` for the same instance on a single thread.
///
/// This macro exists to simplify usage of the linked object pattern, which in its natural form
/// requires complex wiring for efficient use on many threads. If you need dynamic multithreaded
/// storage (i.e. not a single static variable), pass instances of [`Handle<T>`][crate::Handle]
/// instead of using this macro.
///
/// # Example
///
/// ```
/// # #[linked::object]
/// # struct TokenCache { }
/// # impl TokenCache { fn with_capacity(capacity: usize) -> Self { linked::new!(Self { } ) } fn get_token(&self) -> usize { 42 } }
/// linked::variable_ref!(static TOKEN_CACHE: TokenCache = TokenCache::with_capacity(1000));
///
/// fn do_something() {
///     let token = TOKEN_CACHE.with(|cache| cache.get_token());
/// }
/// ```
#[macro_export]
macro_rules! variable_ref {
    () => {};

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr; $($rest:tt)*) => (
        ::linked::variable_ref!($(#[$attr])* $vis static $NAME: $t = $e);
        ::linked::variable_ref!($($rest)*);
    );

    ($(#[$attr:meta])* $vis:vis static $NAME:ident: $t:ty = $e:expr) => {
        ::linked::__private::paste! {
            ::linked::variable!(#[doc(hidden)] static [< $NAME _INITIALIZER >]: $t = $e;);

            thread_local!(#[doc(hidden)] static [< $NAME _RC >]: ::std::rc::Rc<$t> = ::std::rc::Rc::new([< $NAME _INITIALIZER >].get()));

            $(#[$attr])* $vis const $NAME: ::linked::VariableByRc<$t> =
                ::linked::VariableByRc::new(move || &[< $NAME _RC >]);
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
        linked::variable_ref! {
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
