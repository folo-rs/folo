//! Job objects can impose various limits on the Windows processes they govern. These tests
//! verify that the logic in our crate correctly detects these limits and behaves accordingly
//! to keep its own behavior within those limits.
//!
//! NB! We do not cache any processor set in this file because we apply weird constraints and
//! do not want to interfere with other tests.

#![cfg(windows)]

use many_cpus::SystemHardware;
use new_zealand::nz;
use testing::{Job, ProcessorTimePct, f64_diff_abs};
use serial_test::serial;

// Floating point comparison tolerance.
// https://rust-lang.github.io/rust-clippy/master/index.html#float_cmp
const CLOSE_ENOUGH: f64 = 0.01;

#[serial] // Job objects are global state, so mutual exclusion is necessary.
#[test]
#[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
#[expect(
    clippy::float_cmp,
    reason = "we use absolute error, which is the right way to compare"
)]
fn obeys_processor_selection_limits() {
    let hw = SystemHardware::current();

    if hw.all_processors().len() <= 2 {
        eprintln!("Skipping test: not enough processors available.");
        return;
    }

    // Restrict the current process to only use 2 processors for the duration of this test.
    let job = Job::builder().with_processor_count(nz!(2)).build();

    let processor_count = hw.processors().len();
    assert_eq!(processor_count, 2);

    // This must also constrain the processor time quota to 2 processors.
    // We require at least 3 processors to run this test, so without correct
    // limiting the behavior would round up and say at least 3.
    let resource_quota = hw.resource_quota();

    assert_eq!(
        f64_diff_abs(resource_quota.max_processor_time(), 2.0, CLOSE_ENOUGH),
        0.0,
        "The resource quota should limit the number of processors to 2.0 processor-seconds per second. Expected: 2.0, Actual: {}",
        resource_quota.max_processor_time()
    );

    drop(job);
}

#[serial] // Job objects are global state, so mutual exclusion is necessary.
#[test]
#[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
#[expect(
    clippy::cast_precision_loss,
    reason = "all expected values are in safe range"
)]
#[expect(
    clippy::float_cmp,
    reason = "we use absolute error, which is the right way to compare"
)]
fn obeys_processor_time_limits() {
    let hw = SystemHardware::current();

    // Restrict the current process to only use 50% of the system processor time.
    let job = Job::builder()
        .with_max_processor_time_pct(ProcessorTimePct::new_static::<50>())
        .build();

    // This is "100%". This count may also include processors that are not available to the
    // current process (e.g. when job objects already constrain our processors due to
    // executing in a container).
    let system_processor_count = hw.active_processor_count();

    let resource_quota = hw.resource_quota();

    // This should say we are allowed to use 50% of the system processor time, which we
    // express as processor-seconds per second.
    // NB! This can never be higher than our process's max processor time. We rely on the
    // test not having process-specific limits that bring it lower than the 50% here.
    let max_processor_time = resource_quota.max_processor_time();

    let expected_processor_time = system_processor_count as f64 * 0.5;

    assert_eq!(
        f64_diff_abs(max_processor_time, expected_processor_time, CLOSE_ENOUGH),
        0.0,
        "The resource quota should be 50% of the available processor time. Expected: {expected_processor_time}, Actual: {max_processor_time}",
    );

    // Building a ProcessorSet will obey the resource quota by default.
    let quota_limited_processor_count = hw.processors().len();

    let expected_limited_processor_count = (system_processor_count as f64 * 0.5).ceil();

    assert_eq!(
        f64_diff_abs(
            expected_limited_processor_count,
            quota_limited_processor_count as f64,
            CLOSE_ENOUGH
        ),
        0.0,
        "The resource quota should limit the number of processors to half of the available processors, rounded up. Expected: {expected_limited_processor_count}, Actual: {quota_limited_processor_count}",
    );

    drop(job);
}

#[serial] // Job objects are global state, so mutual exclusion is necessary.
#[test]
#[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
#[expect(
    clippy::float_cmp,
    reason = "we use absolute error, which is the right way to compare"
)]
fn noop_job_has_no_effect() {
    let hw = SystemHardware::current();

    let unconstrained_processor_count = hw.all_processors().len();

    // Create a job with no limits. This should not affect the current process.
    let job = Job::builder().build();

    let processor_count = hw.processors().len();
    assert_eq!(processor_count, unconstrained_processor_count);

    let resource_quota = hw.resource_quota();

    #[expect(clippy::cast_precision_loss, reason = "unavoidable f64-usize casting")]
    {
        assert_eq!(
            f64_diff_abs(
                resource_quota.max_processor_time(),
                unconstrained_processor_count as f64,
                CLOSE_ENOUGH
            ),
            0.0,
            "The resource quota should match the unconstrained processor count: {}, Actual: {}",
            unconstrained_processor_count,
            resource_quota.max_processor_time()
        );
    }

    drop(job);
}
