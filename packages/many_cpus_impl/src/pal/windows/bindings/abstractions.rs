use std::fmt::Debug;

use windows::Win32::System::JobObjects::JOBOBJECT_CPU_RATE_CONTROL_INFORMATION;
use windows::Win32::System::Kernel::PROCESSOR_NUMBER;
use windows::Win32::System::SystemInformation::{
    GROUP_AFFINITY, LOGICAL_PROCESSOR_RELATIONSHIP, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows::core::Result;

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

    fn get_current_job_cpu_rate_control(&self) -> Option<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>;

    /// Returns the nominal maximum clock frequency in MHz of the processors in the processor group
    /// identified by `group_number`, indexed by each processor's index within the group, for up to
    /// `group_size` processors. Processors for which the platform reports no value hold 0.
    ///
    /// The underlying operating system query only reports the calling thread's processor group, so
    /// this moves the calling thread into `group_number` for the duration of the query. The caller
    /// must only pass a `group_number` that has at least one active processor and must query every
    /// group in turn to obtain the values for all processors on the system.
    fn get_processor_group_max_mhz(&self, group_number: u16, group_size: usize) -> Vec<u32>;

    fn get_current_thread_legacy_group_affinity(&self) -> GROUP_AFFINITY;
}
