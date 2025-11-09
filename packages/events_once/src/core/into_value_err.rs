/// Error kind returned from `R::into_value()`.
#[expect(
    clippy::exhaustive_enums,
    reason = "intentionally narrow, accepting the risk"
)]
#[derive(Debug, Eq, PartialEq)]
pub enum IntoValueError<R> {
    /// The value has not yet been sent.
    ///
    /// This error returns the receiver `R` to the caller so they can try again later.
    Pending(R),

    /// The sender disconnected before sending a value.
    Disconnected,
}
