//! Vacancy map implementation for tracking which slabs have vacant slots.
//!
//! This module provides a minimal bit vector specifically designed for tracking slab
//! vacancies in the infinity pool. It supports only the operations required for this
//! purpose and intentionally omits features like iteration, complex slicing, or bit
//! arithmetic.

use std::ops::RangeBounds;

use num_integer::Integer;

/// The type used for storage blocks in the vacancy map.
type BitBlock = u64;

/// Number of bits in each storage block.
const BITS_PER_BLOCK: usize = BitBlock::BITS as usize;

/// A bit vector that stores bits in 64-bit blocks.
///
/// This implementation is optimized for the specific needs of vacancy tracking and provides
/// only the minimal API surface required.
#[derive(Debug)]
pub(crate) struct VacancyMap {
    /// Storage blocks.
    blocks: Vec<BitBlock>,

    /// Number of bits in the vector.
    len: usize,
}

impl VacancyMap {
    /// Creates a new, empty vacancy map.
    pub(crate) const fn new() -> Self {
        Self {
            blocks: Vec::new(),
            len: 0,
        }
    }

    /// Returns the number of bits in the map.
    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    /// Resizes the vacancy map to the specified length.
    ///
    /// If the new length is greater than the current length, new bits are set to `initial_value`.
    /// If the new length is less than the current length, the map is truncated.
    pub(crate) fn resize(&mut self, new_len: usize, initial_value: bool) {
        if new_len == self.len {
            return;
        }

        let old_len = self.len;
        self.len = new_len;

        let new_blocks = new_len.div_ceil(BITS_PER_BLOCK);
        let old_blocks = old_len.div_ceil(BITS_PER_BLOCK);

        if new_len > old_len {
            // Growing: need to set new bits to initial_value.
            self.blocks
                .resize(new_blocks, if initial_value { BitBlock::MAX } else { 0 });

            // If there was a partial block at the end, we need to set the new bits in it.
            if old_len % BITS_PER_BLOCK != 0 && old_blocks == new_blocks {
                let old_block_bits = old_len % BITS_PER_BLOCK;
                let new_block_bits = new_len % BITS_PER_BLOCK;

                // SAFETY: This will never wrap because that would imply we entered this "partial
                // block" branch with zero blocks, which is contradictory. For the same reason we
                // know that there must be a block at this index.
                let partial_block =
                    unsafe { self.blocks.get_unchecked_mut(old_blocks.wrapping_sub(1)) };

                for bit_offset in old_block_bits..new_block_bits {
                    if initial_value {
                        *partial_block |= 1 << bit_offset;
                    } else {
                        *partial_block &= !(1 << bit_offset);
                    }
                }
            }
        } else {
            // Shrinking: truncate blocks. The last block may now be a partial one.
            // We do not care about the "leftover" bits beyond new_len.
            self.blocks.truncate(new_blocks);
        }
    }

    /// Replaces the bit at the specified index with `value`, returning the old value.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `index < self.len()`. Accessing an out-of-bounds index
    /// will result in a panic in debug builds or undefined behavior in release builds.
    pub(crate) unsafe fn replace_unchecked(&mut self, index: usize, value: bool) -> bool {
        debug_assert!(
            index < self.len,
            "index {index} out of bounds for VacancyMap of length {}",
            self.len
        );

        let (block_index, bit_offset) = index.div_rem(&BITS_PER_BLOCK);

        // SAFETY: The caller guarantees that index is in bounds, which means block_index is valid.
        let block = unsafe { self.blocks.get_unchecked_mut(block_index) };

        let old_value = (*block & (1 << bit_offset)) != 0;

        if value {
            *block |= 1 << bit_offset;
        } else {
            *block &= !(1 << bit_offset);
        }

        old_value
    }

    /// Returns a vacancy map slice view of the specified range.
    ///
    /// Returns `None` if the range is out of bounds.
    pub(crate) fn get(&self, range: impl RangeBounds<usize>) -> Option<VacancyMapSlice<'_>> {
        let start = match range.start_bound() {
            std::ops::Bound::Included(&s) => s,
            std::ops::Bound::Excluded(&s) => s.checked_add(1)?,
            std::ops::Bound::Unbounded => 0,
        };

        let end = match range.end_bound() {
            std::ops::Bound::Included(&e) => e.checked_add(1)?,
            std::ops::Bound::Excluded(&e) => e,
            std::ops::Bound::Unbounded => self.len,
        };

        if start > end || end > self.len {
            return None;
        }

        Some(VacancyMapSlice {
            map: self,
            start,
            end,
        })
    }
}

