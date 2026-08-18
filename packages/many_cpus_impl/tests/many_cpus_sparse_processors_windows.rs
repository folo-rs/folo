//! A process may be permitted to use a non-contiguous subset of the machine's processors, in
//! which case the ID space the machine describes is larger than the set of processors the
//! process can actually use and has holes in it. Simulated hardware is otherwise our only way
//! to reach that state, so this test reaches it on the real machine by placing the current
//! process in a job object whose affinity limit keeps only the lowest and highest processor
//! that the process is permitted to use.
//!
//! Windows describes the machine and the constraints on the process separately: a job object
//! affinity limit changes which processors the process may use but does not change how many
//! processors Windows says the machine has, whether at maximum or currently active. The gap
//! this exercises is therefore between the machine and the process, and the count of active
//! processors stays with the machine.
//!
//! One test per file to enforce process isolation: job objects are process-level state and
//! hardware observations are cached on first use, so the limit must be in place before anything
//! in the process looks at the hardware.

#![cfg(windows)]

use std::num::NonZero;

use many_cpus::{Processor, ProcessorId, SystemHardware};
use testing::Job;
use windows::Win32::System::Threading::{
    GetActiveProcessorGroupCount, GetCurrentProcess, GetProcessAffinityMask,
};

#[test]
#[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
fn job_affinity_limit_reveals_sparse_processor_ids() {
    // The package numbers processors across all processor groups, whereas both the affinity mask
    // of a process and the affinity limit of a job object address a single group. The two ways of
    // naming a processor coincide only on a machine that has a single processor group, which is
    // the only kind of machine on which this test can translate between them.
    // SAFETY: No safety requirements.
    if unsafe { GetActiveProcessorGroupCount() } != 1 {
        eprintln!("Skipping test: the machine has more than one processor group.");
        return;
    }

    let available = available_processor_ids();

    // With fewer than three processors there is no processor between the lowest and the highest
    // to leave out, so the ID space we observe stays contiguous and there is nothing to test.
    if available.len() < 3 {
        eprintln!("Skipping test: fewer than three processors available.");
        return;
    }

    let lowest = *available.first().unwrap();
    let highest = *available.last().unwrap();

    // Nothing in this process observes the hardware before the job limit is in place, so what we
    // see below is the machine as it appears to a process constrained to these two processors.
    let job = Job::builder()
        .processor_affinity_mask(processor_mask(&[lowest, highest]))
        .build();

    let hw = SystemHardware::current();

    let all_processors = hw.all_processors();

    let mut observed_ids = all_processors.iter().map(Processor::id).collect::<Vec<_>>();
    observed_ids.sort_unstable();

    assert_eq!(observed_ids, [lowest, highest]);

    // The ID space covers the whole machine, while only the two processors the job permits are
    // usable - this is the gap between the maximum and the available that we are here to
    // exercise on real hardware.
    assert!(hw.max_processor_count() > all_processors.len());

    // The active count describes the machine, which a constraint on the process does not change,
    // so the active count also exceeds what the process may use. A job object affinity limit
    // therefore cannot make the active count differ from the maximum count.
    assert!(hw.active_processor_count() > all_processors.len());

    // Every active processor occupies an ID, while the ID space may additionally cover
    // processors that are not currently active.
    assert!(hw.max_processor_count() >= hw.active_processor_count());

    // The default processor set obeys the resource quota on top of the affinity limit, so it can
    // only be a subset of what the limit permits.
    let default_processors = hw.processors();
    assert!(default_processors.len() <= all_processors.len());
    assert!(
        default_processors
            .iter()
            .all(|processor| observed_ids.contains(&processor.id()))
    );

    // The thread executing this test can only be on a processor the job permits, which the
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

    drop(job);
}

/// Returns the IDs of the processors the current process is permitted to use, in ascending
/// order.
///
/// This asks the operating system directly instead of going through the package under test, so
/// that the package makes its first observation of the hardware under the job affinity limit.
/// The caller guarantees that the machine has a single processor group, so the affinity mask of
/// the process describes every processor of the machine and the position of a bit in it is the
/// ID of the processor it selects.
fn available_processor_ids() -> Vec<ProcessorId> {
    let mut process_mask: usize = 0;
    let mut system_mask: usize = 0;

    // SAFETY: No safety requirements. The handle does not need to be closed.
    let current_process = unsafe { GetCurrentProcess() };

    // SAFETY: Both output buffers are live local variables for the duration of the call.
    unsafe {
        GetProcessAffinityMask(current_process, &raw mut process_mask, &raw mut system_mask)
            .unwrap();
    }

    // With a single processor group, the ID of a processor is the position of its bit in the
    // affinity mask.
    (0..ProcessorId::try_from(usize::BITS).unwrap())
        .filter(|bit| process_mask & (1_usize << bit) != 0)
        .collect()
}

/// Builds a processor affinity mask that selects exactly the given processors.
fn processor_mask(processor_ids: &[ProcessorId]) -> NonZero<usize> {
    let mask = processor_ids.iter().fold(0_usize, |mask, &processor_id| {
        mask | (1_usize << processor_id)
    });

    NonZero::new(mask).unwrap()
}
