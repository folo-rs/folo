use std::fmt::Debug;

use windows::{
    core::Result,
    Win32::{
        Foundation::{BOOL, HANDLE},
        System::{
            Kernel::PROCESSOR_NUMBER,
            SystemInformation::{
                GetLogicalProcessorInformationEx, GROUP_AFFINITY, LOGICAL_PROCESSOR_RELATIONSHIP,
                SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
            },
            Threading::{
                GetActiveProcessorCount, GetCurrentProcessorNumberEx, GetCurrentThread,
                GetMaximumProcessorCount, GetMaximumProcessorGroupCount, GetNumaHighestNodeNumber,
                SetThreadGroupAffinity,
            },
        },
    },
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

    fn get_current_thread(&self) -> HANDLE {
        // SAFETY: No safety requirements.
        unsafe { GetCurrentThread() }
    }

    fn get_current_processor_number_ex(&self) -> PROCESSOR_NUMBER {
        // SAFETY: No safety requirements.
        unsafe { GetCurrentProcessorNumberEx() }
    }

    unsafe fn set_thread_group_affinity(
        &self,
        thread: HANDLE,
        group_affinity: *const GROUP_AFFINITY,
        previous_group_affinity: Option<*mut GROUP_AFFINITY>,
    ) -> BOOL {
        SetThreadGroupAffinity(thread, group_affinity, previous_group_affinity)
    }

    unsafe fn get_logical_processor_information_ex(
        &self,
        relationship_type: LOGICAL_PROCESSOR_RELATIONSHIP,
        buffer: Option<*mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>,
        returned_length: *mut u32,
    ) -> Result<()> {
        GetLogicalProcessorInformationEx(relationship_type, buffer, returned_length)
    }

    fn get_numa_highest_node_number(&self) -> u32 {
        let mut result: u32 = 0;

        // SAFETY: No safety requirements beyond passing valid input.
        unsafe { GetNumaHighestNodeNumber(&raw mut result) }
            .expect("platform refused to inform us about memory region count");

        result
    }
}
