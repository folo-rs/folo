// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;

use crate::__private::{InstanceFactory, Link};

/// A handle can be obtained from any instance of a [linked object][crate] and used to create new
/// instances from the same family on any thread.
///
/// Contrast this to cloning the linked object, which can only create a new instance on the same
/// thread because linked object instances do not implement `Send` and cannot be moved across
/// threads.
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
    #[cfg_attr(test, mutants::skip)] // We have no API contract for this.
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
    #[inline]
    pub fn __private_into(self) -> T {
        Link::new(Arc::clone(&self.instance_factory)).into_instance()
    }
}
