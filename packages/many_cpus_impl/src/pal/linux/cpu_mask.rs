use std::any::type_name;
use std::fmt::{self, Debug, Formatter};
use std::num::NonZero;

use libc::{c_ulong, cpu_set_t};
use smallvec::{SmallVec, smallvec};

use crate::ProcessorId;

/// A set of processors, in the shape that the operating system's thread affinity API expects.
///
/// The kernel expresses thread affinity as a bit per processor, packed into an array of machine
/// words, whose length in bytes the caller provides on every call. This type owns such an array
/// and the bit arithmetic over it, so that the rest of the platform abstraction layer can speak
/// in terms of processor identifiers.
///
/// Small masks live on the stack and only sufficiently large ones reach for the heap. See
/// `packages/many_cpus/docs/linux.md`, "Thread affinity masks", for why the mask is counted in
/// words and why the inline capacity is the size that it is.
#[derive(Clone)]
pub(crate) struct CpuMask {
    words: SmallVec<[c_ulong; INLINE_WORDS]>,
}

/// Mask words kept inline, before a mask spills to the heap.
///
/// This matches the width of the fixed-size mask that the platform's own C API offers, so every
/// machine that the platform can describe using that mask keeps the entire mask on the stack and
/// only machines that the C API cannot describe at all pay for an allocation.
const INLINE_WORDS: usize = size_of::<cpu_set_t>().div_euclid(size_of::<c_ulong>());

/// The inline width above accounts for the whole of the platform's own mask only while that mask
/// is a whole number of machine words. Every ABI defines it that way, so this is pinned rather
/// than handled: an ABI that ever stops doing so breaks the build rather than the arithmetic.
const _: () = assert!(
    size_of::<cpu_set_t>().is_multiple_of(size_of::<c_ulong>()),
    "the platform's fixed-size mask must be a whole number of machine words"
);

/// Bits in one mask word.
const WORD_BITS: u32 = c_ulong::BITS;

/// A mask word with no processors in it.
const EMPTY_WORD: c_ulong = 0;

/// A mask word with only the lowest bit set, from which single-processor masks are built.
const LOW_BIT: c_ulong = 1;

impl CpuMask {
    /// Creates a mask that contains no processors, wide enough for the processors that the
    /// platform's own fixed-size mask can describe.
    pub(crate) fn new() -> Self {
        Self::with_words(Self::default_words())
    }

    /// The width of a mask that has not been asked to describe any particular processor.
    pub(crate) fn default_words() -> NonZero<usize> {
        NonZero::new(INLINE_WORDS).expect("the platform mask is at least one word wide")
    }

    /// Creates a mask that contains no processors, exactly `words` machine words wide.
    ///
    /// The width matters when the operating system fills the mask: it rejects a buffer that is
    /// too narrow to describe every processor the kernel knows of.
    pub(crate) fn with_words(words: NonZero<usize>) -> Self {
        Self {
            words: smallvec![EMPTY_WORD; words.get()],
        }
    }

    /// The width of the mask in machine words.
    #[cfg(test)]
    pub(crate) fn words(&self) -> NonZero<usize> {
        NonZero::new(self.words.len())
            .expect("a mask is created at least one word wide and never becomes narrower")
    }

    /// The size of the mask in bytes, which is what the operating system asks for.
    pub(crate) fn len_bytes(&self) -> usize {
        self.words
            .len()
            .checked_mul(size_of::<c_ulong>())
            .expect("the mask occupies this many bytes of memory already, so this cannot overflow")
    }

    /// Adds a processor to the mask, widening the mask if the processor does not fit.
    pub(crate) fn insert(&mut self, processor_id: ProcessorId) {
        let position = BitPosition::of(processor_id);

        let required_words = position
            .word
            .checked_add(1)
            .expect("a processor identifier is a u32, so its word index cannot overflow a usize");

        // A mask never becomes narrower, because its width may be one that the operating system
        // demanded rather than one that the processors in it call for.
        self.words
            .resize(self.words.len().max(required_words), EMPTY_WORD);

        let word = self
            .words
            .get_mut(position.word)
            .expect("the mask was just widened to include this word");

        *word |= position.bit();
    }

    /// Whether the mask contains a processor.
    ///
    /// A processor that lies beyond the width of the mask is not in the mask.
    #[cfg(test)]
    pub(crate) fn contains(&self, processor_id: ProcessorId) -> bool {
        let position = BitPosition::of(processor_id);

        self.words
            .get(position.word)
            .is_some_and(|word| word & position.bit() != EMPTY_WORD)
    }

    /// The processors in the mask, in ascending order.
    pub(crate) fn processor_ids(&self) -> impl Iterator<Item = ProcessorId> {
        self.words
            .iter()
            .copied()
            .enumerate()
            // The width of a mask is chosen by the operating system and can far exceed the
            // processors in it: a process confined to a few processors on a very large machine
            // is still answered in the machine's terms. Empty words are therefore the common
            // case on exactly the machines this mask exists for, and skipping them keeps the
            // cost of a read close to the number of processors rather than the width.
            .filter(|(_, bits)| *bits != EMPTY_WORD)
            .flat_map(|(word, bits)| {
                (0..WORD_BITS)
                    .map(move |offset| BitPosition { word, offset })
                    .filter(move |position| bits & position.bit() != EMPTY_WORD)
                    .map(BitPosition::processor_id)
            })
    }

