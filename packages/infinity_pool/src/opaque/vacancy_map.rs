//! Vacancy map implementation for tracking which slabs have vacant slots.
//!
//! This module provides a minimal bit vector specifically designed for tracking slab
//! vacancies in the infinity pool. It supports only the operations required for this
//! purpose and intentionally omits features like iteration, complex slicing, or bit
//! arithmetic.

use std::ops::{Bound, RangeBounds};

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
    blocks: Vec<BitBlock>,
    len_bits: usize,
}

impl VacancyMap {
    /// Creates a new, empty vacancy map.
    pub(crate) const fn new() -> Self {
        Self {
            blocks: Vec::new(),
            len_bits: 0,
        }
    }

    /// Returns the number of bits in the map.
    pub(crate) const fn len(&self) -> usize {
        self.len_bits
    }

    /// Resizes the vacancy map to the specified length.
    ///
    /// If the new length is greater than the current length, new bits are set to `initial_value`.
    /// If the new length is less than the current length, the map is truncated.
    #[cfg_attr(test, mutants::skip)] // Some mutations extend logic into impossible branches, which are untestable.
    pub(crate) fn resize(&mut self, len_bits: usize, initial_value: bool) {
        if len_bits == self.len_bits {
            return;
        }

        let old_len_bits = self.len_bits;
        self.len_bits = len_bits;

        let new_len_blocks = len_bits.div_ceil(BITS_PER_BLOCK);
        let old_len_blocks = old_len_bits.div_ceil(BITS_PER_BLOCK);

        if len_bits > old_len_bits {
            // Growing: need to set new bits to initial_value.
            self.blocks.resize(
                new_len_blocks,
                if initial_value { BitBlock::MAX } else { 0 },
            );

            // If there was a partial block at the end, we need to set the new bits in it.
            // This block may or may not be the final block after resizing but was before.
            if !old_len_bits.is_multiple_of(BITS_PER_BLOCK) && old_len_blocks == new_len_blocks {
                let previous_partial_block_len_bits = old_len_bits % BITS_PER_BLOCK;
                let updated_partial_block_len_bits = len_bits % BITS_PER_BLOCK;

                // SAFETY: This will never wrap because that would imply we entered this "partial
                // block" branch with zero blocks, which is contradictory. For the same reason we
                // know that there must be a block at this index.
                let partial_block = unsafe {
                    self.blocks
                        .get_unchecked_mut(old_len_blocks.wrapping_sub(1))
                };

                for fresh_bit_index in
                    previous_partial_block_len_bits..updated_partial_block_len_bits
                {
                    if initial_value {
                        set_bit(partial_block, fresh_bit_index);
                    } else {
                        clear_bit(partial_block, fresh_bit_index);
                    }
                }
            }
        } else {
            // Shrinking: truncate blocks. The last block may now be a partial one.
            // We do not care about the "leftover" bits beyond new_len as they will
            // never participate in any logic and will be overwritten if the map is extended.
            self.blocks.truncate(new_len_blocks);
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
            index < self.len_bits,
            "index {index} out of bounds for VacancyMap of length {}",
            self.len_bits
        );

        let (block_index, index_in_block) = index.div_rem(&BITS_PER_BLOCK);

        // SAFETY: The caller guarantees that index is in bounds, which means block_index is valid.
        let block = unsafe { self.blocks.get_unchecked_mut(block_index) };

        let old_value = get_bit(*block, index_in_block);

        if value {
            set_bit(block, index_in_block);
        } else {
            clear_bit(block, index_in_block);
        }

        old_value
    }

    /// Returns a vacancy map slice view of the specified range.
    ///
    /// Returns `None` if the range is out of bounds.
    pub(crate) fn get(&self, range: impl RangeBounds<usize>) -> Option<VacancyMapSlice<'_>> {
        let start = match range.start_bound() {
            Bound::Included(&s) => s,
            Bound::Excluded(&s) => s.checked_add(1)?,
            Bound::Unbounded => 0,
        };

        let end = match range.end_bound() {
            Bound::Included(&e) => e.checked_add(1)?,
            Bound::Excluded(&e) => e,
            Bound::Unbounded => self.len_bits,
        };

        if start > end || end > self.len_bits {
            return None;
        }

        Some(VacancyMapSlice {
            map: self,
            start_bit_index: start,
            end_bit_index: end,
        })
    }
}

fn get_bit(block: BitBlock, bit_index: usize) -> bool {
    (block & (1 << bit_index)) != 0
}

fn set_bit(block: &mut BitBlock, bit_index: usize) {
    *block |= 1 << bit_index;
}

fn clear_bit(block: &mut BitBlock, bit_index: usize) {
    *block &= !(1 << bit_index);
}

