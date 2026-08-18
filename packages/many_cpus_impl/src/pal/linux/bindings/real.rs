use std::fmt::Debug;
use std::io;
use std::num::NonZero;

use libc::cpu_set_t;

use crate::pal::linux::{Bindings, CpuMask};

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
        // as its size accompanies it. A mask is a sequence of machine words, so it satisfies the
        // alignment that `cpu_set_t` demands.
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
                    assert_eq!(error.raw_os_error(), Some(libc::EINVAL), "{error}");
                }
            }
        }

        panic!("the operating system accepted no mask, however wide");
    }

    #[test]
    fn affinity_is_readable_once_the_mask_is_wide_enough() {
        let (_, mask) = read_affinity_by_widening();

        let current_processor = ProcessorId::try_from(BuildTargetBindings.sched_getcpu())
            .expect("the current thread runs on a processor with a valid identifier");

        // A thread is always allowed to run where it is already running.
        assert!(mask.contains(current_processor), "{mask:?}");
    }

    #[test]
    fn affinity_is_writable_in_the_width_the_operating_system_offered() {
        let (_, mask) = read_affinity_by_widening();

        // Writing back the mask we just read changes nothing, which makes it safe to do in a
        // test, while still exercising the full path into the operating system.
        BuildTargetBindings
            .sched_setaffinity_current(&mask)
            .expect("the operating system accepts the affinity it just reported");
    }
}
