/// Error kind returned from `R::into_value()`.
#[expect(
    clippy::exhaustive_enums,
    reason = "intentionally narrow, accepting the risk"
)]
#[derive(Debug, Eq, PartialEq)]
pub enum IntoValueError<R> {
    /// The event has not completed yet, so neither a value nor a disconnection is available.
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

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // This type carries the receiver and adds no state of its own, so unwind safety is exactly
    // the receiver's. The negative case uses a minimally qualified stand-in that lacks unwind
    // safety, pinning that the enum neither adds nor removes the property.
    assert_impl_all!(IntoValueError<u32>: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(IntoValueError<Box<dyn Send>>: UnwindSafe, RefUnwindSafe);
}