    /// A pointer to the start of the mask, for handing to the operating system.
    pub(crate) fn as_ptr(&self) -> *const c_ulong {
        self.words.as_ptr()
    }

    /// A pointer to the start of the mask, for the operating system to fill.
    pub(crate) fn as_mut_ptr(&mut self) -> *mut c_ulong {
        self.words.as_mut_ptr()
    }

    /// The word at an index, with words beyond the mask reading as empty.
    ///
    /// Padding a narrow mask with empty words is what lets masks of different widths describe
    /// the same set of processors.
    fn word(&self, index: usize) -> c_ulong {
        self.words.get(index).copied().unwrap_or(EMPTY_WORD)
    }
}

/// Where a processor's bit sits in a mask.
#[derive(Clone, Copy, Debug)]
struct BitPosition {
    /// The word that holds the bit.
    word: usize,

    /// The bit within that word.
    offset: u32,
}

impl BitPosition {
    /// Locates the bit that represents a processor.
    fn of(processor_id: ProcessorId) -> Self {
        let word = processor_id
            .checked_div(WORD_BITS)
            .expect("a machine word is never zero bits wide");

        let offset = processor_id
            .checked_rem(WORD_BITS)
            .expect("a machine word is never zero bits wide");

        Self {
            word: word as usize,
            offset,
        }
    }

    /// A word in which only this position's bit is set.
    fn bit(self) -> c_ulong {
        LOW_BIT
            .checked_shl(self.offset)
            .expect("the offset is a remainder of the word width, so it is within the word")
    }

    /// The processor that this position represents.
    fn processor_id(self) -> ProcessorId {
        let first_in_word = ProcessorId::try_from(self.word)
            .ok()
            .and_then(|word| word.checked_mul(WORD_BITS))
            .expect("masks only ever cover processors whose identifiers fit in a ProcessorId");

        first_in_word
            .checked_add(self.offset)
            .expect("masks only ever cover processors whose identifiers fit in a ProcessorId")
    }
}

impl PartialEq for CpuMask {
    /// Masks are equal when they contain the same processors, whatever their widths.
    ///
    /// Width is a property of the buffer handed to the operating system, not of the set of
    /// processors that the mask describes, so it does not take part in the comparison.
    fn eq(&self, other: &Self) -> bool {
        let words = self.words.len().max(other.words.len());

        (0..words).all(|index| self.word(index) == other.word(index))
    }
}

impl Eq for CpuMask {}

impl Default for CpuMask {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for CpuMask {
    /// Renders the processors in the mask, as the raw words say little to a reader.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}({})",
            type_name::<Self>(),
            cpulist::emit(self.processor_ids())
        )
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::{mem, slice};

    use new_zealand::nz;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(CpuMask: UnwindSafe, RefUnwindSafe);

