// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

#![doc = include_str!("../README.md")]

#[doc(hidden)]
pub mod __private;

mod block;
pub use block::*;

mod block_rc;
pub use block_rc::*;

mod r#box;
pub use r#box::*;

mod macros;

mod types;
pub use types::*;

/// Marks a struct as implementing the [linked object pattern][crate].
///
/// # Usage
///
/// Apply attribute on a struct block with named fields.
///
/// # Example
///
/// ```
/// #[linked::object]
/// pub struct TokenCache {
///     some_value: usize,
/// }
///
/// impl TokenCache {
///     pub fn new(some_value: usize) -> Self {
///         linked::new!(Self {
///             some_value,
///         })
///     }
/// }
/// ```
///
/// # Effects
///
/// Applying this macro has the following effects:
///
/// 1. Generates the necessary wiring to support calling `linked::new!` in constructors.
/// 2. Implements `Clone` for the struct. All linked objects can be cloned to create new
///    instances linked to the same family.
/// 3. Implements the trait `linked::Linked` for the struct, enabling standard linked object
///    pattern mechanisms such as calling `.handle()` on instances.
/// 4. Implements `From<linked::Handle<T>>` for the struct. This allows creating a new
///    linked instance from a handle previously obtained from `.handle()`.
/// 5. Removes `Send` and `Sync` traits implementation for the struct. Linked objects are single-
///    threaded and require explicit steps to transfer instances across threads (e.g. via handles).
///
/// # Constraints
///
/// Only structs defined in the named fields form are supported (no tuple structs).
pub use linked_macros::__macro_linked_object as object;

// This is so procedural macros can produce code which refers to
// ::linked::* which will work also in the current crate.
#[doc(hidden)]
extern crate self as linked;
