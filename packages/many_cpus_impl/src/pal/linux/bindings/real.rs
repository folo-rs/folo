use std::fmt::Debug;
use std::io;
use std::num::NonZero;

use libc::{c_ulong, cpu_set_t};

use crate::pal::linux::{Bindings, CpuMask};

/// The bindings hand a mask to the C API where the C API's own fixed-size mask type is expected.
/// A mask is a sequence of machine words and the fixed-size mask is the same sequence under
/// another name, so a pointer into one is aligned for the other. That is a property of the ABI
/// rather than something we control, so it is pinned here: an ABI that ever stops defining its
/// mask in terms of machine words breaks the build rather than the pointer.
const _: () = assert!(
    align_of::<cpu_set_t>() <= align_of::<c_ulong>(),
    "a mask must be aligned for the fixed-size mask type that it stands in for"
);

/// FFI bindings that target the real operating system that the build is targeting.
///
/// You would only use different bindings in PAL unit tests that need to use mock bindings.
/// Even then, whenever possible, unit tests should use real bindings for maximum realism.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetBindings;

// Real OS bindings are excluded from coverage measurement because:
// 1. They are tested via integration tests running on actual Linux.
// 2. Error paths require OS-level failures that are impractical to trigger in tests.
#[cfg_attr(coverage_nightly, coverage(off))]
impl Bindings for BuildTargetBindings {
    fn sched_setaffinity_current(&self, mask: &CpuMask) -> Result<(), io::Error> {
        // The mask is typed as `cpu_set_t` in the C API but the operating system only ever
        // touches the number of bytes we declare, so a mask of any width may be passed as long
        // as its size accompanies it. Alignment is guaranteed by the assertion above.
        let mask_ptr = mask.as_ptr().cast::<cpu_set_t>();

        // 0 means current thread.
        // SAFETY: The mask is a valid buffer of the size we declare, for as long as the call
        // lasts, and the operating system reads no more than that.
        let result = unsafe { libc::sched_setaffinity(0, mask.len_bytes(), mask_ptr) };

        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn sched_getcpu(&self) -> i32 {
        // SAFETY: No safety requirements.
        unsafe { libc::sched_getcpu() }
    }

    fn sched_getaffinity_current(&self, words: NonZero<usize>) -> Result<CpuMask, io::Error> {
        let mut mask = CpuMask::with_words(words);
        let len_bytes = mask.len_bytes();

        // See `sched_setaffinity_current` for why a mask may stand in for a `cpu_set_t`.
        let mask_ptr = mask.as_mut_ptr().cast::<cpu_set_t>();

        // 0 means current thread.
        // SAFETY: The mask is a valid buffer of the size we declare, for as long as the call
        // lasts, and the operating system writes no more than that.
        let result = unsafe { libc::sched_getaffinity(0, len_bytes, mask_ptr) };

        if result == 0 {
            Ok(mask)
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::iter;

    use new_zealand::nz;

    use super::*;
    use crate::ProcessorId;

    /// How many mask widths a test may try before concluding that the operating system is not
    /// going to accept any of them. The last one describes more processors than any machine has,
    /// so reaching the end means something is wrong rather than that the machine is large.
    const MASK_WIDTH_ATTEMPTS: usize = 20;

    /// The narrowest mask that the operating system may be asked to fill.
    const NARROWEST_MASK: NonZero<usize> = nz!(1);

    /// Ratio between one mask width and the next one to try.
    const MASK_GROWTH: NonZero<usize> = nz!(2);

    /// How much wider than the platform's own fixed-size mask a deliberately oversized mask is.
    /// Any factor above one exercises the same path; this one keeps the mask small enough that
    /// its cost is irrelevant.
    const OVERSIZED_MASK_GROWTH: NonZero<usize> = nz!(4);

    /// Reads the affinity of the current thread the way the platform does, and reports how wide
    /// the mask had to be, so that tests can assert on the search as well as on its outcome.
    fn read_affinity_by_widening() -> (NonZero<usize>, CpuMask) {
        let bindings = BuildTargetBindings;

        for words in iter::successors(Some(NARROWEST_MASK), |words| words.checked_mul(MASK_GROWTH))
            .take(MASK_WIDTH_ATTEMPTS)
        {
            match bindings.sched_getaffinity_current(words) {
                Ok(mask) => return (words, mask),
                Err(error) => {
                    // The only rejection we expect is the one that says the mask is too narrow.
                    // Any other rejection would mean the search rests on a false premise.
                    assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
                }
            }
        }

        panic!("the operating system accepted no mask, however wide");
    }

    #[test]
    fn affinity_is_readable_once_the_mask_is_wide_enough() {
        let (_, mask) = read_affinity_by_widening();

        let current_processor = ProcessorId::try_from(BuildTargetBindings.sched_getcpu()).unwrap();

        // A thread is always allowed to run where it is already running.
        assert!(mask.contains(current_processor));
    }

    #[test]
    fn affinity_is_writable_in_the_width_the_operating_system_offered() {
        let (_, mask) = read_affinity_by_widening();

        // Writing back the mask we just read changes nothing, which makes it safe to do in a
        // test, while still exercising the full path into the operating system.
        BuildTargetBindings
            .sched_setaffinity_current(&mask)
            .unwrap();
    }

    #[test]
    fn a_mask_wider_than_the_fixed_size_one_reaches_the_operating_system_intact() {
        // Exceeding the platform's own fixed-size mask is the entire point of the type, yet only
        // a machine large enough to demand it would otherwise take that path. Asking for a width
        // beyond the fixed-size mask exercises it on every machine.
        let words = CpuMask::default_words()
            .checked_mul(OVERSIZED_MASK_GROWTH)
            .unwrap();

        let mask = BuildTargetBindings
            .sched_getaffinity_current(words)
            .unwrap();

        assert_eq!(mask.words(), words);

        // A wider buffer must not change the answer, only where there is room to put it.
        let (_, reference) = read_affinity_by_widening();
        assert_eq!(mask, reference);

        BuildTargetBindings
            .sched_setaffinity_current(&mask)
            .unwrap();
    }
}