/// Preserves only the bits between `start_bit_index` and `end_bit_index` (inclusive)
/// in `block`, setting all other bits to `0`.
fn mask_bits(block: BitBlock, start_bit_index: usize, end_bit_index: usize) -> BitBlock {
    // We start by masking off anything before the start.
    // For a range of 4..=8, this gives us a mask with the first four bits zeroed:
    // ..1111_1111_0000
    let mask_start = BitBlock::MAX << start_bit_index;

    // Then we mask off anything after the end. First we construct a suitable mask for this part.
    // This will never overflow because it is a usize and we are only indexing bits in a u64.
    let one_past_end_bit_index = end_bit_index.wrapping_add(1);

    // For a range of 4..=8, this gives us a mask with only bit 9(!) set:
    // ..0010_0000_0000
    //
    // We may be shifting by the entire width of the block (64), so
    // we do an unbounded shift here (which will just result in zero).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "we are counting bits, u32 is enough"
    )]
    let mask_past_end = (1 as BitBlock).unbounded_shl(one_past_end_bit_index as u32);

    // And now we do the subtraction movement to flip it and shift by one:
    // ..0000_1111_1111
    // This will never overflow because we already started with a "plus one" above.
    let mask_end = mask_past_end.wrapping_sub(1);

    // Done! We apply one mask to zero out bits before the start,
    // and the other to zero out bits after the end.
    block & mask_start & mask_end
}

/// A borrowed view into a `VacancyMap`.
#[derive(Debug)]
pub(crate) struct VacancyMapSlice<'a> {
    map: &'a VacancyMap,

    start_bit_index: usize,

    // Exclusive (index of the first bit past the end)
    end_bit_index: usize,
}

