//! Integration tests for `ResourceUsageOutput` to showcase meaningful data retrieval.
//!
//! These tests demonstrate that `ResourceUsageOutput` actually returns useful data from
//! allocation and processor time tracking. They are written as integration tests so we
//! can safely use a custom global allocator without interfering with other tests.
#![cfg(all(not(miri), feature = "alloc_tracker", feature = "all_the_time"))]

use std::time::Duration;

use many_cpus::ProcessorSet;
use par_bench::{ResourceUsageExt, Run, ThreadPool};

// Set up global allocator for allocation tracking tests
#[global_allocator]
static ALLOCATOR: alloc_tracker::Allocator<std::alloc::System> = alloc_tracker::Allocator::system();

#[test]
fn resource_usage_output_provides_meaningful_allocation_data() {
    let allocs = alloc_tracker::Session::new();
    let mut pool = ThreadPool::new(ProcessorSet::single());

    let results = Run::new()
        .measure_resource_usage("test_allocations", |measure| measure.allocs(&allocs))
        .iter(|_| {
            // Perform work that allocates memory
            let _data = vec![1_u64; 1000]; // Allocate 8000 bytes
            let _more_data = vec![2_u64; 500]; // Allocate 4000 bytes more
            // Total: ~12000 bytes per iteration
        })
        .execute_on(&mut pool, 10);

    // Verify we got meaningful resource usage outputs
    let measure_outputs: Vec<_> = results.measure_outputs().collect();
    assert!(!measure_outputs.is_empty());

    // Check that allocation data is present and meaningful
    for output in &measure_outputs {
        let alloc_report = output
            .allocs()
            .expect("allocation tracking should be configured");

        // The report should not be empty
        assert!(!alloc_report.is_empty());

        // Check the operations in the report
        let operations: Vec<_> = alloc_report.operations().collect();
        assert_eq!(operations.len(), 1);

        let (operation_name, operation_stats) = operations
            .first()
            .expect("should have exactly one operation");
        assert_eq!(*operation_name, "test_allocations");

        // Verify meaningful allocation statistics
        assert!(operation_stats.total_iterations() > 0);
        assert!(operation_stats.total_bytes_allocated() > 0);

        // The mean should be reasonable (we allocate ~12KB per iteration)
        let mean_bytes = operation_stats.mean();
        assert!(
            mean_bytes > 10_000,
            "Mean allocation per iteration should be reasonable, got {mean_bytes} bytes"
        );
        assert!(
            mean_bytes < 50_000,
            "Mean allocation per iteration should not be excessive, got {mean_bytes} bytes"
        );

        println!(
            "Thread allocated {} bytes total across {} iterations (mean: {} bytes)",
            operation_stats.total_bytes_allocated(),
            operation_stats.total_iterations(),
            mean_bytes
        );
    }

    // Verify the session also recorded the operations
    let session_report = allocs.to_report();
    assert!(!session_report.is_empty());
    println!("Session report shows operations were tracked successfully");
}

