//! Job objects can impose various limits on the Windows processes they govern. These tests
//! verify that the logic in our crate correctly detects these limits and behaves accordingly
//! to keep its own behavior within those limits.
//!
//! NB! We `ProcessorSet::builder()` in all cases in this file to avoid `ProcessorSet::default()`
//! which in the current implementation gets cached on first access - we do not want to cache
//! anything from our tests here as they apply weird constants, and we also do not want to use
//! a cached processor set from some other test, since it will not have had our constraints applied.

#![cfg(windows)]

use many_cpus::{HardwareTracker, ProcessorSet};
use new_zealand::nz;
use testing::{Job, ProcessorTimePct, f64_diff_abs};

// Floating point comparison tolerance.
// https://rust-lang.github.io/rust-clippy/master/index.html#float_cmp
const CLOSE_ENOUGH: f64 = 0.01;

#[test]
#[cfg_attr(miri, ignore)] // Miri cannot talk to the real platform.
#[expect(
    clippy::float_cmp,
    reason = "we use absolute error, which is the right way to compare"
)]
fn obeys_processor_selection_limits() {
    if ProcessorSet::builder()
        .ignoring_resource_quota()
        .take_all()
        .unwrap()
        .len()
        <= 2
    {
        eprintln!("Skipping test: not enough processors available.");
        return;
    }

    // Restrict the current process to only use 2 processors for the duration of this test.
    let job = Job::builder().with_processor_count(nz!(2)).build();

    let processor_count = ProcessorSet::builder().take_all().unwrap().len();
    assert_eq!(processor_count, 2);

    // This must also constrain the processor time quota to 2 processors.
    // We require at least 3 processors to run this test, so without correct
    // limiting the behavior would round up and say at least 3.
    let resource_quota = HardwareTracker::resource_quota();

    assert_eq!(
        f64_diff_abs(resource_quota.max_processor_time(), 2.0, CLOSE_ENOUGH),
        0.0,
        "The resource quota should limit the number of processors to 2.0 processor-seconds per second. Expected: 2.0, Actual: {}",
        resource_quota.max_processor_time()
    );

    drop(job);
}

#[test]
#[cfg_attr(miri, ignore)] // Miri cannot talk to the real platform.
#[expect(
    clippy::cast_precision_loss,
    reason = "all expected values are in safe range"
)]
#[expect(
    clippy::float_cmp,
    reason = "we use absolute error, which is the right way to compare"
)]
fn obeys_processor_time_limits() {
    // Restrict the current process to only use 50% of the system processor time.
    let job = Job::builder()
        .with_max_processor_time_pct(ProcessorTimePct::new_static::<50>())
        .build();

    // This is "100%". This count may also include processors that are not available to the
    // current process (e.g. when job objects already constrain our processors due to
    // executing in a container).
    let system_processor_count = HardwareTracker::active_processor_count();

    let resource_quota = HardwareTracker::resource_quota();

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
    let quota_limited_processor_count = ProcessorSet::builder().take_all().unwrap().len();

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

#[test]
#[cfg_attr(miri, ignore)] // Miri cannot talk to the real platform.
#[expect(
    clippy::float_cmp,
    reason = "we use absolute error, which is the right way to compare"
)]
fn noop_job_has_no_effect() {
    let unconstrained_processor_count = ProcessorSet::builder()
        .ignoring_resource_quota()
        .take_all()
        .unwrap()
        .len();

    // Create a job with no limits. This should not affect the current process.
    let job = Job::builder().build();

    let processor_count = ProcessorSet::builder().take_all().unwrap().len();
    assert_eq!(processor_count, unconstrained_processor_count);

    let resource_quota = HardwareTracker::resource_quota();

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
