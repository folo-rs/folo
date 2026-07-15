use std::fmt::Debug;
use std::iter;

use windows::Win32::System::JobObjects::{
    IsProcessInJob, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JobObjectCpuRateControlInformation,
    JobObjectGroupInformationEx, QueryInformationJobObject,
};
use windows::Win32::System::Kernel::PROCESSOR_NUMBER;
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::SystemInformation::{
    GROUP_AFFINITY, GetLogicalProcessorInformationEx, LOGICAL_PROCESSOR_RELATIONSHIP,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows::Win32::System::Threading::{
    GetActiveProcessorCount, GetCurrentProcess, GetCurrentProcessorNumberEx, GetCurrentThread,
    GetMaximumProcessorCount, GetMaximumProcessorGroupCount, GetNumaHighestNodeNumber,
    GetProcessDefaultCpuSetMasks, GetThreadGroupAffinity, GetThreadSelectedCpuSetMasks,
    SetThreadSelectedCpuSetMasks,
};
use windows::core::{BOOL, PCWSTR, Result};

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

    fn get_processor_max_mhz(&self, max_processor_count: usize) -> Vec<u32> {
        // Windows records the nominal maximum clock frequency of each logical processor in the
        // registry under `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\<id>` as the `~MHz`
        // value. The kernel populates these entries at boot for every logical processor across all
        // processor groups, so reading them is a fully passive operation: it never changes the
        // affinity or any other runtime state of the calling thread and it is not limited to the
        // 64 processors of a single processor group the way the legacy power-information API is.
        //
        // The `~MHz` value is the nominal maximum clock and - unlike a live "current MHz" reading -
        // is stable and does not fluctuate with power management or thermal throttling, making it a
        // reliable value for identifying processors.
        (0..max_processor_count)
            .map(read_processor_nominal_max_mhz)
            .collect()
    }

    fn get_processor_name_strings(&self, max_processor_count: usize) -> Vec<Option<String>> {
        // Windows records the brand string of each logical processor in the registry under
        // `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\<id>` as the `ProcessorNameString`
        // value. The kernel populates these entries at boot for every logical processor across all
        // processor groups, so reading them is a fully passive operation: it never changes the
        // affinity or any other runtime state of the calling thread and it is not limited to the
        // 64 processors of a single processor group the way the legacy power-information API is.
        (0..max_processor_count)
            .map(read_processor_name_string)
            .collect()
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

/// Reads the nominal maximum clock frequency in MHz that Windows recorded for a single logical
/// processor in the registry, returning 0 when the platform records no value for that processor.
///
/// The value lives at `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\<processor_id>\~MHz` and
/// is populated by the kernel at boot for every logical processor, so this read is passive and
/// covers all processor groups without touching any thread's affinity.
fn read_processor_nominal_max_mhz(processor_id: usize) -> u32 {
    let subkey = to_nul_terminated_wide(&format!(
        "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\{processor_id}"
    ));
    let value_name = to_nul_terminated_wide("~MHz");

    let mut value: u32 = 0;
    let mut value_size_bytes: u32 =
        u32::try_from(size_of::<u32>()).expect("the size of a u32 always fits in a u32");

    // SAFETY: `subkey` and `value_name` are NUL-terminated UTF-16 strings that outlive the call.
    // `value` and `value_size_bytes` are valid for writes for the duration of the call, and
    // `value_size_bytes` truthfully declares the size of the `value` buffer. `RRF_RT_REG_DWORD`
    // restricts the query to REG_DWORD values, matching the `u32` buffer we provide.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut value_size_bytes),
        )
    };

    // A missing or unreadable entry (for example an offline processor the kernel never populated)
    // is reported as 0, which the caller maps onto the synthetic relative speed.
    if status.is_ok() { value } else { 0 }
}

/// Reads the brand string that Windows recorded for a single logical processor in the registry,
/// returning `None` when the platform records no value for that processor.
///
/// The value lives at
/// `HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\<processor_id>\ProcessorNameString` and is
/// populated by the kernel at boot for every logical processor, so this read is passive and covers
/// all processor groups without touching any thread's affinity.
//
// Excluded from coverage because the failure branches (an unreadable or missing registry value, or
// an empty brand string) cannot be exercised in automation: the kernel always records a non-empty
// `ProcessorNameString` for every logical processor on the machines that run our tests.
#[cfg_attr(coverage_nightly, coverage(off))]
fn read_processor_name_string(processor_id: usize) -> Option<String> {
    let subkey = to_nul_terminated_wide(&format!(
        "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\{processor_id}"
    ));
    let value_name = to_nul_terminated_wide("ProcessorNameString");

    // First query how many bytes the value occupies so we can size the receiving buffer. Passing a
    // null data pointer with a valid size-out pointer requests only the required size.
    let mut value_size_bytes: u32 = 0;

    // SAFETY: `subkey` and `value_name` are NUL-terminated UTF-16 strings that outlive the call.
    // The data pointer is null (we only want the size) and `value_size_bytes` is valid for writes
    // for the duration of the call. `RRF_RT_REG_SZ` restricts the query to REG_SZ values.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&raw mut value_size_bytes),
        )
    };

    if status.is_err() || value_size_bytes == 0 {
        return None;
    }

    // The size is in bytes but the buffer holds UTF-16 code units, so round up when dividing.
    let mut buffer: Vec<u16> = vec![0; (value_size_bytes as usize).div_ceil(size_of::<u16>())];

    // SAFETY: `subkey` and `value_name` are NUL-terminated UTF-16 strings that outlive the call.
    // `buffer` is valid for writes of `value_size_bytes` bytes for the duration of the call and
    // `value_size_bytes` truthfully declares the buffer size. `RRF_RT_REG_SZ` restricts the query
    // to REG_SZ values, matching the UTF-16 buffer we provide.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut value_size_bytes),
        )
    };

    if status.is_err() {
        return None;
    }

    // Trim the trailing NUL terminator that the registry API guarantees, then trim any surrounding
    // whitespace that some firmware pads the brand string with. Everything the API wrote after the
    // NUL (and the zero-initialized tail of the buffer) is excluded by stopping at the first NUL.
    let units: Vec<u16> = buffer.into_iter().take_while(|&unit| unit != 0).collect();
    let text = String::from_utf16_lossy(&units);
    let trimmed = text.trim();

    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Encodes a string as a NUL-terminated wide (UTF-16) string, as expected by the wide-character
/// Windows registry API.
fn to_nul_terminated_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(iter::once(0)).collect()
}
