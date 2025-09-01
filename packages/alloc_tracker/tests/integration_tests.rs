//! Integration tests for `alloc_tracker` with real memory allocations.
//!
//! These tests use a global allocator setup to test the full functionality
//! of the allocation tracking system, including single-threaded and multithreaded scenarios.

#![cfg(not(miri))] // Miri replaces the global allocator, so cannot be used here.

use std::hint::black_box;
use std::thread;

#[cfg(feature = "panic_on_next_alloc")]
use alloc_tracker::panic_on_next_alloc;
use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[test]
fn no_span_is_empty_session() {
    let session = Session::new();

    let _op = session.operation("test_no_span");

    assert!(session.is_empty());
}

#[test]
fn span_with_no_allocation_is_not_empty_session() {
    let session = Session::new();

    {
        let op = session.operation("test_no_allocation");
        drop(op.measure_process());
    } // op is dropped here, merging data to session

    assert!(
        !session.is_empty(),
        "Session should not be empty after creating a span"
    );
}

#[test]
fn single_thread_allocations() {
    const BYTES_PER_ITERATION: usize = 100;
    const TEST_ITERATIONS: usize = 5;

    let session = Session::new();

    // Test process span in single-threaded context
    let process_total = {
        let process_op = session.operation("process_single_thread");
        for i in 1..=TEST_ITERATIONS {
            let _span = process_op.measure_process();
            let _data = vec![0_u8; i * BYTES_PER_ITERATION];
            black_box(&_data);
        }
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
        thread_op.total_bytes_allocated()
    };

    // Both should have allocated some memory
    assert!(process_total > 0);
    assert!(thread_total > 0);

    assert!(process_total >= thread_total);
}

#[test]
fn multithreaded_allocations_show_span_differences() {
    const NUM_WORKER_THREADS: u32 = 4;
    const ALLOCATIONS_PER_THREAD: u32 = 50;
    const MAIN_THREAD_ALLOCATIONS: u32 = 10;
    const TEST_ITERATIONS: usize = 3;

    let session = Session::new();

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
        process_op.total_bytes_allocated()
    };

    // Test thread span with multithreaded work (should only capture main thread)
    let thread_total = {
        let thread_op = session.operation("thread_multithreaded");
        for _ in 0..TEST_ITERATIONS {
            let _span = thread_op.measure_thread();
            spawn_workers();
        }
        thread_op.total_bytes_allocated()
    };

    // Both should have allocated some memory
    assert!(process_total > 0);
    assert!(thread_total > 0);

    // Process span should capture significantly more than thread span
    assert!(
        process_total > thread_total * 2,
        "Process span should capture much more allocation than thread span in multithreaded context. Process: {process_total}, Thread: {thread_total}"
    );
}

#[test]
fn mixed_span_types_in_multithreaded_context() {
    const ITERATIONS: usize = 3;

    let session = Session::new();
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
    assert!(total > 0);
}

#[test]
fn report_is_empty_matches_session_is_empty() {
    let session = Session::new();

    // Test 1: Both empty initially
    let report = session.to_report();
    assert_eq!(session.is_empty(), report.is_empty());
    assert!(session.is_empty());
    assert!(report.is_empty());

    // Test 2: Create operation without spans - both should still be empty
    let _operation = session.operation("test");
    let report = session.to_report();
    assert_eq!(session.is_empty(), report.is_empty());
    assert!(session.is_empty());
    assert!(report.is_empty());

    // Test 3: Add spans - both should be non-empty
    {
        let operation = session.operation("test_with_spans");
        let _span = operation.measure_process();
        // No actual allocation needed for span to exist
    } // Operation is dropped here, merging data to session

    let report = session.to_report();
    assert_eq!(session.is_empty(), report.is_empty());
    assert!(!session.is_empty());
    assert!(!report.is_empty());
}

#[test]
fn report_mean_with_known_allocations() {
    const NUM_ITERATIONS: u64 = 5;

    let session = Session::new();

    {
        let operation = session.operation("known_allocation");
        for _ in 0..NUM_ITERATIONS {
            let _span = operation.measure_thread();
            // Allocate a single Box<u64> - predictable size allocation
            let boxed_value = Box::new(42_u64);
            black_box(boxed_value); // Ensure allocation is not optimized away, but Box is dropped
        }
    } // Operation is dropped here, merging data to session

    let report = session.to_report();
    let operations: Vec<_> = report.operations().collect();
    assert_eq!(operations.len(), 1);

    let (_name, op) = operations.first().unwrap();
    assert_eq!(op.total_iterations(), NUM_ITERATIONS);

    // Check that we have tracked some allocations
    let total_bytes = op.total_bytes_allocated();
    assert!(total_bytes > 0);

    // With Box<u64>, we expect at least 8 bytes per allocation (size of u64)
    // but allocators may add overhead, so we just verify basic sanity
    assert!(total_bytes >= NUM_ITERATIONS * 8);

    // The mean should be greater than 0
    let mean_bytes = op.mean();
    assert!(mean_bytes > 0);

    // Basic sanity check: mean should be total / iterations (integer division is intentional)
    #[allow(
        clippy::integer_division,
        reason = "Integer division is intended for mean calculation"
    )]
    let expected_mean = total_bytes / NUM_ITERATIONS;
    assert_eq!(mean_bytes, expected_mean);

    // Verify mean calculation makes sense - should be at least 8 bytes per allocation
    assert!(mean_bytes >= 8);
}

