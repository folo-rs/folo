//! Job objects can impose various limits on the Windows processes they govern. These tests
//! verify that the logic in our crate correctly detects these limits and behaves accordingly
//! to keep its own behavior within those limits.

#![cfg(windows)]

use std::sync::Mutex;

use folo_utils::nz;
use many_cpus::{HardwareTracker, ProcessorSet};
use testing::{Job, ProcessorTimePct};

/// The tests can only run one at a time because they tinker with process-specific configuration.
static MUTUALLY_EXCLUSIVE: Mutex<()> = Mutex::new(());

#[test]
fn obeys_processor_selection_limits() {
    let _guard = MUTUALLY_EXCLUSIVE.lock().unwrap();

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

    let processor_count = ProcessorSet::all().len();
    assert_eq!(processor_count, 2);

    drop(job);
}

#[test]
#[expect(clippy::cast_precision_loss, reason = "unavoidable f64-usize casting")]
fn obeys_processor_time_limits() {
    let _guard = MUTUALLY_EXCLUSIVE.lock().unwrap();

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

    let expected_processor_time = f64::from(system_processor_count) * 0.5;

    assert!(
        processor_time_eq(max_processor_time, expected_processor_time),
        "The resource quota should be 50% of the available processor time. Expected: {expected_processor_time}, Actual: {max_processor_time}",
    );

    // Building a ProcessorSet will obey the resource quota by default.
    let quota_limited_processor_count = ProcessorSet::builder().take_all().unwrap().len();

    let expected_limited_processor_count = (f64::from(system_processor_count) * 0.5).ceil();

    assert!(
        processor_time_eq(
            expected_limited_processor_count,
            quota_limited_processor_count as f64
        ),
        "The resource quota should limit the number of processors to half of the available processors, rounded up. Expected: {expected_limited_processor_count}, Actual: {quota_limited_processor_count}",
    );

    drop(job);
}

fn processor_time_eq(a: f64, b: f64) -> bool {
    // Floating point comparison tolerance.
    // https://rust-lang.github.io/rust-clippy/master/index.html#float_cmp
    const CLOSE_ENOUGH: f64 = 0.01;

    let diff = (a - b).abs();
    diff < CLOSE_ENOUGH
}
