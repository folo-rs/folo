use std::fmt::Debug;

use windows::{
    core::Result,
    Win32::System::{
        Kernel::PROCESSOR_NUMBER,
        SystemInformation::{
            GROUP_AFFINITY, LOGICAL_PROCESSOR_RELATIONSHIP, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
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

    fn get_current_processor_number_ex(&self) -> PROCESSOR_NUMBER;

    fn get_numa_highest_node_number(&self) -> u32;

    fn get_current_process_default_cpu_set_masks(&self) -> Vec<GROUP_AFFINITY>;
    fn get_current_thread_cpu_set_masks(&self) -> Vec<GROUP_AFFINITY>;
    fn set_current_thread_cpu_set_masks(&self, masks: &[GROUP_AFFINITY]);

    unsafe fn get_logical_processor_information_ex(
        &self,
        relationship_type: LOGICAL_PROCESSOR_RELATIONSHIP,
        buffer: Option<*mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>,
        returned_length: *mut u32,
    ) -> Result<()>;

    // JobObjectGroupInformationEx; may return empty list if not affinitized.
    fn get_current_job_cpu_set_masks(&self) -> Vec<GROUP_AFFINITY>;
}