impl VacancyMapSlice<'_> {
    /// Finds the index of the first bit set to `1` in this slice.
    ///
    /// Returns the index relative to the start of the slice, or `None` if no bits are set.
    #[cfg_attr(test, mutants::skip)] // Too annoying to test. Some mutations may also cause logic to go out of bounds.
    pub(crate) fn first_one(&self) -> Option<usize> {
        let mut start_bit_index = self.start_bit_index;

        // This cannot wrap unless we just created the slice with start > end,
        // in which case we deserve our fate.
        let mut bits_remaining = self.end_bit_index.wrapping_sub(self.start_bit_index);

        while bits_remaining != 0 {
            let (block_index, start_index_in_block) = start_bit_index.div_rem(&BITS_PER_BLOCK);

            let end_index_in_block_exclusive = usize::min(
                BITS_PER_BLOCK,
                // This will never wrap because it can never go beyond the number of bits in the map, which is usize-bounded.
                start_index_in_block.wrapping_add(bits_remaining),
            );

            // SAFETY: This can only be out of bounds if the slice was created with invalid indices.
            let block = unsafe { self.map.blocks.get_unchecked(block_index) };

            let masked_block = mask_bits(
                *block,
                start_index_in_block,
                // This will never wrap because that would imply we have an empty range, which is
                // impossible as it would result in bits_remaining being zero, exiting the loop.
                end_index_in_block_exclusive.wrapping_sub(1),
            );

            if masked_block != 0 {
                // There must be at least one bit set in this masked block.
                let one_index_in_block = masked_block.trailing_zeros() as usize;

                // Calculate absolute position of the found bit in the entire map.
                // This will never wrap because that would imply the bit was found outside the map.
                let absolute_pos = block_index
                    .wrapping_mul(BITS_PER_BLOCK)
                    .wrapping_add(one_index_in_block);

                // And make it relative to the start of the slice.
                // This will never wrap because it would mean we found
                // the set bit before the start of the slice.
                return Some(absolute_pos.wrapping_sub(self.start_bit_index));
            }

            // These will not wrap unless we screwed up our math somewhere.
            let bits_scanned_in_this_block =
                end_index_in_block_exclusive.wrapping_sub(start_index_in_block);

            start_bit_index = start_bit_index.wrapping_add(bits_scanned_in_this_block);
            bits_remaining = bits_remaining.wrapping_sub(bits_scanned_in_this_block);
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
#[cfg_attr(coverage_nightly, coverage(off))]
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
        assert_eq!(slice.start_bit_index, 0);
        assert_eq!(slice.end_bit_index, 10);
    }

    #[test]
    fn get_partial_range() {
        let mut map = VacancyMap::new();
        map.resize(10, true);

        let slice = map.get(3..7).unwrap();
        assert_eq!(slice.start_bit_index, 3);
        assert_eq!(slice.end_bit_index, 7);
    }

    #[test]
    fn get_open_ended_range() {
        let mut map = VacancyMap::new();
        map.resize(10, true);

        let slice = map.get(5..).unwrap();
        assert_eq!(slice.start_bit_index, 5);
        assert_eq!(slice.end_bit_index, 10);
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

    #[test]
    fn first_one_ignores_bits_before_slice_in_partial_block() {
        let mut map = VacancyMap::new();
        map.resize(100, false);

        // Set bit at position 5 (before slice start).
        unsafe {
            map.replace_unchecked(5, true);
        }

        // Create slice that starts at position 10.
        let slice = map.get(10..20).unwrap();
        assert_eq!(slice.first_one(), None);
    }

    #[test]
    fn first_one_ignores_bits_after_slice_in_partial_block() {
        let mut map = VacancyMap::new();
        map.resize(100, false);

        // Set bit at position 25 (after slice end).
        unsafe {
            map.replace_unchecked(25, true);
        }

        // Create slice that ends at position 20.
        let slice = map.get(10..20).unwrap();
        assert_eq!(slice.first_one(), None);
    }

    #[test]
    fn first_one_ignores_bits_outside_slice_in_multiblock() {
        let mut map = VacancyMap::new();
        map.resize(200, false);

        // Set bits before and after the slice range.
        unsafe {
            map.replace_unchecked(5, true); // Before slice start
            map.replace_unchecked(150, true); // After slice end
        }

        // Create slice from 10 to 140.
        let slice = map.get(10..140).unwrap();
        assert_eq!(slice.first_one(), None);
    }

    #[test]
    fn first_one_finds_bit_in_partial_first_block() {
        let mut map = VacancyMap::new();
        map.resize(200, false);

        unsafe {
            map.replace_unchecked(5, true); // Before slice
            map.replace_unchecked(15, true); // Inside slice
        }

        let slice = map.get(10..140).unwrap();
        assert_eq!(slice.first_one(), Some(5)); // 15 - 10 = 5
    }

    #[test]
    fn first_one_finds_bit_in_partial_last_block() {
        let mut map = VacancyMap::new();
        map.resize(200, false);

        unsafe {
            map.replace_unchecked(135, true); // Inside slice
            map.replace_unchecked(145, true); // After slice
        }

        let slice = map.get(10..140).unwrap();
        assert_eq!(slice.first_one(), Some(125)); // 135 - 10 = 125
    }

    #[test]
    fn first_one_with_bits_in_same_block_before_and_in_slice() {
        let mut map = VacancyMap::new();
        map.resize(100, false);

        unsafe {
            map.replace_unchecked(3, true); // Before slice
            map.replace_unchecked(7, true); // Inside slice
        }

        // Slice from 5..15, both bits are in the same block (block 0).
        let slice = map.get(5..15).unwrap();
        assert_eq!(slice.first_one(), Some(2)); // 7 - 5 = 2
    }

    #[test]
    fn first_one_with_bits_in_same_block_in_and_after_slice() {
        let mut map = VacancyMap::new();
        map.resize(100, false);

        unsafe {
            map.replace_unchecked(12, true); // Inside slice
            map.replace_unchecked(18, true); // After slice
        }

        // Slice from 5..15, both bits are in the same block (block 0).
        let slice = map.get(5..15).unwrap();
        assert_eq!(slice.first_one(), Some(7)); // 12 - 5 = 7
    }

    #[test]
    fn first_one_multiblock_with_bit_right_at_last_block_boundary() {
        let mut map = VacancyMap::new();
        map.resize(200, false);

        // Set a bit right at position 139 (last valid position in slice 10..140).
        unsafe {
            map.replace_unchecked(139, true);
            map.replace_unchecked(140, true); // Just outside
        }

        let slice = map.get(10..140).unwrap();
        assert_eq!(slice.first_one(), Some(129)); // 139 - 10 = 129
    }

    #[test]
    fn get_bit_is_sane() {
        assert!(!get_bit(0b0000_0000, 3));
        assert!(get_bit(0b0000_1000, 3));

        assert!(get_bit(BitBlock::MAX, 0));
        assert!(get_bit(BitBlock::MAX, BitBlock::BITS as usize - 1));
    }

    #[test]
    fn set_bit_is_sane() {
        let mut block = 0b0000_0000;

        set_bit(&mut block, 2);
        assert_eq!(block, 0b0000_0100);

        set_bit(&mut block, 2);
        assert_eq!(block, 0b0000_0100);

        set_bit(&mut block, 0);
        assert_eq!(block, 0b0000_0101);

        set_bit(&mut block, 63);
        assert_eq!(
            block,
            0b1000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0101
        );
    }

    #[test]
    fn clear_bit_is_sane() {
        let mut block = 0b1111_1111;

        clear_bit(&mut block, 2);
        assert_eq!(block, 0b1111_1011);

        clear_bit(&mut block, 2);
        assert_eq!(block, 0b1111_1011);

        clear_bit(&mut block, 0);
        assert_eq!(block, 0b1111_1010);

        block = BitBlock::MAX;
        clear_bit(&mut block, 63);
        assert_eq!(
            block,
            0b0111_1111_1111_1111_1111_1111_1111_1111_1111_1111_1111_1111_1111_1111_1111_1111
        );
    }
}