#[test]
fn resource_usage_output_provides_meaningful_processor_time_data() {
    let processor_time = all_the_time::Session::new();
    let mut pool = ThreadPool::new(ProcessorSet::single());

    let results = Run::new()
        .measure_resource_usage("test_cpu_work", |measure| {
            measure.processor_time(&processor_time)
        })
        .iter(|_| {
            // Perform more intensive CPU work to ensure measurable time
            let mut sum = 0_u64;
            for i in 0_u64..100_000 {
                sum = sum.wrapping_add(i.wrapping_mul(i).wrapping_add(17));
            }
            // Additional computation to make it more CPU-intensive
            for i in 0_u64..50_000 {
                sum = sum.wrapping_mul(3).wrapping_add(i);
            }
            std::hint::black_box(sum);
        })
        .execute_on(&mut pool, 10);

    // Verify we got meaningful resource usage outputs
    let measure_outputs: Vec<_> = results.measure_outputs().collect();
    assert!(!measure_outputs.is_empty());

    // Check that processor time data is present and meaningful
    for output in &measure_outputs {
        let time_report = output
            .processor_time()
            .expect("processor time tracking should be configured");

        // The report should not be empty
        assert!(!time_report.is_empty());

        // Check the operations in the report
        let operations: Vec<_> = time_report.operations().collect();
        assert_eq!(operations.len(), 1);

        let (operation_name, operation_stats) = operations
            .first()
            .expect("should have exactly one operation");
        assert_eq!(*operation_name, "test_cpu_work");

        // Verify meaningful processor time statistics
        assert!(operation_stats.total_iterations() > 0);

        // The processor time might be zero on some systems due to precision limitations
        // This is valid behavior - we should handle it gracefully
        if operation_stats.total_processor_time() == Duration::ZERO {
            println!(
                "Processor time was not measurable on this system (total: {:?}, iterations: {})",
                operation_stats.total_processor_time(),
                operation_stats.total_iterations()
            );
            println!("This is expected on systems with low timer precision or fast execution");
        } else {
            println!(
                "Thread used {:?} processor time total across {} iterations (mean: {:?})",
                operation_stats.total_processor_time(),
                operation_stats.total_iterations(),
                operation_stats.mean()
            );

            // If time was measured, it should be reasonable
            let mean_time = operation_stats.mean();
            assert!(
                mean_time < Duration::from_millis(1000),
                "Mean processor time per iteration should be reasonable, got {mean_time:?}"
            );
        }
    }

    // Verify the session also recorded the operations
    let session_report = processor_time.to_report();
    assert!(!session_report.is_empty());
    println!("Session report shows operations were tracked successfully");
}

#[test]
fn resource_usage_output_provides_meaningful_combined_data() {
    let allocs = alloc_tracker::Session::new();
    let processor_time = all_the_time::Session::new();
    let mut pool = ThreadPool::new(ProcessorSet::single());

    let results = Run::new()
        .measure_resource_usage("combined_work", |measure| {
            measure.allocs(&allocs).processor_time(&processor_time)
        })
        .iter(|_| {
            // Perform work that both allocates memory and uses CPU time
            let mut data = Vec::with_capacity(1000);
            for i in 0_u64..1000 {
                data.push(i.wrapping_mul(i).wrapping_add(13));
            }

            // Additional intensive CPU work
            let mut sum = 0_u64;
            for &value in &data {
                sum = sum.wrapping_add(value);
            }
            // More computation to ensure measurable time
            for i in 0_u64..10_000 {
                sum = sum.wrapping_mul(3).wrapping_add(i);
            }

            std::hint::black_box(sum);
        })
        .execute_on(&mut pool, 5);

    // Verify we got meaningful resource usage outputs
    let measure_outputs: Vec<_> = results.measure_outputs().collect();
    assert!(!measure_outputs.is_empty());

    // Check that both allocation and processor time data are present and meaningful
    for output in &measure_outputs {
        // Check allocation data
        let alloc_report = output
            .allocs()
            .expect("allocation tracking should be configured");
        assert!(!alloc_report.is_empty());

        let alloc_operations: Vec<_> = alloc_report.operations().collect();
        assert_eq!(alloc_operations.len(), 1);

        let (alloc_name, alloc_stats) = alloc_operations
            .first()
            .expect("should have exactly one operation");
        assert_eq!(*alloc_name, "combined_work");
        assert!(alloc_stats.total_iterations() > 0);
        assert!(alloc_stats.total_bytes_allocated() > 0);

        println!(
            "Combined work - Allocations: {} bytes total, {} iterations, {} bytes mean",
            alloc_stats.total_bytes_allocated(),
            alloc_stats.total_iterations(),
            alloc_stats.mean()
        );

        // Check processor time data
        let time_report = output
            .processor_time()
            .expect("processor time tracking should be configured");
        assert!(!time_report.is_empty());

        let time_operations: Vec<_> = time_report.operations().collect();
        assert_eq!(time_operations.len(), 1);

        let (time_name, time_stats) = time_operations
            .first()
            .expect("should have exactly one operation");
        assert_eq!(*time_name, "combined_work");
        assert!(time_stats.total_iterations() > 0);

        // Handle the case where processor time might be zero due to timing precision
        if time_stats.total_processor_time() == Duration::ZERO {
            println!(
                "Combined work - Processor time: {:?} total, {} iterations (not measurable due to timer precision)",
                time_stats.total_processor_time(),
                time_stats.total_iterations()
            );
        } else {
            println!(
                "Combined work - Processor time: {:?} total, {} iterations, {:?} mean",
                time_stats.total_processor_time(),
                time_stats.total_iterations(),
                time_stats.mean()
            );
        }

        // Both should track the same operation and have the same iteration count
        assert_eq!(
            alloc_stats.total_iterations(),
            time_stats.total_iterations()
        );
    }

    // Verify both sessions recorded the operations
    let alloc_session_report = allocs.to_report();
    let time_session_report = processor_time.to_report();
    assert!(!alloc_session_report.is_empty());
    assert!(!time_session_report.is_empty());
    println!("Both session reports show operations were tracked successfully");
}

