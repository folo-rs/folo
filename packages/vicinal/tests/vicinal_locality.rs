#![expect(
    missing_docs,
    reason = "This is a test file, documentation is not required."
)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use many_cpus::{Processor, SystemHardware};
use new_zealand::nz;
use vicinal::{Pool, WorkerPolicy};

#[test]
fn tasks_execute_on_spawning_processor() {
    let pool = Pool::new();
    let scheduler = pool.scheduler();

    // We will spawn tasks from different processors and verify they execute on the same processor.
    let processors = SystemHardware::current().all_processors();

    // Counter for tasks that executed on their expected processor.
    let correct_processor_count = Arc::new(AtomicUsize::new(0));
    let total_tasks = Arc::new(AtomicUsize::new(0));

    // Spawn threads on different processors, each spawning tasks into the pool.
    let handles = processors.spawn_threads({
        let correct_processor_count = Arc::clone(&correct_processor_count);
        let total_tasks = Arc::clone(&total_tasks);

        move |processor| {
            let spawning_processor_id = processor.id();

            // Spawn several tasks from this processor.
            let mut task_handles = Vec::new();
            for _ in 0..5 {
                let correct_count = Arc::clone(&correct_processor_count);
                let total = Arc::clone(&total_tasks);

                let handle = scheduler.spawn(move || {
                    // Check which processor we are executing on.
                    let executing_processor_id = SystemHardware::current().current_processor_id();

                    total.fetch_add(1, Ordering::Relaxed);

                    if executing_processor_id == spawning_processor_id {
                        correct_count.fetch_add(1, Ordering::Relaxed);
                    }

                    (spawning_processor_id, executing_processor_id)
                });
                task_handles.push(handle);
            }

            // Wait for all tasks to complete.
            for handle in task_handles {
                // We use block_on from futures crate for simplicity in this example.
                futures::executor::block_on(handle);
            }
        }
    });

    // Wait for all spawning threads to complete.
    for handle in handles {
        handle.join().unwrap();
    }

    let correct = correct_processor_count.load(Ordering::Relaxed);
    let total = total_tasks.load(Ordering::Relaxed);

    assert_eq!(
        correct, total,
        "All tasks should execute on the processor they were spawned from"
    );
}

#[test]
fn regional_tasks_preserve_region_and_full_eligible_affinity() {
    testing::with_watchdog(|| {
        let hardware = SystemHardware::current();
        let processors = hardware.all_processors();
        let pool = Pool::builder()
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(2_u32)))
            .build();
        let scheduler = pool.scheduler();
        let threads = processors.spawn_threads(move |processor| {
            let expected_region = processor.memory_region_id();
            let handle = scheduler.spawn(move || {
                let hardware = SystemHardware::current();
                assert_eq!(hardware.current_memory_region_id(), expected_region);
                let affinity = hardware.thread_processors().unwrap();
                let expected = hardware
                    .all_processors()
                    .to_builder()
                    .filter(|p| p.memory_region_id() == expected_region)
                    .take_all()
                    .unwrap();
                assert_eq!(
                    affinity.iter().map(Processor::id).collect::<BTreeSet<_>>(),
                    expected.iter().map(Processor::id).collect::<BTreeSet<_>>()
                );
            });
            futures::executor::block_on(handle);
        });
        for thread in threads {
            thread.join().unwrap();
        }
    });
}

::testing::set_allocator!();
