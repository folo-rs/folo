/// Helper trait to extract the `T` parameter from an event reference via associated types.
///
/// This helps keep type signatures simple. You should never need this in user code, as it only
/// exists for internal type resolving. It is public only due to type system rules.
pub trait ReflectiveT {
    /// The type `T` extracted from the type signature.
    type T;
}

/// Helper trait to extract the `T` parameter from an event reference via associated types.
///
/// This helps keep type signatures simple. You should never need this in user code, as it only
/// exists for internal type resolving. It is public only due to type system rules.
pub trait ReflectiveTSend {
    /// The type `T` extracted from the type signature.
    type T: Send;
}
