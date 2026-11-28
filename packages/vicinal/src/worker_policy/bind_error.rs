use std::error::Error;
use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::OhnoCore;

/// A recoverable error reported by a policy during binding.
///
/// Use this error when a custom [`WorkerPolicy`][crate::worker_policy::WorkerPolicy] cannot finish
/// one-time setup for a pool. The original policy-specific error remains in the source chain, so
/// callers can inspect it through the standard error API or aggregate it in application errors.
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
    /// Wraps a policy-specific failure while preserving its source and backtrace.
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
