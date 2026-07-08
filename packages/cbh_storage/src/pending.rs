use std::sync::atomic::{AtomicBool, Ordering};

/// A one-shot "a cloud object was removed or overwritten" flag.
///
/// A cloud writer [`arm`](Self::arm)s it on a `delete` or on a `put_overwrite`
/// that *replaces an existing object* — the only operations that break the storage
/// model's per-key immutability — and a single flush at the end of a command
/// [`take`](Self::take)s it and, when set, writes one fresh [`cache_epoch_key`](super::cache_epoch_key)
/// marker. Coalescing every mutation in a command into a single marker write keeps
/// the bump to one round-trip. Creating a new key never arms it — whether through
/// write-once `put` (the additive path the CI collection uses) or a
/// `put_overwrite` that turns out to add rather than replace — so an append-only
/// run never invalidates the cache, and a read-through mirror still discovers the
/// new key through its always-fresh listing.
#[derive(Debug, Default)]
pub(crate) struct PendingInvalidation {
    armed: AtomicBool,
}

impl PendingInvalidation {
    /// Arms the flag so a later [`take`](Self::take) reports a pending bump.
    pub(crate) fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }

    /// Returns whether the flag was armed and clears it, so a second flush in the
    /// same process performs no further marker write.
    pub(crate) fn take(&self) -> bool {
        self.armed.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn pending_invalidation_starts_unarmed() {
        let pending = PendingInvalidation::default();
        assert!(!pending.take(), "a fresh flag is not armed");
    }

    #[test]
    fn pending_invalidation_arms_and_takes_once() {
        let pending = PendingInvalidation::default();
        pending.arm();
        assert!(pending.take(), "an armed flag reports a pending bump");
        assert!(
            !pending.take(),
            "taking clears the flag so a second flush is a no-op"
        );
    }

    #[test]
    fn pending_invalidation_arming_is_idempotent() {
        let pending = PendingInvalidation::default();
        pending.arm();
        pending.arm();
        assert!(
            pending.take(),
            "repeated arming still reports one pending bump"
        );
        assert!(!pending.take());
    }
}
