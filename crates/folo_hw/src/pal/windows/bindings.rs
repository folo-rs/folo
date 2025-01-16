use std::{fmt::Debug, hash::Hash};

use windows::{
    core::Result,
    Win32::{
        Foundation::{BOOL, HANDLE},
        System::{
            SystemInformation::{
                GetLogicalProcessorInformationEx, GROUP_AFFINITY, LOGICAL_PROCESSOR_RELATIONSHIP,
                SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
            },
            Threading::{
                GetActiveProcessorCount, GetCurrentThread, GetMaximumProcessorCount,
                GetMaximumProcessorGroupCount, SetThreadGroupAffinity,
            },
        },
    },
};

/// Bindings for FFI calls into external libraries (either provided by operating system or not).
///
/// All PAL FFI calls must go through this trait, enabling them to be mocked.
#[expect(non_snake_case)] // Following Windows API naming.
pub(super) trait Bindings:
    Clone + Copy + Debug + Eq + Ord + Hash + PartialEq + PartialOrd + Send + Sync + 'static
{
    fn GetActiveProcessorCount(&self, group_number: u16) -> u32;
    fn GetMaximumProcessorCount(&self, group_number: u16) -> u32;

    fn GetMaximumProcessorGroupCount(&self) -> u16;

    fn GetCurrentThread(&self) -> HANDLE;

    unsafe fn SetThreadGroupAffinity(
        &self,
        thread: HANDLE,
        group_affinity: *const GROUP_AFFINITY,
        previous_group_affinity: Option<*mut GROUP_AFFINITY>,
    ) -> BOOL;

    unsafe fn GetLogicalProcessorInformationEx(
        &self,
        relationship_type: LOGICAL_PROCESSOR_RELATIONSHIP,
        buffer: Option<*mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>,
        returned_length: *mut u32,
    ) -> Result<()>;
}

#[derive(Copy, Clone, Debug, Default, Eq, Ord, Hash, PartialEq, PartialOrd)]
pub(super) struct BindingsImpl;

impl Bindings for BindingsImpl {
    fn GetActiveProcessorCount(&self, group_number: u16) -> u32 {
        // SAFETY: No safety requirements.
        unsafe { GetActiveProcessorCount(group_number) }
    }

    fn GetMaximumProcessorCount(&self, group_number: u16) -> u32 {
        // SAFETY: No safety requirements.
        unsafe { GetMaximumProcessorCount(group_number) }
    }

    fn GetMaximumProcessorGroupCount(&self) -> u16 {
        // SAFETY: No safety requirements.
        unsafe { GetMaximumProcessorGroupCount() }
    }

    fn GetCurrentThread(&self) -> HANDLE {
        // SAFETY: No safety requirements.
        unsafe { GetCurrentThread() }
    }

    unsafe fn SetThreadGroupAffinity(
        &self,
        thread: HANDLE,
        group_affinity: *const GROUP_AFFINITY,
        previous_group_affinity: Option<*mut GROUP_AFFINITY>,
    ) -> BOOL {
        SetThreadGroupAffinity(thread, group_affinity, previous_group_affinity)
    }

    unsafe fn GetLogicalProcessorInformationEx(
        &self,
        relationship_type: LOGICAL_PROCESSOR_RELATIONSHIP,
        buffer: Option<*mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>,
        returned_length: *mut u32,
    ) -> Result<()> {
        GetLogicalProcessorInformationEx(relationship_type, buffer, returned_length)
    }
}
