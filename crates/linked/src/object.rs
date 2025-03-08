// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use crate::Handle;

/// Operations available on every instance of a [linked object][crate].
///
/// The only supported way to implement this is via [`#[linked::object]`][crate::object].
pub trait Object: Sized + Clone {
    /// Gets a thread-safe handle that can be used to create linked instances on other threads.
    ///
    /// The returned handle can be converted into a new instance of a linked object via
    /// the `From<Handle<T>> for T` implementation (i.e. `let foo: Foo = handle.into()`).
    fn handle(&self) -> Handle<Self>;
}