/// A borrowed view into a `VacancyMap`.
#[derive(Debug)]
pub(crate) struct VacancyMapSlice<'a> {
    map: &'a VacancyMap,
    start: usize,
    end: usize,
}

impl VacancyMapSlice<'_> {
    /// Finds the index of the first bit set to `1` in this slice.
    ///
    /// Returns the index relative to the start of the slice, or `None` if no bits are set.
    #[expect(
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        reason = "All arithmetic and indexing operations are guaranteed to be in bounds by slice construction."
    )]
    pub(crate) fn first_one(&self) -> Option<usize> {
        // This cannot wrap unless we just created the slice with start > end,
        // in which case we deserve our fate.
        let len = self.end.wrapping_sub(self.start);
        if len == 0 {
            return None;
        }

        let (start_block, start_bit) = self.start.div_rem(&BITS_PER_BLOCK);
        let (end_block, end_bit) = (self.end - 1).div_rem(&BITS_PER_BLOCK);

        if start_block == end_block {
            // The slice is entirely within a single block.

            // SAFETY: This can only be out of bounds if the slice was created with invalid indices.
            let block = unsafe { self.map.blocks.get_unchecked(start_block) };

            let mask_start = BitBlock::MAX << start_bit;
            let mask_end = (1 << (end_bit + 1)) - 1;
            let masked_block = block & mask_start & mask_end;

            if masked_block != 0 {
                let bit_pos = masked_block.trailing_zeros() as usize;
                return Some(bit_pos - self.start);
            }
        } else {
            // First block (partial).

            // SAFETY: This can only be out of bounds if the slice was created with invalid indices.
            let first_block = unsafe { self.map.blocks.get_unchecked(start_block) };

            let mask = BitBlock::MAX << start_bit;
            let masked_first = first_block & mask;

            if masked_first != 0 {
                let bit_pos = masked_first.trailing_zeros() as usize;
                return Some(bit_pos - self.start);
            }

            // Middle blocks (full blocks).
            for block_idx in (start_block + 1)..end_block {
                // SAFETY: This can only be out of bounds if the slice was created with invalid indices.
                let block = unsafe { self.map.blocks.get_unchecked(block_idx) };

                if *block != 0 {
                    let bit_pos = block.trailing_zeros() as usize;
                    let absolute_pos = block_idx * BITS_PER_BLOCK + bit_pos;
                    return Some(absolute_pos - self.start);
                }
            }

            // Last block (partial).

            // SAFETY: This can only be out of bounds if the slice was created with invalid indices.
            let last_block = unsafe { self.map.blocks.get_unchecked(end_block) };

            let mask = (1 << (end_bit + 1)) - 1;
            let masked_last = last_block & mask;

            if masked_last != 0 {
                let bit_pos = masked_last.trailing_zeros() as usize;
                let absolute_pos = end_block * BITS_PER_BLOCK + bit_pos;
                return Some(absolute_pos - self.start);
            }
        }

        None
    }
}

#[cfg(test)]
#[expect(
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    reason = "test code, we assume safety is considered"
)]
mod tests {
    use super::*;

