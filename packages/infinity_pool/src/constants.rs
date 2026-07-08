/// Justification for `.expect()` on mutex lock: we guarantee that we never
/// panic while holding any of our mutexes, so they can never be poisoned.
pub(crate) const NEVER_POISONED: &str = "we never panic while holding this lock";
