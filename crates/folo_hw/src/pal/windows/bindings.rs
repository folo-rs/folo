use std::fmt::Debug;

#[cfg(test)]
use std::sync::Arc;

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
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Bindings: Debug + Send + Sync + 'static {
    fn get_active_processor_count(&self, group_number: u16) -> u32;
    fn get_maximum_processor_count(&self, group_number: u16) -> u32;

    fn get_maximum_processor_group_count(&self) -> u16;

    fn get_current_thread(&self) -> HANDLE;

    unsafe fn set_thread_group_affinity(
        &self,
        thread: HANDLE,
        group_affinity: *const GROUP_AFFINITY,
        previous_group_affinity: Option<*mut GROUP_AFFINITY>,
    ) -> BOOL;

    unsafe fn get_logical_processor_information_ex(
        &self,
        relationship_type: LOGICAL_PROCESSOR_RELATIONSHIP,
        buffer: Option<*mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>,
        returned_length: *mut u32,
    ) -> Result<()>;
}

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
}

/// Enum to hide the different binding implementations behind a single wrapper type.
#[derive(Clone, Debug)]
pub(super) enum BindingsImpl {
    Real(&'static BuildTargetBindings),

    #[cfg(test)]
    Mock(Arc<MockBindings>),
}

impl Bindings for BindingsImpl {
    fn get_active_processor_count(&self, group_number: u16) -> u32 {
        match self {
            BindingsImpl::Real(bindings) => bindings.get_active_processor_count(group_number),
            #[cfg(test)]
            BindingsImpl::Mock(bindings) => bindings.get_active_processor_count(group_number),
        }
    }

    fn get_maximum_processor_count(&self, group_number: u16) -> u32 {
        match self {
            BindingsImpl::Real(bindings) => bindings.get_maximum_processor_count(group_number),
            #[cfg(test)]
            BindingsImpl::Mock(bindings) => bindings.get_maximum_processor_count(group_number),
        }
    }

    fn get_maximum_processor_group_count(&self) -> u16 {
        match self {
            BindingsImpl::Real(bindings) => bindings.get_maximum_processor_group_count(),
            #[cfg(test)]
            BindingsImpl::Mock(bindings) => bindings.get_maximum_processor_group_count(),
        }
    }

    fn get_current_thread(&self) -> HANDLE {
        match self {
            BindingsImpl::Real(bindings) => bindings.get_current_thread(),
            #[cfg(test)]
            BindingsImpl::Mock(bindings) => bindings.get_current_thread(),
        }
    }

    unsafe fn set_thread_group_affinity(
        &self,
        thread: HANDLE,
        group_affinity: *const GROUP_AFFINITY,
        previous_group_affinity: Option<*mut GROUP_AFFINITY>,
    ) -> BOOL {
        match self {
            BindingsImpl::Real(bindings) => {
                bindings.set_thread_group_affinity(thread, group_affinity, previous_group_affinity)
            }
            #[cfg(test)]
            BindingsImpl::Mock(bindings) => {
                bindings.set_thread_group_affinity(thread, group_affinity, previous_group_affinity)
            }
        }
    }

    unsafe fn get_logical_processor_information_ex(
        &self,
        relationship_type: LOGICAL_PROCESSOR_RELATIONSHIP,
        buffer: Option<*mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>,
        returned_length: *mut u32,
    ) -> Result<()> {
        match self {
            BindingsImpl::Real(bindings) => bindings.get_logical_processor_information_ex(
                relationship_type,
                buffer,
                returned_length,
            ),
            #[cfg(test)]
            BindingsImpl::Mock(bindings) => bindings.get_logical_processor_information_ex(
                relationship_type,
                buffer,
                returned_length,
            ),
        }
    }
}
