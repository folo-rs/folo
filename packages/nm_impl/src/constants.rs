use crate::Magnitude;

/// Event APIs create a batch internally so all observation paths share one implementation.
pub(crate) const ONE_ITEM_BATCH: usize = 1;

/// Events without an inherent magnitude use the multiplicative identity.
pub(crate) const IMPLICIT_OCCURRENCE_MAGNITUDE: Magnitude = 1;
