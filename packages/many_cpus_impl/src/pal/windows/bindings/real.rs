use std::fmt::Debug;

use windows::Win32::System::JobObjects::{
    IsProcessInJob, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JobObjectCpuRateControlInformation,
    JobObjectGroupInformationEx, QueryInformationJobObject,
};
use windows::Win32::System::Kernel::PROCESSOR_NUMBER;
use windows::Win32::System::Power::{
    CallNtPowerInformation, PROCESSOR_POWER_INFORMATION, ProcessorInformation,
};
use windows::Win32::System::SystemInformation::{
    GROUP_AFFINITY, GetLogicalProcessorInformationEx, LOGICAL_PROCESSOR_RELATIONSHIP,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows::Win32::System::Threading::{
    GetActiveProcessorCount, GetCurrentProcess, GetCurrentProcessorNumberEx, GetCurrentThread,
    GetMaximumProcessorCount, GetMaximumProcessorGroupCount, GetNumaHighestNodeNumber,
    GetProcessDefaultCpuSetMasks, GetThreadGroupAffinity, GetThreadSelectedCpuSetMasks,
    SetThreadGroupAffinity, SetThreadSelectedCpuSetMasks,
};
use windows::core::{BOOL, Result};

use crate::pal::windows::Bindings;

/// FFI bindings that target the real operating system that the build is targeting.
///
/// You would only use different bindings in PAL unit tests that need to use mock bindings.
/// Even then, whenever possible, unit tests should use real bindings for maximum realism.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetBindings;

impl Bindings for BuildTargetBindings {
    fn get_active_processor_count(&self, group_number: u16) -> u32 {
        // SAFETY: No safety requirements.
        unsafe { GetActiveProcessorCount(group_number) }
    }

    fn get_maximum_processor_count(&self, group_number: u16) -> u32 {
        // SAFETY: No safety requirements.
        unsafe { GetMaximumProcessorCount(group_number) }
    }

    fn get_maximum_processor_group_count(&self) -> u16 {
        // SAFETY: No safety requirements.
        unsafe { GetMaximumProcessorGroupCount() }
    }

    fn get_current_processor_number_ex(&self) -> PROCESSOR_NUMBER {
        // SAFETY: No safety requirements.
        unsafe { GetCurrentProcessorNumberEx() }
    }

    unsafe fn get_logical_processor_information_ex(
        &self,
        relationship_type: LOGICAL_PROCESSOR_RELATIONSHIP,
        buffer: Option<*mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>,
        returned_length: *mut u32,
    ) -> Result<()> {
        // SAFETY: Forwarding safety requirements to caller.
        unsafe { GetLogicalProcessorInformationEx(relationship_type, buffer, returned_length) }
    }

    fn get_numa_highest_node_number(&self) -> u32 {
        let mut result: u32 = 0;

        // SAFETY: No safety requirements beyond passing valid input.
        unsafe { GetNumaHighestNodeNumber(&raw mut result) }
            .expect("platform refused to inform us about memory region count");

        result
    }

    fn get_current_process_default_cpu_set_masks(&self) -> Vec<GROUP_AFFINITY> {
        // SAFETY: No safety requirements. Does not require closing the handle.
        let current_process = unsafe { GetCurrentProcess() };

        // TODO: We should cache this, asking this info from the OS can be expensive.
        // TODO: Should we kick this upstream? Though rather annoying low level API for that...
        let max_group_count = self.get_maximum_processor_group_count();

        // The required capacity cannot be greater than the maximum number of processor groups.
        let mut buffer = vec![GROUP_AFFINITY::default(); max_group_count as usize];

        // How many masks from our buffer were actually used. NB! This can be 0 if there is no
        // default CPU set mask applied to the process (which implies all processors are available).
        let mut required_mask_count = 0;

        // SAFETY: No safety requirements beyond passing valid input.
        unsafe {
            GetProcessDefaultCpuSetMasks(
                current_process,
                Some(&mut buffer),
                &raw mut required_mask_count,
            )
        }
        .expect("platform refused to provide the current process default processor affinity");

        buffer.truncate(required_mask_count as usize);
        buffer
    }

    fn get_current_thread_cpu_set_masks(&self) -> Vec<GROUP_AFFINITY> {
        // SAFETY: No safety requirements. Does not require closing the handle.
        let current_thread = unsafe { GetCurrentThread() };

        // TODO: We should cache this, asking this info from the OS can be expensive.
        // TODO: Should we kick this upstream? Though rather annoying low level API for that...
        let max_group_count = self.get_maximum_processor_group_count();

        // The required capacity cannot be greater than the maximum number of processor groups.
        let mut buffer = vec![GROUP_AFFINITY::default(); max_group_count as usize];

        // How many masks from our buffer were actually used. NB! This can be 0 if there is no
        // default CPU set mask applied to the process (which implies all processors are available).
        let mut required_mask_count = 0;

        // SAFETY: No safety requirements beyond passing valid input.
        unsafe {
            GetThreadSelectedCpuSetMasks(
                current_thread,
                Some(&mut buffer),
                &raw mut required_mask_count,
            )
        }
        .expect("platform refused to provide the current process default processor affinity");

        buffer.truncate(required_mask_count as usize);
        buffer
    }

    fn set_current_thread_cpu_set_masks(&self, masks: &[GROUP_AFFINITY]) {
        // SAFETY: No safety requirements. Does not require closing the handle.
        let current_thread = unsafe { GetCurrentThread() };

        // SAFETY: No safety requirements beyond passing valid input.
        unsafe { SetThreadSelectedCpuSetMasks(current_thread, Some(masks)) }
            .expect("platform refused to accept a new current thread processor affinity");
    }

    // Excluded from coverage because the "not in job" branches cannot be tested in automation,
    // as automated test runs are always executed within a job.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn get_current_job_cpu_set_masks(&self) -> Vec<GROUP_AFFINITY> {
        // SAFETY: No safety requirements. Does not require closing the handle.
        let current_process = unsafe { GetCurrentProcess() };

        let mut result: BOOL = BOOL::default();

        // SAFETY: No safety requirements beyond passing valid inputs.
        unsafe {
            IsProcessInJob(current_process, None, &raw mut result).expect(
                "platform refused to confirm or deny whether the current process is part of a job",
            );
        }

        if !result.as_bool() {
            // If not part of a job, no limits apply.
            return Vec::new();
        }

        let mut buffer =
            vec![GROUP_AFFINITY::default(); self.get_maximum_processor_group_count() as usize];

        let mut bytes_written: u32 = 0;

        let buffer_len_items: u32 = buffer.len().try_into().expect(
            "platform does not support more than u32 processor groups, so this can never overflow",
        );

        let size_of_group_affinity = size_of::<GROUP_AFFINITY>()
            .try_into()
            .expect("struct of known size guaranteed to fit in u32");

        let buffer_len_bytes = buffer_len_items.checked_mul(size_of_group_affinity)
            .expect("even under extreme processor group counts, we cannot overflow u32 by having too many GROUP_AFFINITYs");

        // SAFETY: No safety requirements beyond passing valid inputs.
        unsafe {
            QueryInformationJobObject(
                None,
                JobObjectGroupInformationEx,
                buffer.as_mut_ptr().cast(),
                buffer_len_bytes,
                Some(&raw mut bytes_written),
            )
        }
        .expect("platform refused to provide the process's current job processor affinity");

        buffer.truncate(
            bytes_written
                .checked_div(
                    size_of::<GROUP_AFFINITY>()
                        .try_into()
                        .expect("struct of known size guaranteed to fit in u32"),
                )
                .expect("GROUP_AFFINITY is not a ZST, so there can be no division by zero")
                as usize,
        );
        buffer
    }

    fn get_current_thread_legacy_group_affinity(&self) -> GROUP_AFFINITY {
        // SAFETY: No safety requirements. Does not require closing the handle.
        let current_thread = unsafe { GetCurrentThread() };

        let mut aff = GROUP_AFFINITY::default();

        // SAFETY: No safety requirements.
        unsafe { GetThreadGroupAffinity(current_thread, &raw mut aff) }
            .expect("platform refused to provide the current thread's legacy processor affinity");

        aff
    }

    fn get_processor_group_max_mhz(&self, group_number: u16, group_size: usize) -> Vec<u32> {
        // `CallNtPowerInformation(ProcessorInformation)` fills one PROCESSOR_POWER_INFORMATION per
        // logical processor, ordered by the processor's index within its group. `MaxMhz` is the
        // nominal maximum clock frequency which - unlike `CurrentMhz` - is stable and does not
        // fluctuate with power management or thermal throttling, making it a reliable value for
        // identifying processors.
        //
        // This is a legacy API that predates processor groups: it only reports the processors in
        // the processor group that the calling thread currently belongs to. To read every group on
        // systems with more than one processor group (more than 64 logical processors) we move this
        // thread into `group_number` for the duration of the query and restore its original group
        // affinity afterwards. The caller is responsible for querying every group in turn.

        // SAFETY: No safety requirements. Does not require closing the handle.
        let current_thread = unsafe { GetCurrentThread() };

        // The caller guarantees the group has at least one active processor, and active processors
        // occupy the low index positions, so the first processor in the group is always a valid
        // target. We only need to enter the group to observe it, not to schedule real work.
        let target_affinity = GROUP_AFFINITY {
            Mask: 1,
            Group: group_number,
            ..Default::default()
        };

        let mut previous_affinity = GROUP_AFFINITY::default();

        // SAFETY: All pointers we pass are valid for the duration of the call.
        unsafe {
            SetThreadGroupAffinity(
                current_thread,
                &raw const target_affinity,
                Some(&raw mut previous_affinity),
            )
        }
        .expect("platform refused to move the current thread to the target processor group");

        let mut buffer = vec![PROCESSOR_POWER_INFORMATION::default(); group_size];

        let buffer_len_bytes: u32 = group_size
            .checked_mul(size_of::<PROCESSOR_POWER_INFORMATION>())
            .and_then(|len| u32::try_from(len).ok())
            .expect("processor power information buffer size overflowed u32");

        // SAFETY: `ProcessorInformation` requires no input buffer; we provide an output buffer
        // large enough for `group_size` entries and declare its exact size in bytes.
        let query_result = unsafe {
            CallNtPowerInformation(
                ProcessorInformation,
                None,
                0,
                Some(buffer.as_mut_ptr().cast()),
                buffer_len_bytes,
            )
        }
        .ok();

        // Restore the original affinity before reacting to any query failure, so we never leave
        // the thread stranded in a processor group it did not start in.
        // SAFETY: The pointer we pass is valid for the duration of the call.
        unsafe { SetThreadGroupAffinity(current_thread, &raw const previous_affinity, None) }
            .expect("platform refused to restore the current thread's original processor group");

        query_result.expect("platform refused to provide processor power information");

        // The array holds one entry per processor in the group, ordered by the processor's index
        // within the group, so we surface `MaxMhz` in that same order for the caller to map onto
        // global processor IDs.
        buffer.iter().map(|info| info.MaxMhz).collect()
    }

    // Excluded from coverage because the "not in job" branches cannot be tested in automation,
    // as automated test runs are always executed within a job.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn get_current_job_cpu_rate_control(&self) -> Option<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION> {
        // SAFETY: No safety requirements. Does not require closing the handle.
        let current_process = unsafe { GetCurrentProcess() };

        let mut result: BOOL = BOOL::default();

        // SAFETY: No safety requirements beyond passing valid inputs.
        unsafe {
            IsProcessInJob(current_process, None, &raw mut result).expect(
                "platform refused to confirm or deny whether the current process is part of a job",
            );
        }

        if !result.as_bool() {
            // If not part of a job, no rate control constraints apply.
            return None;
        }

        let mut result = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
        let result_size_u32 = size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>()
            .try_into()
            .expect("struct of known size guaranteed to fit in u32");

        // SAFETY: No safety requirements beyond passing valid inputs.
        unsafe {
            QueryInformationJobObject(
                None,
                JobObjectCpuRateControlInformation,
                (&raw mut result).cast(),
                result_size_u32,
                None,
            )
        }
        .expect("platform refused to provide the process's current job processor time constraints");

        Some(result)
    }
}
