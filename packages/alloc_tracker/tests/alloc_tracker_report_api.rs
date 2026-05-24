//! Test the new Report API functionality.

use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[test]
#[cfg_attr(miri, ignore)] // Test uses the real platform which cannot be executed under Miri.
fn alloc_tracker_report_api() {
    let session = Session::new();

    {
        let op = session.operation("test_alloc");
        let _span = op.measure_process().iterations(3);
        // Perform actual allocations
        for _ in 0..3 {
            let _data = [1_u8; 100].to_vec(); // Each allocation creates a Vec
        }
    }

    let report = session.to_report();

    // Test that we can access the data programmatically
    let operations: Vec<_> = report.operations().collect();
    assert_eq!(operations.len(), 1);

    let (name, op) = operations.first().unwrap();
    assert_eq!(*name, "test_alloc");
    assert_eq!(op.total_iterations(), 3);
    // Note: We cannot predict exact allocation amounts due to Vec overhead,
    // but we can verify the API structure
    assert!(op.total_bytes_allocated() > 0);
    assert!(op.mean() > 0);

    println!("✓ alloc_tracker Report API works correctly");
}
