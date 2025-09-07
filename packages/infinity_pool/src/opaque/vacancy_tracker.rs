use bitvec::vec::BitVec;

/// Tracks which slab in a pool has the next vacant slot.
///
/// This information is performance-critical for fast insertion.
///
/// We attempt to fill slabs in order, so we fill the slabs with the lowest index first.
/// This means that our vacancy tracker needs to always indicate the lowest-index slab
/// with a vacancy.
///
/// # What affects vacancy
///
/// * Inserting a new object can fill a slab, thereby making it no longer have any vacancies.
///   Objects may be inserted into any slab - it just depends where the lowest-index vacancy
///   is found - so when we fill a slab, it may be in the middle of the pool, with any of the
///   next slabs still having vacancies (or none).
/// * Removing an object crates a vacancy in the slab it was removed from. This might not be
///   the lowest-index slab with a vacancy, though.
///
/// # How we track vacancies
///
/// The number of slabs in a pool is typically not excessive, so we can simply maintain a
/// cache of `is vacant` boolean for each slab. We receive updated information from the pool
/// whenever a vacancy-affecting operation occurs, and we update our cache accordingly.
#[derive(Debug)]
pub(crate) struct VacancyTracker {
    // Slot index to vacancy status.
    has_vacancy: BitVec,

    // Index of the lowest-index slab with a vacancy, if any.
    next_vacancy: Option<usize>,
}

impl VacancyTracker {
    pub(crate) fn new() -> Self {
        Self {
            has_vacancy: BitVec::new(),
            next_vacancy: None,
        }
    }

    /// Index of the next slab with a vacancy or `None` if all slabs are full.
    pub(crate) fn next_vacancy(&self) -> Option<usize> {
        self.next_vacancy
    }

    /// Informs the tracker that the number of slabs has changed.
    ///
    /// Any added slabs are assumed to be empty.
    pub(crate) fn update_slab_count(&mut self, count: usize) {
        let previous_count = self.has_vacancy.len();

        self.has_vacancy.resize(count, false);

        if count > previous_count {
            // If we added slabs, they are empty and therefore have vacancies.
            // If we previously considered no slabs to have vacancies, the first
            // new slab is now the lowest-index slab with a vacancy.
            if self.next_vacancy.is_none() {
                self.next_vacancy = Some(previous_count);
            }
        } else if let Some(next_vacancy) = self.next_vacancy {
            // If we removed slabs, and the current next vacancy is now out of range,
            // this means there are no more vacancies (because the current one was already
            // the lowest-index one).
            if next_vacancy >= count {
                self.next_vacancy = None;
            }
        }
    }

    /// Updates the vacancy status of a slab.
    pub(crate) fn update_slab_status(&mut self, slab_index: usize, has_vacancy: bool) {
        let slab_previously_had_vacancy = self.has_vacancy.replace(slab_index, has_vacancy);

        if has_vacancy == slab_previously_had_vacancy {
            // No change.
            return;
        }

        if has_vacancy {
            // If we just added a vacancy, and it's the lowest-index vacancy, update our cache.
            if let Some(next_vacancy) = self.next_vacancy {
                if slab_index < next_vacancy {
                    self.next_vacancy = Some(slab_index);
                }
            } else {
                self.next_vacancy = Some(slab_index);
            }
        } else {
            // If we just removed a vacancy, and it was the lowest-index vacancy, find the next one.
            if let Some(next_vacancy) = self.next_vacancy {
                if slab_index == next_vacancy {
                    // There may be a vacancy in a later slab (but never earlier,
                    // as we fill from the start of the slab list).
                    //
                    // Will not wrap because wrapping implies we have more slabs than virtual memory.
                    let remaining_range_start = slab_index.wrapping_add(1);
                    let remaining_range = remaining_range_start..;

                    let Some(remaining_bits) = self.has_vacancy.get(remaining_range) else {
                        // This was the last slab, so there are no more vacancies.
                        self.next_vacancy = None;
                        return;
                    };

                    // Will not wrap because wrapping implies we have more slabs than virtual memory.
                    self.next_vacancy = remaining_bits.first_one().map(|index_in_remaining| {
                        remaining_range_start.wrapping_add(index_in_remaining)
                    });
                }
            }
        }
    }
}
