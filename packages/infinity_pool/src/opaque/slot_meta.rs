use crate::Dropper;

/// Metadata for each slot in a slab, tracking occupancy state and enabling
/// type-erased value management through droppers.
#[derive(Debug)]
pub(crate) enum SlotMeta {
    /// Slot contains a valid object with associated dropper for proper cleanup.
    ///
    /// The object itself can be accessed by offsetting the slot pointer by a constant
    /// defined in the slab layout.
    ///
    /// When this variant is dropped (via assignment, `mem::replace`, or going out of scope),
    /// the dropper field is automatically dropped, which in turn causes the item to be dropped.
    Occupied {
        /// Type-erased dropper that properly destroys the stored item when dropped.
        _dropper: Dropper,
    },

    /// Slot is available and can receive an object.
    Vacant {
        /// Index of the next available slot in the freelist chain.
        next_free_slot_index: usize,
    },
}
