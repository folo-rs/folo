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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    // IntoValueError is UnwindSafe/RefUnwindSafe when R is.
    assert_impl_all!(IntoValueError<u32>: UnwindSafe, RefUnwindSafe);
}
