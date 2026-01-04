// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use std::any::type_name;
use std::fmt::{self, Debug, Formatter};

use crate::__private::{InstanceFactory, Link};

/// Represents a family of [linked objects][crate] and allows you to create additional instances
/// in the same family.
///
/// Clones represent the same family and are functionally equivalent.
///
/// # When to use this type
///
/// The family is a low-level primitive for creating instances of linked objects. You will need to
/// use it directly if you are implementing custom instance management patterns. Typical usage of
/// linked objects occurs via standard macros/wrappers provided by the package:
///
/// * [`linked::instances!`][1]
/// * [`linked::thread_local_rc!`][2]
/// * [`linked::thread_local_arc!`][4] (if `T: Sync`)
/// * [`linked::InstancePerThread<T>`][5]
/// * [`linked::InstancePerThreadSync<T>`][6] (if `T: Sync`)
///
/// # Example
///
/// ```rust
/// # use std::sync::{Arc, Mutex};
/// # #[linked::object]
/// # struct Thing {
/// #     value: Arc<Mutex<String>>,
/// # }
/// # impl Thing {
/// #     pub fn new(initial_value: String) -> Self {
/// #         let shared_value = Arc::new(Mutex::new(initial_value));
/// #         linked::new!(Self {
/// #             value: shared_value.clone(),
/// #         })
/// #     }
/// #     pub fn value(&self) -> String {
/// #         self.value.lock().unwrap().clone()
/// #     }
/// #     pub fn set_value(&self, value: String) {
/// #         *self.value.lock().unwrap() = value;
/// #     }
/// # }
/// use std::thread;
///
/// use linked::Object; // This brings .family() into scope.
///
/// let thing = Thing::new("hello".to_string());
/// assert_eq!(thing.value(), "hello");
///
/// thing.set_value("world".to_string());
///
/// thread::spawn({
///     let thing_family = thing.family();
///
///     move || {
///         let thing: Thing = thing_family.into();
///         assert_eq!(thing.value(), "world");
///     }
/// })
/// .join()
/// .unwrap();
/// ```
///
/// [1]: crate::instances
/// [2]: crate::thread_local_rc
/// [4]: crate::thread_local_arc
/// [5]: crate::InstancePerThread
/// [6]: crate::InstancePerThreadSync
#[derive(Clone)]
pub struct Family<T> {
    // For the family, we extract the factory from the `Link` because the `Link` is not thread-safe.
    // In other words, a `Link` exists only in interactions with a specific instance of `T`.
    instance_factory: InstanceFactory<T>,
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T> Debug for Family<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field(
                "instance_factory",
                &format_args!("Arc<dyn Fn(Link<{t}>) -> {t}>", t = type_name::<T>()),
            )
            .finish()
    }
}

impl<T> Family<T> {
    #[must_use]
    pub(crate) fn new(link: Link<T>) -> Self {
        Self {
            instance_factory: link.instance_factory,
        }
    }

    // Implementation of `From<Family<T>> for T`, called from macro-generated code for a specific T.
    #[doc(hidden)]
    #[inline]
    #[must_use]
    pub fn __private_into(self) -> T {
        Link::new(self.instance_factory).into_instance()
    }
}