    #[test]
    fn new_vacancy_map_is_empty() {
        let map = VacancyMap::new();
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn resize_grow_with_true() {
        let mut map = VacancyMap::new();
        map.resize(10, true);
        assert_eq!(map.len(), 10);

        // Check that all bits are set to true.
        for i in 0..10 {
            unsafe {
                let old = map.replace_unchecked(i, true);
                assert!(old, "bit {i} should be true");
            }
        }
    }

    #[test]
    fn resize_grow_with_false() {
        let mut map = VacancyMap::new();
        map.resize(10, false);
        assert_eq!(map.len(), 10);

        // Check that all bits are set to false.
        for i in 0..10 {
            unsafe {
                let old = map.replace_unchecked(i, false);
                assert!(!old, "bit {i} should be false");
            }
        }
    }

    #[test]
    fn resize_shrink() {
        let mut map = VacancyMap::new();
        map.resize(100, true);
        map.resize(10, false);
        assert_eq!(map.len(), 10);
    }

    #[test]
    fn resize_grow_then_shrink_then_grow() {
        let mut map = VacancyMap::new();
        map.resize(100, true);
        map.resize(10, false);
        map.resize(50, true);
        assert_eq!(map.len(), 50);

        // First 10 bits should still be true from initial resize.
        for i in 0..10 {
            unsafe {
                let old = map.replace_unchecked(i, true);
                assert!(old, "bit {i} should be true");
            }
        }
    }

    #[test]
    fn replace_unchecked_single_bit() {
        let mut map = VacancyMap::new();
        map.resize(10, false);

        unsafe {
            let old = map.replace_unchecked(5, true);
            assert!(!old);
            let old = map.replace_unchecked(5, false);
            assert!(old);
        }
    }

    #[test]
    fn replace_unchecked_at_block_boundaries() {
        let mut map = VacancyMap::new();
        map.resize(200, false);

        // Test at block boundaries.
        for &index in &[0, 1, 63, 64, 65, 127, 128, 129, 191, 192, 199] {
            if index < 200 {
                unsafe {
                    let old = map.replace_unchecked(index, true);
                    assert!(!old, "bit {index} should initially be false");
                    let old = map.replace_unchecked(index, true);
                    assert!(old, "bit {index} should now be true");
                }
            }
        }
    }

    #[test]
    fn get_full_range() {
        let mut map = VacancyMap::new();
        map.resize(10, true);

        let slice = map.get(0..10).unwrap();
        assert_eq!(slice.start, 0);
        assert_eq!(slice.end, 10);
    }

    #[test]
    fn get_partial_range() {
        let mut map = VacancyMap::new();
        map.resize(10, true);

        let slice = map.get(3..7).unwrap();
        assert_eq!(slice.start, 3);
        assert_eq!(slice.end, 7);
    }

    #[test]
    fn get_open_ended_range() {
        let mut map = VacancyMap::new();
        map.resize(10, true);

        let slice = map.get(5..).unwrap();
        assert_eq!(slice.start, 5);
        assert_eq!(slice.end, 10);
    }

    #[test]
    fn get_out_of_bounds_returns_none() {
        let mut map = VacancyMap::new();
        map.resize(10, true);

        assert!(map.get(5..15).is_none());
        assert!(map.get(11..).is_none());
    }

    #[test]
    fn first_one_in_empty_slice() {
        let map = VacancyMap::new();
        let slice = map.get(0..0).unwrap();
        assert_eq!(slice.first_one(), None);
    }

    #[test]
    fn first_one_all_zeros() {
        let mut map = VacancyMap::new();
        map.resize(100, false);

        let slice = map.get(0..100).unwrap();
        assert_eq!(slice.first_one(), None);
    }

    #[test]
    fn first_one_all_ones() {
        let mut map = VacancyMap::new();
        map.resize(100, true);

        let slice = map.get(0..100).unwrap();
        assert_eq!(slice.first_one(), Some(0));
    }

    #[test]
    fn first_one_single_bit_at_start() {
        let mut map = VacancyMap::new();
        map.resize(100, false);
        unsafe {
            map.replace_unchecked(0, true);
        }

        let slice = map.get(0..100).unwrap();
        assert_eq!(slice.first_one(), Some(0));
    }

    #[test]
    fn first_one_single_bit_at_end() {
        let mut map = VacancyMap::new();
        map.resize(100, false);
        unsafe {
            map.replace_unchecked(99, true);
        }

        let slice = map.get(0..100).unwrap();
        assert_eq!(slice.first_one(), Some(99));
    }

    #[test]
    fn first_one_single_bit_in_middle() {
        let mut map = VacancyMap::new();
        map.resize(100, false);
        unsafe {
            map.replace_unchecked(50, true);
        }

        let slice = map.get(0..100).unwrap();
        assert_eq!(slice.first_one(), Some(50));
    }

    #[test]
    fn first_one_in_partial_slice() {
        let mut map = VacancyMap::new();
        map.resize(100, false);
        unsafe {
            map.replace_unchecked(10, true);
            map.replace_unchecked(70, true);
        }

        let slice = map.get(50..100).unwrap();
        assert_eq!(slice.first_one(), Some(20)); // 70 - 50 = 20
    }

    #[test]
    fn first_one_at_block_boundaries() {
        let mut map = VacancyMap::new();
        map.resize(200, false);

        // Test bit at position 64 (start of second block).
        unsafe {
            map.replace_unchecked(64, true);
        }
        let slice = map.get(0..200).unwrap();
        assert_eq!(slice.first_one(), Some(64));
    }

    #[test]
    fn first_one_spanning_multiple_blocks() {
        let mut map = VacancyMap::new();
        map.resize(300, false);

        // Set a bit in the third block.
        unsafe {
            map.replace_unchecked(150, true);
        }

        let slice = map.get(0..300).unwrap();
        assert_eq!(slice.first_one(), Some(150));
    }

    #[test]
    fn first_one_within_single_block_slice() {
        let mut map = VacancyMap::new();
        map.resize(100, false);
        unsafe {
            map.replace_unchecked(10, true);
        }

        let slice = map.get(5..15).unwrap();
        assert_eq!(slice.first_one(), Some(5)); // 10 - 5 = 5
    }
}
