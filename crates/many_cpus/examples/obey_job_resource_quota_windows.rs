//! The mechanism used in Windows to enforce limits on processes is Job Objects. Processes are
//! assigned to jobs, and jobs can be constrained to only use a limited set of processors.
//!
//! This example proves that the APIs we offer can accurately judge the resource quota assigned
//! to the process and follow best practices for processor set sizing when a quota is active.
//!
//! We configure the job object to only grant 50% of the system processor time to the process.
//!
//! This example is Windows-only, as job objects are a Windows-specific feature.

fn main() {
    #[cfg(windows)]
    windows::main();

    #[cfg(not(windows))]
    panic!("This example is only supported on Windows.");
}

#[cfg(windows)]
mod windows {
    use many_cpus::{HardwareTracker, ProcessorSet};
    use testing::{Job, ProcessorTimePct};

    pub(crate) fn main() {
        // Restrict the current process to only use 50% of the system processor time.
        let _job = Job::builder()
            .with_max_processor_time_pct(ProcessorTimePct::new_static::<50>())
            .build();

        verify_limits_obeyed();
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "all expected values are in safe range"
    )]
    fn verify_limits_obeyed() {
        // This is "100%". This count may also include processors that are not available to the
        // current process (e.g. when job objects already constrain our processors due to
        // executing in a container).
        let system_processor_count = HardwareTracker::active_processor_count();

        let resource_quota = HardwareTracker::resource_quota();

        // This should say we are allowed to use 50% of the system processor time, which we
        // express as processor-seconds per second.
        // NB! This can never be higher than our process's max processor time. We rely on the
        // example not having process-specific limits that bring it lower than the 50% here.
        let max_processor_time = resource_quota.max_processor_time();

        println!(
            "Current process is allowed to use {max_processor_time} seconds of processor time per second of real time."
        );

        let expected_processor_time = system_processor_count as f64 * 0.5;

        assert!(
            processor_time_eq(max_processor_time, expected_processor_time),
            "The resource quota should be 50% of the available processor time. Expected: {expected_processor_time}, Actual: {max_processor_time}",
        );

        // The default processor set obeys all the limits that apply to the current process.
        let quota_limited_processor_count = ProcessorSet::default().len();

        println!(
            "The resource quota allows the current process to use {quota_limited_processor_count} out of a total of {system_processor_count} processors."
        );

        let expected_limited_processor_count = (system_processor_count as f64 * 0.5).floor();

        assert!(
            processor_time_eq(
                expected_limited_processor_count,
                quota_limited_processor_count as f64
            ),
            "The resource quota should limit the number of processors to half of the available processors, rounded down. Expected: {expected_limited_processor_count}, Actual: {quota_limited_processor_count}",
        );
    }

    fn processor_time_eq(a: f64, b: f64) -> bool {
        // Floating point comparison tolerance.
        // https://rust-lang.github.io/rust-clippy/master/index.html#float_cmp
        const CLOSE_ENOUGH: f64 = 0.01;

        let diff = (a - b).abs();
        diff < CLOSE_ENOUGH
    }
}
