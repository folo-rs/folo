use crate::Magnitude;

// Registry code validates every expected failure before mutation and never invokes caller code
// while holding a write guard, so a poisoned registry lock indicates an internal invariant failure.
pub(crate) const ERR_POISONED_LOCK: &str = concat!(
    "registry write paths release their locks before propagating expected panics, so the lock ",
    "cannot be poisoned during supported operation"
);

/// Event APIs create a batch internally so all observation paths share one implementation.
pub(crate) const ONE_ITEM_BATCH: usize = 1;

/// Events without an inherent magnitude use the multiplicative identity.
pub(crate) const IMPLICIT_OCCURRENCE_MAGNITUDE: Magnitude = 1;
