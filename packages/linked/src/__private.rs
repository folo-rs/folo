// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

//! This module contains logically private things that must be technically public
//! because they are accessed from macro-generated code.

use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;

/// Re-export so we can use it via macros in projects that do not have a reference to `pastey`.
pub use ::pastey::paste;

use crate::{Family, Object};

/// This is meant to be used via the [`linked::new!`][crate::new] macro, never directly called.
///
/// Creates a family of linked objects, the instances of which are created using a callback whose
/// captured state connects all members of the linked object family.
///
/// The instance factory must be thread-safe, which implies that all captured state in this factory
/// function must be `Send` + `Sync` + `'static`. The instances it returns do not need to be thread-
/// safe, however.
#[inline]
pub fn new<T>(instance_factory: impl Fn(Link<T>) -> T + Send + Sync + 'static) -> T {
    Link::new(Arc::new(instance_factory)).into_instance()
}

/// This is meant to be used via the `#[linked::object]` macro, never directly called.
///
/// Clones a linked object. They require a specific pattern to clone, so the `#[linked::object]`
/// macro wires up a suitable `Clone` implementation for all such types to avoid mistakes.
#[inline]
pub fn clone<T>(value: &T) -> T
where
    T: Object + From<Family<T>>,
{
    value.family().into()
}

pub(crate) type InstanceFactory<T> = Arc<dyn Fn(Link<T>) -> T + Send + Sync + 'static>;

/// An object that connects an instance to other instances in the same linked object family.
///
/// This type serves the linked object infrastructure and is not meant to be used by user code.
/// It is a private public type because it is used in macro-generated code.
pub struct Link<T> {
    pub(crate) instance_factory: InstanceFactory<T>,
}

impl<T> Debug for Link<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Link")
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

impl<T> Link<T> {
    #[must_use]
    pub(crate) fn new(instance_factory: InstanceFactory<T>) -> Self {
        Self { instance_factory }
    }

    #[must_use]
    pub(crate) fn into_instance(self) -> T {
        let instance_factory = Arc::clone(&self.instance_factory);
        (instance_factory)(self)
    }

    // This type deliberately does not implement `Clone` to discourage accidental implementation of
    // cloning of type `T` via `#[derive(Clone)]`. The expected pattern is to use `#[linked::object]`
    // which generates both a `Linked` implementation and a specialized `Clone` implementation.
    #[must_use]
    fn clone(&self) -> Self {
        Self {
            instance_factory: Arc::clone(&self.instance_factory),
        }
    }

    #[inline]
    #[must_use]
    pub fn family(&self) -> Family<T> {
        Family::new(self.clone())
    }
}