    /// The processor count that the inline part of a mask describes.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the inline width is a handful of words, so it fits in any integer type"
    )]
    const INLINE_BITS: ProcessorId = INLINE_WORDS as ProcessorId * WORD_BITS;

    /// The highest processor that fits in a mask of the inline width.
    const LAST_INLINE_PROCESSOR: ProcessorId = INLINE_BITS - 1;

    #[test]
    fn new_mask_is_empty_and_inline() {
        let mask = CpuMask::new();

        assert_eq!(mask.words().get(), INLINE_WORDS);
        assert_eq!(mask.processor_ids().count(), 0);
        assert!(!mask.contains(0));
    }

    #[test]
    fn with_words_uses_exactly_the_requested_width() {
        let mask = CpuMask::with_words(nz!(1));

        assert_eq!(mask.words().get(), 1);
        assert_eq!(mask.len_bytes(), size_of::<c_ulong>());
    }

    #[test]
    fn len_bytes_is_a_whole_number_of_words() {
        // The operating system rejects a mask whose size is not a whole number of words, so this
        // is a contract with the kernel and not merely an implementation detail.
        for words in [1_usize, 3, INLINE_WORDS, INLINE_WORDS * 2] {
            let mask = CpuMask::with_words(NonZero::new(words).unwrap());

            assert_eq!(mask.len_bytes() % size_of::<c_ulong>(), 0);
            assert_eq!(mask.len_bytes(), words * size_of::<c_ulong>());
        }
    }

    #[test]
    fn insert_and_contains_agree() {
        let mut mask = CpuMask::new();

        mask.insert(0);
        mask.insert(1);
        mask.insert(LAST_INLINE_PROCESSOR);

        assert!(mask.contains(0));
        assert!(mask.contains(1));
        assert!(mask.contains(LAST_INLINE_PROCESSOR));
        assert!(!mask.contains(2));
    }

    #[test]
    fn inline_processors_do_not_widen_the_mask() {
        let mut mask = CpuMask::new();

        mask.insert(LAST_INLINE_PROCESSOR);

        assert_eq!(mask.words().get(), INLINE_WORDS);
    }

    #[test]
    fn processor_beyond_inline_width_widens_the_mask() {
        let mut mask = CpuMask::new();
        mask.insert(0);
        mask.insert(LAST_INLINE_PROCESSOR);

        mask.insert(INLINE_BITS);

        assert!(mask.contains(INLINE_BITS));
        assert_eq!(mask.words().get(), INLINE_WORDS + 1);

        // Widening moves no bits: the processors already in the mask are still in it.
        assert!(mask.contains(0));
        assert!(mask.contains(LAST_INLINE_PROCESSOR));
        assert_eq!(
            mask.processor_ids().collect::<Vec<_>>(),
            vec![0, LAST_INLINE_PROCESSOR, INLINE_BITS]
        );
    }

    #[test]
    fn far_away_processor_widens_the_mask_to_fit() {
        let mut mask = CpuMask::new();
        let processor_id = INLINE_BITS * 4;

        mask.insert(processor_id);

        assert!(mask.contains(processor_id));
        assert_eq!(mask.processor_ids().collect::<Vec<_>>(), vec![processor_id]);
    }

    #[test]
    fn contains_beyond_mask_width_is_false() {
        let mask = CpuMask::with_words(nz!(1));

        assert!(!mask.contains(WORD_BITS));
        assert!(!mask.contains(ProcessorId::MAX));
    }

    #[test]
    fn processor_ids_are_ascending() {
        let mut mask = CpuMask::new();
        let expected = vec![0, 1, WORD_BITS, LAST_INLINE_PROCESSOR, INLINE_BITS * 2];

        // Insert out of order to prove that the order comes from the mask, not from the caller.
        for processor_id in expected.iter().rev() {
            mask.insert(*processor_id);
        }

        assert_eq!(mask.processor_ids().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn masks_of_different_widths_are_equal_when_they_hold_the_same_processors() {
        let mut narrow = CpuMask::with_words(nz!(1));
        narrow.insert(1);

        let mut wide = CpuMask::with_words(nz!(64));
        wide.insert(1);

        assert_eq!(narrow, wide);
        assert_eq!(wide, narrow);
    }

    #[test]
    fn masks_holding_different_processors_are_not_equal() {
        let mut narrow = CpuMask::with_words(nz!(1));
        narrow.insert(1);

        let mut wide = CpuMask::with_words(nz!(64));
        wide.insert(1);
        wide.insert(INLINE_BITS);

        assert_ne!(narrow, wide);
        assert_ne!(wide, narrow);
    }

    #[test]
    fn default_mask_equals_new_mask() {
        assert_eq!(CpuMask::default(), CpuMask::new());
    }

    #[test]
    fn debug_names_the_processors() {
        let mut mask = CpuMask::new();
        mask.insert(0);
        mask.insert(1);
        mask.insert(2);
        mask.insert(5);

        let rendered = format!("{mask:?}");

        assert!(rendered.contains("0-2"));
        assert!(rendered.contains('5'));
    }

    #[test]
    fn inline_width_matches_the_platform_mask() {
        // The whole point of the inline width is that a mask of the size the platform's C API
        // offers needs no allocation, so guard that relationship.
        assert_eq!(CpuMask::new().len_bytes(), size_of::<cpu_set_t>());
    }

    #[test]
    fn mask_bytes_match_the_platform_mask() {
        // The operating system, not this type, interprets the bits, so where a processor's bit
        // lands is a contract with the platform. Every other test here both writes and reads the
        // bits through this type, which cannot tell a correct layout apart from one that is
        // merely self-consistent, so the layout is compared against the platform's own.
        let processors = [0, 1, WORD_BITS - 1, WORD_BITS, LAST_INLINE_PROCESSOR];

        // SAFETY: A `cpu_set_t` is an array of machine words, for which an all-zero bit pattern
        // is valid and means that the set is empty.
        let mut expected: cpu_set_t = unsafe { mem::zeroed() };

        for processor in processors {
            // SAFETY: Every processor here is one that the fixed-size mask can describe, which
            // is what `CPU_SET` requires.
            unsafe { libc::CPU_SET(processor as usize, &mut expected) }
        }

        let mut mask = CpuMask::new();

        for processor in processors {
            mask.insert(processor);
        }

        // SAFETY: A `cpu_set_t` holds no padding, so all of its bytes are initialized, and the
        // borrow keeps it alive for as long as the slice. `expected` is a local that nothing
        // mutates while `expected_bytes` is live, so the shared borrow has no aliasing conflict.
        let expected_bytes = unsafe {
            slice::from_raw_parts((&raw const expected).cast::<u8>(), size_of::<cpu_set_t>())
        };

        // SAFETY: The mask owns this many initialized bytes and outlives the slice. `mask` is a
        // local that nothing mutates while `actual_bytes` is live, so the shared borrow has no
        // aliasing conflict.
        let actual_bytes =
            unsafe { slice::from_raw_parts(mask.as_ptr().cast::<u8>(), mask.len_bytes()) };

        assert_eq!(actual_bytes, expected_bytes);
    }
}
