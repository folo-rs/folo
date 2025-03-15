// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

/// Defines the template used to create every instance in a linked object family.
///
/// You are expected to use this in the constructor of a [linked object][crate],
/// except when you want to always express the linked object via trait objects (`dyn Xyz`),
/// in which case you should use [`linked::new_box`][crate::new_box].
///
/// The macro body must be a struct-expression of the `Self` type. Any variables the macro body
/// captures must be thread-safe (`Send` + `Sync` + `'static`). The returned object itself does
/// not need to be thread-safe.
///
/// # Example
///
/// ```
/// use std::sync::{Arc, Mutex};
///
/// #[linked::object]
/// struct TokenCache {
///     tokens_created: usize,
///     name: String,
///     master_key: Arc<Mutex<String>>,
///     is_multidimensional: bool,
/// }
///
/// impl TokenCache {
///     fn new(name: String, is_multidimensional: bool) -> Self {
///         // Any shared data referenced by the macro body must be thread-safe.
///         let master_key = Arc::new(Mutex::new(String::new()));
///
///         linked::new!(Self {
///             tokens_created: 0,
///             name: name.clone(),
///             master_key: Arc::clone(&master_key),
///             is_multidimensional,
///         })
///     }
/// }
/// ```
///
/// Complex expressions are supported within the `Self` struct-expression:
///
/// ```
/// #[linked::object]
/// struct TokenCache {
///     token_sources: Vec<linked::Box<dyn TokenSource>>,
/// }
/// # trait TokenSource {}
///
/// impl TokenCache {
///     fn new(source_handles: Vec<linked::Handle<linked::Box<dyn TokenSource>>>) -> Self {
///         linked::new!(Self {
///             token_sources: source_handles
///                 .iter()
///                 .cloned()
///                 .map(linked::Handle::into)
///                 .collect()
///         })
///     }
/// }
/// ```
///
/// For a complete example, see `examples/linked_basic.rs`.
#[macro_export]
macro_rules! new {
    // `new!()` is forwarded to `new!(Self {})`
    (Self) => {
        ::linked::new!(Self {})
    };
    // Special case if there are no field initializers (for proper comma handling).
    (Self {}) => {
        ::linked::__private::new(move |__private_linked_link| Self {
            __private_linked_link,
        })
    };
    // Typical case - struct expression with zero or more field initializers.
    // Each field initializer is processed as per the `@expand` rules below,
    // which essentially does not touch/change them.
    (Self { $($field:ident $( : $value:expr )?),* $(,)? }) => {
        ::linked::__private::new(move |__private_linked_link| Self {
            $($field: ::linked::new!(@expand $field $( : $value )?)),*,
            __private_linked_link,
        })
    };
    (@expand $field:ident : $value:expr) => {
        $value
    };
    (@expand $field:ident) => {
        $field
    };
}
