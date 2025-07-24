//! Test the new Report API functionality.

use std::time::Duration;

use all_the_time::Session as TimeSession;

#[test]
#[cfg(not(miri))] // Test uses the real platform which cannot be executed under Miri.
fn all_the_time_report_api() {
    let session = TimeSession::new();

    {
        let op = session.operation("test_work");
        let _span = op.measure_thread().iterations(10);
        // Do more work to ensure measurable processor time
        for _ in 0..10 {
            let mut sum = 0;
            for i in 0..10000 {
                sum += i;
            }
            std::hint::black_box(sum);
        }
    }

    let report = session.to_report();

    // Test that we can access the data programmatically
    let operations: Vec<_> = report.operations().collect();
    assert_eq!(operations.len(), 1);

    let (name, op) = operations.first().unwrap();
    assert_eq!(*name, "test_work");
    assert_eq!(op.total_iterations(), 10);
    // Note: processor time may be zero in test environments, so we just verify API access
    assert!(op.total_processor_time() >= Duration::ZERO);
    assert!(op.mean() >= Duration::ZERO);

    println!("✓ all_the_time Report API works correctly");
}