#[test]
#[cfg(feature = "panic_on_next_alloc")]
fn panic_on_next_alloc_can_be_controlled() {
    // This test verifies the API works but doesn't test the panic behavior
    // as that would terminate the test process

    // Default state should allow allocations
    panic_on_next_alloc(false);
    #[expect(
        clippy::useless_vec,
        reason = "we need actual allocation to test the feature"
    )]
    let _allowed_allocation = vec![1, 2, 3];

    // Enable and then immediately disable panic on next allocation
    // We don't test the actual panic as it would kill the test
    panic_on_next_alloc(true);
    panic_on_next_alloc(false);

    // Allocations should work again
    #[expect(
        clippy::useless_vec,
        reason = "we need actual allocation to test the feature"
    )]
    let _another_allowed_allocation = vec![4, 5, 6];
}

#[test]
#[cfg(feature = "panic_on_next_alloc")]
fn panic_on_next_alloc_resets_automatically() {
    use std::panic;

    // Enable panic on next allocation
    panic_on_next_alloc(true);

    // First allocation should panic
    let result = panic::catch_unwind(|| {
        #[expect(
            clippy::useless_vec,
            reason = "we need actual allocation to test the feature"
        )]
        let _vec = vec![1, 2, 3];
    });
    assert!(result.is_err());

    // Second allocation should work because flag was reset
    #[expect(
        clippy::useless_vec,
        reason = "we need actual allocation to test the feature"
    )]
    let _allowed_allocation = vec![4, 5, 6];
}

#[test]
fn process_report_includes_allocations_from_multiple_threads() {
    const THREAD_A_ALLOCS: usize = 40;
    const THREAD_B_ALLOCS: usize = 25;
    const SIZE_A: usize = 128; // bytes per allocation in thread A
    const SIZE_B: usize = 256; // bytes per allocation in thread B

    let session = Session::new();
    {
        let op = session.operation("two_thread_process");
        let _span = op.measure_process();

        let handle_a = thread::spawn(|| {
            let mut total = 0_usize;
            for _ in 0..THREAD_A_ALLOCS {
                let v = vec![0_u8; SIZE_A];
                total += v.len();
                black_box(&v);
            }
            total
        });

        let handle_b = thread::spawn(|| {
            let mut total = 0_usize;
            for _ in 0..THREAD_B_ALLOCS {
                let v = vec![1_u8; SIZE_B];
                black_box(&v);
                total += v.len();
            }
            total
        });

        // Also allocate on the main thread so we can distinguish process span > sum of one thread.
        let main_alloc = vec![2_u8; 64];
        black_box(&main_alloc);

        let a_bytes = handle_a.join().expect("thread A joined");
        let b_bytes = handle_b.join().expect("thread B joined");

        // Basic sanity: ensure we actually performed the expected sizes.
        assert_eq!(a_bytes, THREAD_A_ALLOCS * SIZE_A);
        assert_eq!(b_bytes, THREAD_B_ALLOCS * SIZE_B);
    }

    let report = session.to_report();
    let operations: Vec<_> = report.operations().collect();
    assert_eq!(
        operations.len(),
        1,
        "expected exactly one operation in report"
    );
    let (_name, op) = operations.first().unwrap();
    let total = op.total_bytes_allocated();

    // Expect at least the sum of the two thread totals (plus main thread allocation overhead)
    let min_expected = (THREAD_A_ALLOCS * SIZE_A + THREAD_B_ALLOCS * SIZE_B) as u64;
    assert!(
        total >= min_expected,
        "total {total} < expected minimum {min_expected}"
    );

    // Ensure neither thread's contribution is trivially missing: total should exceed each individual component
    assert!(total >= (THREAD_A_ALLOCS * SIZE_A) as u64);
    assert!(total >= (THREAD_B_ALLOCS * SIZE_B) as u64);
}
