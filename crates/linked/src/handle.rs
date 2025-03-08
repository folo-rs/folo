// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;

use crate::__private::{InstanceFactory, Link};

/// A handle that can be transformed into an instance of a [linked object][crate]
/// from a specific family of linked objects of type `T`.
///
/// The handle can be cloned to allow multiple instances of the linked object to be created.
/// Alternatively, the instances of linked objects can themselves be cloned - both approaches
/// end up with the same outcome.
///
/// # Thread safety
///
/// The handle is thread-safe and may be used on any thread.
#[derive(Clone)]
pub struct Handle<T> {
    // For the handle, we extract the factory from the `Link` because the `Link` is not thread-safe.
    // In other words, a `Link` exists only in interactions with a specific instance of `T`.
    instance_factory: InstanceFactory<T>,
}

impl<T> Debug for Handle<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handle")
            .field(
                "instance_factory",
                &format_args!(
                    "Arc<dyn Fn(Link<{t}>) -> {t}>",
                    t = std::any::type_name::<T>()
                ),
            )
            .finish()
    }
}

impl<T> Handle<T> {
    pub(super) fn new(link: Link<T>) -> Self {
        Self {
            instance_factory: link.instance_factory,
        }
    }

    // Implementation of `From<Handle<T>> for T`, called from macro-generated code for a specific T.
    #[doc(hidden)]
    pub fn __private_into(self) -> T {
        Link::new(Arc::clone(&self.instance_factory)).into_instance()
    }
}
