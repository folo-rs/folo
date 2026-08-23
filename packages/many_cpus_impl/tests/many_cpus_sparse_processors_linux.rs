//! A process may be permitted to use a non-contiguous subset of the machine's processors, in
//! which case the ID space the machine describes is larger than the set of processors the
//! process can actually use and has holes in it. Simulated hardware is otherwise our only way
//! to reach that state, so this test reaches it on the real machine by narrowing the processor
//! affinity of the current process to its lowest and highest processor.
//!
//! One test per file to enforce process isolation: processor affinity is process-level state
//! and hardware observations are cached on first use, so the constraint must be in place before
//! anything in the process looks at the hardware.

#![cfg(target_os = "linux")]

use std::mem;

use libc::cpu_set_t;
use many_cpus::{Processor, ProcessorId, SystemHardware};

#[test]
#[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
fn narrowed_affinity_reveals_sparse_processor_ids() {
    let available = available_processor_ids();

    // With fewer than three processors there is no processor between the lowest and the highest
    // to leave out, so the ID space we observe stays contiguous and there is nothing to test.
    if available.len() < 3 {
        eprintln!("Skipping test: fewer than three processors available.");
        return;
    }

    let lowest = *available.first().unwrap();
    let highest = *available.last().unwrap();

    if !narrow_process_affinity_to(&[lowest, highest]) {
        eprintln!("Skipping test: the environment does not permit narrowing processor affinity.");
        return;
    }

    // Nothing in this process has observed the hardware before this point, so what we see here
    // is the machine as it appears under the narrowed affinity.
    let hw = SystemHardware::current();

    let all_processors = hw.all_processors();

    let mut observed_ids = all_processors.iter().map(Processor::id).collect::<Vec<_>>();
    observed_ids.sort_unstable();

    assert_eq!(observed_ids, [lowest, highest]);

    // The ID space covers the whole machine, while only the two processors we kept are usable -
    // this is the gap between the maximum and the available that we are here to exercise on real
    // hardware.
    assert!(hw.max_processor_count() > all_processors.len());

    // The active count describes the machine, which a constraint on the process does not change,
    // so the active count also exceeds what the process may use. Narrowing the affinity of the
    // process therefore cannot make the active count differ from the maximum count.
    assert!(hw.active_processor_count() > all_processors.len());

    // Every active processor occupies an ID, while the ID space may additionally cover
    // processors that are not currently active.
    assert!(hw.max_processor_count() >= hw.active_processor_count());

    // The default processor set obeys the resource quota on top of the affinity, so it can only
    // be a subset of what the affinity permits.
    let default_processors = hw.processors();
    assert!(default_processors.len() <= all_processors.len());
    assert!(
        default_processors
            .iter()
            .all(|processor| observed_ids.contains(&processor.id()))
    );

    // The thread executing this test can only be on a processor the affinity permits, which the
    // package must report through the sparse ID space rather than through a dense position.
    assert!(observed_ids.contains(&hw.current_processor_id()));

    // The memory region reported for the current processor must be one of the regions the
    // permitted processors belong to. Looking the processor up by an ID that is no longer the
    // position of the processor in the set is what makes this more than a formality.
    let current_memory_region_id = hw.current_memory_region_id();

    assert!(
        all_processors
            .iter()
            .any(|processor| processor.memory_region_id() == current_memory_region_id)
    );
}

/// Returns the IDs of the processors the calling thread is currently permitted to use, in
/// ascending order.
///
/// This asks the operating system directly instead of going through the package under test, so
/// that the package makes its first observation of the hardware under the narrowed affinity.
fn available_processor_ids() -> Vec<ProcessorId> {
    // SAFETY: All zeroes is a valid `cpu_set_t` - it is a plain bit set.
    let mut cpu_set: cpu_set_t = unsafe { mem::zeroed() };

    // A process ID of 0 means the calling thread.
    // SAFETY: The buffer we point at is a live `cpu_set_t` of exactly the size we declare.
    let result = unsafe { libc::sched_getaffinity(0, size_of::<cpu_set_t>(), &raw mut cpu_set) };
    assert_eq!(result, 0);

    // The bound is the number of bits the structure holds, which is what `CPU_ISSET` checks the
    // index against. `libc::CPU_SETSIZE` is not that number on every libc implementation - musl
    // publishes a value below the capacity of the structure it ships - and scanning only that
    // far would hide the processors above it on a large machine.
    let cpu_set_capacity = size_of::<cpu_set_t>()
        .checked_mul(usize::try_from(u8::BITS).unwrap())
        .unwrap();

    (0..cpu_set_capacity)
        // SAFETY: The index is within the set size and the set is initialized.
        .filter(|index| unsafe { libc::CPU_ISSET(*index, &cpu_set) })
        .map(|index| ProcessorId::try_from(index).unwrap())
        .collect()
}

/// Narrows the processor affinity of the current process to the given processors, returning
/// whether the operating system permitted the change.
///
/// Both the main thread and the calling thread are narrowed: the package reads the processors
/// permitted to the process from `/proc/self/status`, which reports the affinity of the main
/// thread, while the processors permitted to the caller come from the affinity of the calling
/// thread, and the test harness need not run the test on the main thread.
fn narrow_process_affinity_to(processor_ids: &[ProcessorId]) -> bool {
    // SAFETY: All zeroes is a valid `cpu_set_t` - it is a plain bit set.
    let mut cpu_set: cpu_set_t = unsafe { mem::zeroed() };

    for &processor_id in processor_ids {
        let index = usize::try_from(processor_id).unwrap();

        // SAFETY: The index came from a `cpu_set_t` we read, so it is within the set size, and
        // the set we write into is initialized.
        unsafe {
            libc::CPU_SET(index, &mut cpu_set);
        }
    }

    // SAFETY: No safety requirements.
    let main_thread = unsafe { libc::getpid() };

    // A process ID of 0 means the calling thread.
    for target in [main_thread, 0] {
        // SAFETY: The buffer we point at is a live `cpu_set_t` of exactly the size we declare.
        let result =
            unsafe { libc::sched_setaffinity(target, size_of::<cpu_set_t>(), &raw const cpu_set) };

        if result != 0 {
            return false;
        }
    }

    true
}
