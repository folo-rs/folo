use std::num::NonZero;
use std::ptr;
use std::sync::{Mutex, MutexGuard};

use deranged::int;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL,
    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
    JOB_OBJECT_LIMIT_AFFINITY, JOBOBJECT_BASIC_LIMIT_INFORMATION,
    JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectCpuRateControlInformation,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::Threading::GetCurrentProcess;

/// Jobs tinker with process-specific configuration so they are mutually exclusive.
static MUTUALLY_EXCLUSIVE: Mutex<()> = Mutex::new(());

/// Represents the duration over which the current process is subject to custom job limits.
///
/// The limits are applied via `JobBuilder` and released when the `Job` is dropped. Note that
/// the process remains associated with the job, it simply no longer has any limits applied.
#[derive(Debug)]
pub struct Job<'a> {
    rate_control_active: bool,
    affinity_active: bool,

    handle: HANDLE,

    #[expect(
        dead_code,
        reason = "we just want to keep it alive until we drop the job"
    )]
    mutex_guard: MutexGuard<'a, ()>,
}

impl Job<'_> {
    /// Starts building a new job to apply to the current process.
    #[must_use]
    pub fn builder() -> JobBuilder {
        JobBuilder::default()
    }
}

impl Drop for Job<'_> {
    fn drop(&mut self) {
        // It is not possible to remove a process from a job, so the best we can do in terms
        // of cleanup is to reconfigure the job to remove all limits. This is clunky but
        // good enough for example/test purposes. The only time we really care about removing
        // the limits is when we are reusing one process for executing multiple tests.

        // The system may refuse a "reset" of something that was never set.
        if self.rate_control_active {
            let limit = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();

            // SAFETY: No safety requirements as long as we pass valid inputs.
            unsafe {
                SetInformationJobObject(
                    self.handle,
                    JobObjectCpuRateControlInformation,
                    ptr::from_ref(&limit).cast(),
                    size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>()
                        .try_into()
                        .expect("struct of known size guaranteed to fit in u32"),
                )
                .unwrap();
            }
        }

        // The system may refuse a "reset" of something that was never set.
        if self.affinity_active {
            let limit = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();

            // SAFETY: No safety requirements, as long as we pass valid inputs.
            unsafe {
                SetInformationJobObject(
                    self.handle,
                    JobObjectExtendedLimitInformation,
                    ptr::from_ref(&limit).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()
                        .try_into()
                        .expect("struct of known size guaranteed to fit in u32"),
                )
                .unwrap();
            }
        }

        // SAFETY: No safety requirements.
        unsafe {
            CloseHandle(self.handle).unwrap();
        }
    }
}

/// Percentage of processor time that the job is allowed to use (1..=100).
pub type ProcessorTimePct = int!(1, 100);

/// Configures instances of [`Job`] before creation.
#[derive(Debug, Default)]
pub struct JobBuilder {
    processor_count: Option<NonZero<u32>>,
    max_processor_time_pct: Option<ProcessorTimePct>,
}

impl JobBuilder {
    /// Creates a new job builder that will not apply any job limits.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Restricts the job to only execute on a specific number of processors.
    ///
    /// The implementation will choose the specific processors, all you can do as caller is
    /// to specify the number of processors.
    #[must_use]
    pub fn with_processor_count(mut self, processor_count: NonZero<u32>) -> Self {
        self.processor_count = Some(processor_count);
        self
    }

    /// Sets the maximum processor time percentage for the job.
    ///
    /// 100% is all available processors on the system, without caring how many of those are usable
    /// to the current process (i.e. this does not consider `with_processor_count()`).
    #[must_use]
    pub fn with_max_processor_time_pct(mut self, pct: ProcessorTimePct) -> Self {
        self.max_processor_time_pct = Some(pct);
        self
    }

    /// Creates the job object and assigns the current process to the job.
    ///
    /// Jobs for testing purposes are mutually exclusive - this function will block if there
    /// already is a job assigned to the current process (only counting jobs created by this type).
    ///
    /// There may also exist externally assigned jobs (e.g. because we are running in a container),
    /// which we ignore here. If both external and internal jobs define limits, the lowest limits
    /// will apply.
    ///
    /// # Panics
    ///
    /// Panics if anything goes wrong.
    #[must_use]
    pub fn build<'a>(self) -> Job<'a> {
        let mutex_guard = MUTUALLY_EXCLUSIVE.lock().unwrap();

        // SAFETY: No safety requirements.
        let job = unsafe { CreateJobObjectW(None, None).unwrap() };
        assert!(!job.is_invalid());

        if let Some(max_processor_time_pct) = self.max_processor_time_pct {
            let limit = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
                ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL(
                    JOB_OBJECT_CPU_RATE_CONTROL_ENABLE.0 | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP.0,
                ),
                Anonymous: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 {
                    CpuRate: u32::from(max_processor_time_pct.get())
                        .checked_mul(100)
                        .expect("cannot overflow here because we constrained input range"),
                },
            };

            // SAFETY: No safety requirements as long as we pass valid inputs.
            unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectCpuRateControlInformation,
                    ptr::from_ref(&limit).cast(),
                    size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>()
                        .try_into()
                        .expect("struct of known size guaranteed to fit in u32"),
                )
                .unwrap();
            }
        }

        if let Some(processor_count) = self.processor_count {
            // We are just going to assume that the primary processor group of this process
            // has a sufficient number of processors to satisfy the request. Good enough
            // for test/example purposes, though obviously not production-quality logic.

            // We set the first `processor_count` bits of the processor affinity mask to 1.
            #[expect(
                clippy::arithmetic_side_effects,
                reason = "side effects are intentional here"
            )]
            let affinity_mask = (1_usize << processor_count.get()) - 1;

            let limit = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_AFFINITY,
                    Affinity: affinity_mask,
                    ..Default::default()
                },
                ..Default::default()
            };

            // SAFETY: No safety requirements, as long as we pass valid inputs.
            unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    ptr::from_ref(&limit).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()
                        .try_into()
                        .expect("struct of known size guaranteed to fit in u32"),
                )
                .unwrap();
            }
        }

        // Configuration complete. Now all that is left to do is assign the current process.

        // SAFETY: No safety requirements. Handle does not need to be closed.
        let current_process = unsafe { GetCurrentProcess() };

        // Assign the suspended process to the job, so our limits are enforced.
        // SAFETY: No safety requirements.
        unsafe {
            AssignProcessToJobObject(job, current_process).unwrap();
        }

        Job {
            handle: job,
            affinity_active: self.processor_count.is_some(),
            rate_control_active: self.max_processor_time_pct.is_some(),
            mutex_guard,
        }
    }
}

#[cfg(test)]
#[cfg(not(miri))] // Miri cannot use the real operating system APIs.
mod tests {
    use new_zealand::nz;

    use super::*;

    #[test]
    fn one_job() {
        let job = Job::builder()
            .with_processor_count(nz!(1))
            .with_max_processor_time_pct(ProcessorTimePct::new_static::<50>())
            .build();

        drop(job);
    }

    #[test]
    fn two_jobs() {
        let job = Job::builder()
            .with_processor_count(nz!(1))
            .with_max_processor_time_pct(ProcessorTimePct::new_static::<50>())
            .build();

        drop(job);

        let job = Job::builder()
            .with_processor_count(nz!(2))
            .with_max_processor_time_pct(ProcessorTimePct::new_static::<60>())
            .build();

        drop(job);
    }
}
