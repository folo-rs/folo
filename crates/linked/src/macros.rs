// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

/// Creates the first instance in a linked object family. You are expected to use this in the
/// constructor of a [linked object][crate], except when you want to express the linked
/// object via trait objects, in which case you should use [`linked::new_box`][crate::new_box].
///
/// The macro body must be a struct-expression of the `Self` type. Any variables the macro body
/// captures must be thread-safe (`Send` + `Sync` + `'static`). The returned object itself does
/// not need to be thread-safe.
///
/// # Example
///
/// ```ignore
/// linked::new!(Self {
///     field1: value1,
///     field2: value2,
///     field3: Arc::clone(&value3),
///     another_field,
/// })
/// ```
///
/// Complex expressions as values are supported within the `Self` struct-expression:
///
/// ```ignore
/// linked::new!(Self {
///     sources: source_handles
///         .iter()
///         .cloned()
///         .map(Handle::into)
///         .collect_vec()
/// })
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
