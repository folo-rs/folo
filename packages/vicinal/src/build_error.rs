//! Public error aggregation for fallible pool construction.

use std::any::Any;
use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::OhnoCore;

use crate::worker_policy::BindError;
use crate::{discard_panic, extract_panic_message};

/// A worker policy or worker infrastructure failed while building a pool.
///
/// A build error retains a policy-specific binding error or a contained infrastructure panic.
/// Construction has stopped participating workers before this value reaches the caller.
#[derive(ohno::Error)]
#[no_constructors]
#[from(BindError, BuildPanicked)]
#[display("worker pool build failed")]
pub struct BuildError {
    #[error]
    core: OhnoCore,
}

impl UnwindSafe for BuildError {}
impl RefUnwindSafe for BuildError {}

impl BuildError {
    pub(crate) fn from_panic(payload: Box<dyn Any + Send>) -> Self {
        let error = BuildPanicked::new(extract_panic_message(&payload));
        discard_panic(payload);
        error.into()
    }
}

/// Describes a worker-infrastructure panic contained after construction cleanup.
#[ohno::error]
#[display("worker infrastructure panicked: {message}")]
pub(crate) struct BuildPanicked {
    message: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(BuildError: Send, Sync, UnwindSafe, RefUnwindSafe);
}
