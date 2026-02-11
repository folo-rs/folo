//! Job objects can impose various limits on the Windows processes they govern. These tests
//! verify that the logic in our crate correctly detects these limits and behaves accordingly
//! to keep its own behavior within those limits.
//!
//! One test per file to enforce process isolation (job objects are process-level state).

#![cfg(windows)]

use many_cpus::SystemHardware;
use testing::{Job, f64_diff_abs};

// Floating point comparison tolerance.
// https://rust-lang.github.io/rust-clippy/master/index.html#float_cmp
const CLOSE_ENOUGH: f64 = 0.01;

#[test]
#[cfg_attr(miri, ignore)] // Miri cannot call platform APIs.
#[expect(
    clippy::float_cmp,
    reason = "we use absolute error, which is the right way to compare"
)]
fn noop_job_has_no_effect() {
    let hw = SystemHardware::current();

    // NB! The process may already be in a job (e.g. because it is running in a container)!
    // The most we can do is apply additional constraints on top of any existing job we are in.
    // Therefore, we need to take the **constrained** original processor count as the "expected" value.
    let expected_processor_count = hw.processors().len();

    // Create a job with no limits. This should not affect the current process.
    let job = Job::builder().build();

    let processor_count = hw.processors().len();
    assert_eq!(processor_count, expected_processor_count);

    let resource_quota = hw.resource_quota();

    #[expect(clippy::cast_precision_loss, reason = "unavoidable f64-usize casting")]
    {
        assert_eq!(
            f64_diff_abs(
                resource_quota.max_processor_time(),
                expected_processor_count as f64,
                CLOSE_ENOUGH
            ),
            0.0,
            "The resource quota should match the expected processor count: {}, Actual: {}",
            expected_processor_count,
            resource_quota.max_processor_time()
        );
    }

    drop(job);
}
