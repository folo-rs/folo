use std::num::NonZero;

pub(crate) const NEVER_POISONED: &str = "we never panic while holding this lock";

/// One permit, for use in single-permit acquire convenience methods.
pub(crate) const ONE_PERMIT: NonZero<usize> = NonZero::new(1).unwrap();
