//! Job objects can impose various limits on the Windows processes they govern. These tests
//! verify that the logic in our crate correctly detects these limits and behaves accordingly
//! to keep its own behavior within those limits.
//!
//! One test per file to enforce process isolation (job objects are process-level state).

#![cfg(windows)]

use many_cpus::SystemHardware;
use new_zealand::nz;
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
fn obeys_processor_selection_limits() {
    let hw = SystemHardware::current();

    if hw.all_processors().len() <= 2 {
        eprintln!("Skipping test: not enough processors available.");
        return;
    }

    // Restrict the current process to only use 2 processors for the duration of this test.
    let job = Job::builder().processor_count(nz!(2)).build();

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
