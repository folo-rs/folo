/// Internal coordinates for tracking items within the pool structure.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ItemCoordinates {
    /// The index of the slab containing this item.
    slab_index: usize,
    /// The index within the slab where this item is stored.
    index_in_slab: usize,
}

impl ItemCoordinates {
    #[must_use]
    pub(crate) fn from_parts(slab: usize, index_in_slab: usize) -> Self {
        Self {
            slab_index: slab,
            index_in_slab,
        }
    }

    /// Returns the index of the slab containing this item.
    #[must_use]
    pub(crate) fn slab_index(&self) -> usize {
        self.slab_index
    }

    /// Returns the index within the slab where this item is stored.
    #[must_use]
    pub(crate) fn index_in_slab(&self) -> usize {
        self.index_in_slab
    }
}
