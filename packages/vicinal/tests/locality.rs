#![expect(
    missing_docs,
    reason = "This is a test file, documentation is not required."
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use many_cpus::SystemHardware;
use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
use testing::with_watchdog;
use vicinal::Pool;

#[test]
fn tasks_execute_on_spawning_processor() {
    let pool = Pool::new();
    let scheduler = pool.scheduler();

    // We will spawn tasks from different processors and verify they execute on the same processor.
    let processors = SystemHardware::current().processors();

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
fn tasks_execute_on_spawning_processor_with_fake_hardware() {
    with_watchdog(|| {
        let hardware = SystemHardware::fake(
            HardwareBuilder::new()
                .processor(ProcessorBuilder::new().id(0).memory_region(0))
                .processor(ProcessorBuilder::new().id(1).memory_region(0))
                .processor(ProcessorBuilder::new().id(2).memory_region(1)),
        );

        let pool = Pool::builder().hardware(hardware.clone()).build();
        let scheduler = pool.scheduler();

        let processors = hardware.processors();

        // Counter for tasks that executed on their expected processor.
        let correct_processor_count = Arc::new(AtomicUsize::new(0));
        let total_tasks = Arc::new(AtomicUsize::new(0));

        // Spawn threads on different processors, each spawning tasks into the pool.
        let handles = processors.spawn_threads({
            let correct_processor_count = Arc::clone(&correct_processor_count);
            let total_tasks = Arc::clone(&total_tasks);

            move |processor| {
                let spawning_processor_id = processor.id();

                let mut task_handles = Vec::new();
                for _ in 0..5 {
                    let correct_count = Arc::clone(&correct_processor_count);
                    let total = Arc::clone(&total_tasks);
                    let hw = hardware.clone();

                    let handle = scheduler.spawn(move || {
                        let executing_processor_id = hw.current_processor_id();

                        total.fetch_add(1, Ordering::Relaxed);

                        if executing_processor_id == spawning_processor_id {
                            correct_count.fetch_add(1, Ordering::Relaxed);
                        }

                        (spawning_processor_id, executing_processor_id)
                    });
                    task_handles.push(handle);
                }

                for handle in task_handles {
                    futures::executor::block_on(handle);
                }
            }
        });

        for handle in handles {
            handle.join().unwrap();
        }

        let correct = correct_processor_count.load(Ordering::Relaxed);
        let total = total_tasks.load(Ordering::Relaxed);

        assert_eq!(
            correct, total,
            "All tasks should execute on the processor they were spawned from"
        );
    });
}
