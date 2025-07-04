//! Integration tests for `cpu_time_tracker`

use cpu_time_tracker::Session;
use std::time::Duration;

#[test]
fn session_integration() {
    let mut session = Session::new();

    // Test that we can create operations and track time
    {
        let op1 = session.operation("test_operation_1");
        let _span = op1.thread_span();
        // Some work
        let mut sum = 0;
        for i in 0..1000 {
            sum += i;
        }
        std::hint::black_box(sum);
    }

    {
        let op2 = session.operation("test_operation_2");
        let _span = op2.thread_span();
        // Some different work
        let mut sum = 0;
        for i in 0..500 {
            sum += i * 2;
        }
        std::hint::black_box(sum);
    }

    // Check that operations exist and have tracked time
    {
        let op1 = session.operation("test_operation_1");
        assert_eq!(op1.spans(), 1);
        assert!(op1.total_cpu_time() >= Duration::ZERO);
    }

    {
        let op2 = session.operation("test_operation_2");
        assert_eq!(op2.spans(), 1);
        assert!(op2.total_cpu_time() >= Duration::ZERO);
    }

    assert!(!session.is_empty());
}

#[test]
fn multiple_spans_per_operation() {
    let mut session = Session::new();
    let op = session.operation("multi_span_operation");

    // First span
    {
        let _span = op.thread_span();
        std::hint::black_box(42);
    }

    // Second span
    {
        let _span = op.thread_span();
        std::hint::black_box(84);
    }

    // Third span
    {
        let _span = op.thread_span();
        std::hint::black_box(126);
    }

    assert_eq!(op.spans(), 3);
    assert!(op.total_cpu_time() >= Duration::ZERO);
    assert!(op.average() >= Duration::ZERO);
}

#[test]
fn empty_session() {
    let session = Session::new();
    assert!(session.is_empty());
}

#[test]
fn session_with_operations_but_no_spans() {
    let mut session = Session::new();
    let _op = session.operation("unused_operation");
    // Don't create any spans
    assert!(session.is_empty());
}
