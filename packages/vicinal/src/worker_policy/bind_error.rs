use std::error::Error;
use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::OhnoCore;

/// A worker policy failed while binding to a pool.
///
/// The error retains the policy's original failure in its source chain.
#[derive(ohno::Error)]
#[no_constructors]
#[display("worker policy binding failed")]
pub struct BindError {
    #[error]
    core: OhnoCore,
}

// The erased foreign error need not be unwind-safe. This wrapper guards no user data
// and exposes no mutation; reevaluate these impls if that changes.
impl UnwindSafe for BindError {}
impl RefUnwindSafe for BindError {}

impl BindError {
    /// Creates a binding error from a policy-specific failure.
    #[must_use]
    pub fn new(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            core: OhnoCore::from(error),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(BindError: Send, Sync, UnwindSafe, RefUnwindSafe);
}
