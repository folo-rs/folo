//! Integration tests for `alloc_tracker` with real memory allocations.
//!
//! These tests use a global allocator setup to test the full functionality
//! of the allocation tracking system, including single-threaded and multithreaded scenarios.

#![cfg(not(miri))] // Miri replaces the global allocator, so cannot be used here.

use std::hint::black_box;
use std::thread;

use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[test]
fn no_span_is_empty_session() {
    let mut session = Session::new();

    _ = session.operation("test_no_span");

    assert!(session.is_empty());
}

#[test]
fn span_with_no_allocation_is_not_empty_session() {
    let mut session = Session::new();

    let op = session.operation("test_no_allocation");

    drop(op.measure_process());

    assert!(
        !session.is_empty(),
        "Session should not be empty after creating a span"
    );
}

#[test]
fn single_thread_allocations() {
    const BYTES_PER_ITERATION: usize = 100;
    const TEST_ITERATIONS: usize = 5;

    let mut session = Session::new();

    // Test process span in single-threaded context
    let process_total = {
        let process_op = session.operation("process_single_thread");
        for i in 1..=TEST_ITERATIONS {
            let _span = process_op.measure_process();
            let _data = vec![0_u8; i * BYTES_PER_ITERATION];
            black_box(&_data);
        }
        assert_eq!(process_op.spans(), TEST_ITERATIONS as u64);
        process_op.total_bytes_allocated()
    };

    // Test thread span in single-threaded context
    let thread_total = {
        let thread_op = session.operation("thread_single_thread");
        for i in 1..=TEST_ITERATIONS {
            let _span = thread_op.measure_thread();
            let _data = vec![0_u8; i * BYTES_PER_ITERATION];
            black_box(&_data);
        }
        assert_eq!(thread_op.spans(), TEST_ITERATIONS as u64);
        thread_op.total_bytes_allocated()
    };

    // Both should have allocated some memory
    assert!(process_total > 0, "Process span should track allocations");
    assert!(thread_total > 0, "Thread span should track allocations");

    // In single-threaded context, both should track similar amounts
    #[expect(
        clippy::cast_precision_loss,
        reason = "acceptable precision loss for test comparison"
    )]
    let ratio = process_total.max(thread_total) as f64 / process_total.min(thread_total) as f64;
    assert!(
        ratio < 2.0,
        "Single-threaded allocations should be similar between process and thread spans, got process: {process_total}, thread: {thread_total}"
    );
}

#[test]
fn multithreaded_allocations_show_span_differences() {
    const NUM_WORKER_THREADS: u32 = 4;
    const ALLOCATIONS_PER_THREAD: u32 = 50;
    const MAIN_THREAD_ALLOCATIONS: u32 = 10;
    const TEST_ITERATIONS: usize = 3;

    let mut session = Session::new();

    // Helper function to spawn worker threads that allocate memory
    let spawn_workers = || {
        let handles: Vec<_> = (0..NUM_WORKER_THREADS)
            .map(|thread_id| {
                thread::spawn(move || {
                    for i in 0..ALLOCATIONS_PER_THREAD {
                        let size = ((thread_id + 1) * 100 + i) as usize;
                        let data = vec![42_u8; size];
                        black_box(data);
                    }
                })
            })
            .collect();

        // Do some allocations on the main thread
        for i in 0..MAIN_THREAD_ALLOCATIONS {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "small test values won't truncate"
            )]
            let data = vec![i as u8; 100];
            black_box(data);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().expect("thread should complete successfully");
        }
    };

    // Test process span with multithreaded work (should capture all threads)
    let process_total = {
        let process_op = session.operation("process_multithreaded");
        for _ in 0..TEST_ITERATIONS {
            let _span = process_op.measure_process();
            spawn_workers();
        }
        assert_eq!(process_op.spans(), TEST_ITERATIONS as u64);
        process_op.total_bytes_allocated()
    };

    // Test thread span with multithreaded work (should only capture main thread)
    let thread_total = {
        let thread_op = session.operation("thread_multithreaded");
        for _ in 0..TEST_ITERATIONS {
            let _span = thread_op.measure_thread();
            spawn_workers();
        }
        assert_eq!(thread_op.spans(), TEST_ITERATIONS as u64);
        thread_op.total_bytes_allocated()
    };

    // Both should have allocated some memory
    assert!(process_total > 0, "Process span should track allocations");
    assert!(thread_total > 0, "Thread span should track allocations");

    // Process span should capture significantly more than thread span
    assert!(
        process_total > thread_total * 2,
        "Process span should capture much more allocation than thread span in multithreaded context. Process: {process_total}, Thread: {thread_total}"
    );
}

#[test]
fn mixed_span_types_in_multithreaded_context() {
    const ITERATIONS: usize = 3;

    let mut session = Session::new();
    let mixed_op = session.operation("mixed_multithreaded");

    for iteration in 1..=ITERATIONS {
        // Alternate between process and thread spans
        if iteration % 2 == 0 {
            let _span = mixed_op.measure_process();
            // Spawn a thread that allocates memory
            let handle = thread::spawn(|| {
                let data = vec![0_u8; 500];
                black_box(data);
            });
            // Also allocate on main thread
            let data = vec![0_u8; 100];
            black_box(data);
            handle.join().expect("thread should complete successfully");
        } else {
            let _span = mixed_op.measure_thread();
            // Spawn a thread that allocates memory (won't be captured by thread span)
            let handle = thread::spawn(|| {
                let data = vec![0_u8; 500];
                black_box(data);
            });
            // Only main thread allocation should be captured
            let data = vec![0_u8; 100];
            black_box(data);
            handle.join().expect("thread should complete successfully");
        }
    }

    let total = mixed_op.total_bytes_allocated();
    assert_eq!(mixed_op.spans(), ITERATIONS as u64);
    assert!(total > 0, "Mixed spans should track some allocations");
}
