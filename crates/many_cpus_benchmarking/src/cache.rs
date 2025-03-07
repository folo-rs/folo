use std::{
    cell::RefCell,
    hint::black_box,
    mem::{self},
    ptr,
    sync::LazyLock,
};

// Large servers can make hundreds of MBs of L3 cache available to a single core, though it
// depends on the specific model and hardware configuration. We use a sufficiently large data set
// here to have a good chance of evicting the real payload data from the caches.
#[cfg(not(miri))]
const CACHE_CLEANER_LEN_BYTES: usize = 128 * 1024 * 1024;
#[cfg(miri)]
const CACHE_CLEANER_LEN_BYTES: usize = 1024;

const CACHE_CLEANER_LEN_U64: usize = CACHE_CLEANER_LEN_BYTES / mem::size_of::<u64>();

// We copy the data from a shared immutable source.
static CACHE_CLEANER_SOURCE: LazyLock<Vec<u64>> =
    LazyLock::new(|| vec![0x0102030401020304; CACHE_CLEANER_LEN_U64]);

// To a thread-specific destination (just to avoid overlap/conflict).
// The existing values here do not matter, we will overwrite them (potentially multiple times).
thread_local! {
    static CACHE_CLEANER_DESTINATION: RefCell<Vec<u64>> =
        RefCell::new(vec![0xFFFFFFFFFFFFFFFF; CACHE_CLEANER_LEN_U64]);
}

/// As the whole point of this benchmark harness is to demonstrate differences when running under
/// different many-processor configurations, we need to ensure that memory actually gets accessed
/// during the benchmark runs - that all data is not simply cached locally.
///
/// This function will perform a large memory copy operation, which hopefully trashes any caches.
pub(crate) fn clean_caches() {
    let source_ptr = CACHE_CLEANER_SOURCE.as_ptr();
    let destination_ptr =
        CACHE_CLEANER_DESTINATION.with_borrow_mut(|destination| destination.as_mut_ptr());

    // SAFETY: Lengths are correct, pointers are valid, we are good to go.
    unsafe {
        ptr::copy_nonoverlapping(source_ptr, destination_ptr, CACHE_CLEANER_LEN_U64);
    }

    // SAFETY: We just filled these bytes, it is all good.
    CACHE_CLEANER_DESTINATION.with_borrow_mut(|destination| unsafe {
        destination.set_len(CACHE_CLEANER_LEN_U64);
    });

    // Read from the destination to prevent the compiler from optimizing the copy away.
    // SAFETY: The pointer is valid, we just used it.
    let _ = black_box(unsafe { destination_ptr.read() });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_caches_smoke_test() {
        // Just make sure it does not panic and gets a clean bill of health from Miri.
        clean_caches();
    }
}
