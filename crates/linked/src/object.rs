// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

use crate::Family;

/// Operations available on every instance of a [linked object][crate].
///
/// The only supported way to implement this is via [`#[linked::object]`][crate::object].
pub trait Object: From<Family<Self>> + Sized + Clone + 'static {
    /// The object family that the current instance is linked to.
    ///
    /// The returned object can be used to create additional instances linked to the same family.
    fn family(&self) -> Family<Self>;
}