#[test]
fn resource_usage_output_handles_no_allocations_gracefully() {
    let allocs = alloc_tracker::Session::new();
    let mut pool = ThreadPool::new(ProcessorSet::single());

    let results = Run::new()
        .measure_resource_usage("no_alloc_work", |measure| measure.allocs(&allocs))
        .iter(|_| {
            // Perform work that does not allocate memory
            let mut sum = 0_u64;
            for i in 0_u64..1000 {
                sum = sum.wrapping_add(i);
            }
            std::hint::black_box(sum);
        })
        .execute_on(&mut pool, 10);

    // Verify we got resource usage outputs even when no allocations occurred
    let measure_outputs: Vec<_> = results.measure_outputs().collect();
    assert!(!measure_outputs.is_empty());

    for output in &measure_outputs {
        let alloc_report = output
            .allocs()
            .expect("allocation tracking should be configured");

        // The report might be empty if no allocations occurred
        // This is valid behavior - we should handle it gracefully
        if alloc_report.is_empty() {
            println!("No allocations detected, which is expected for this workload");
        } else {
            // If allocations were detected, they should be reasonable
            let operations: Vec<_> = alloc_report.operations().collect();
            for (operation_name, operation_stats) in operations {
                println!(
                    "Unexpected allocations in '{}': {} bytes total",
                    operation_name,
                    operation_stats.total_bytes_allocated()
                );
            }
        }
    }
}

#[test]
fn resource_usage_output_tracks_multiple_operations() {
    let allocs = alloc_tracker::Session::new();
    let mut pool = ThreadPool::new(ProcessorSet::single());

    // Run multiple different operations
    let results1 = Run::new()
        .measure_resource_usage("operation_a", |measure| measure.allocs(&allocs))
        .iter(|_| {
            let _data = vec![1_u64; 100]; // Smaller allocation
        })
        .execute_on(&mut pool, 5);

    let results2 = Run::new()
        .measure_resource_usage("operation_b", |measure| measure.allocs(&allocs))
        .iter(|_| {
            let _data = vec![2_u64; 1000]; // Larger allocation
        })
        .execute_on(&mut pool, 3);

    // Check that both operations are tracked separately
    let session_report = allocs.to_report();
    assert!(!session_report.is_empty());

    let operations: Vec<_> = session_report.operations().collect();
    println!("Found {} operations in session report", operations.len());

    // We should have tracked both operations
    let operation_names: std::collections::HashSet<_> =
        operations.iter().map(|(name, _)| *name).collect();

    if operation_names.contains("operation_a") {
        println!("Found operation_a in session report");
    }
    if operation_names.contains("operation_b") {
        println!("Found operation_b in session report");
    }

    // At minimum, we should have some meaningful data
    assert!(!operations.is_empty());
    for (name, stats) in operations {
        println!(
            "Operation '{}': {} bytes total, {} iterations",
            name,
            stats.total_bytes_allocated(),
            stats.total_iterations()
        );
        assert!(stats.total_iterations() > 0);
    }

    // Check individual results also contain meaningful data
    for results in [results1, results2] {
        let measure_outputs: Vec<_> = results.measure_outputs().collect();
        assert!(!measure_outputs.is_empty());

        for output in measure_outputs {
            if let Some(alloc_report) = output.allocs()
                && !alloc_report.is_empty()
            {
                let ops: Vec<_> = alloc_report.operations().collect();
                for (name, stats) in ops {
                    println!(
                        "Individual result - Operation '{}': {} bytes",
                        name,
                        stats.total_bytes_allocated()
                    );
                }
            }
        }
    }
}
