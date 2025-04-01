use std::{fmt::Debug, mem};

use windows::{
    Win32::System::{
        JobObjects::{IsProcessInJob, JobObjectGroupInformationEx, QueryInformationJobObject},
        Kernel::PROCESSOR_NUMBER,
        SystemInformation::{
            GROUP_AFFINITY, GetLogicalProcessorInformationEx, LOGICAL_PROCESSOR_RELATIONSHIP,
            SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
        },
        Threading::{
            GetActiveProcessorCount, GetCurrentProcess, GetCurrentProcessorNumberEx,
            GetCurrentThread, GetMaximumProcessorCount, GetMaximumProcessorGroupCount,
            GetNumaHighestNodeNumber, GetProcessDefaultCpuSetMasks, GetThreadGroupAffinity,
            GetThreadSelectedCpuSetMasks, SetThreadSelectedCpuSetMasks,
        },
    },
    core::{BOOL, Result},
};

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

    fn get_current_job_cpu_set_masks(&self) -> Vec<GROUP_AFFINITY> {
        // SAFETY: No safety requirements. Does not require closing the handle.
        let current_process = unsafe { GetCurrentProcess() };

        let mut result: BOOL = BOOL::default();

        // SAFETY: No safety requirements beyond passing valid inputs.
        unsafe {
            IsProcessInJob(current_process, None, &raw mut result).expect(
                "platform refused to confirm or deny whether the current process is part of a job",
            )
        }

        if !result.as_bool() {
            // If not part of a job, no limits apply.
            return Vec::new();
        }

        let mut buffer =
            vec![GROUP_AFFINITY::default(); self.get_maximum_processor_group_count() as usize];

        let mut bytes_written: u32 = 0;

        // SAFETY: No safety requirements beyond passing valid inputs.
        unsafe {
            QueryInformationJobObject(
                None,
                JobObjectGroupInformationEx,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32 * mem::size_of::<GROUP_AFFINITY>() as u32,
                Some(&raw mut bytes_written),
            )
        }
        .expect("platform refused to provide the process's current job processor affinity");

        buffer.truncate((bytes_written / mem::size_of::<GROUP_AFFINITY>() as u32) as usize);
        buffer
    }

    fn get_current_thread_legacy_group_affinity(&self) -> GROUP_AFFINITY {
        // SAFETY: No safety requirements. Does not require closing the handle.
        let current_thread = unsafe { GetCurrentThread() };

        let mut aff = GROUP_AFFINITY::default();

        unsafe { GetThreadGroupAffinity(current_thread, &raw mut aff) }
            .expect("platform refused to provide the current thread's legacy processor affinity");

        aff
    }
}
