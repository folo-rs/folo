#![expect(
    missing_docs,
    reason = "This is a test file, documentation is not required."
)]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use many_cpus::{Processor, SystemHardware};
use new_zealand::nz;
use testing::with_watchdog;
use vicinal::{Pool, WorkerPolicy};

testing::init_tracing!();

#[test]
fn tasks_execute_on_spawning_processor() {
    // Repeated submissions exercise an established destination as well as its first startup.
    const TASKS_PER_PROCESSOR: usize = 5;

    with_watchdog(|| {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let processors = SystemHardware::current().all_processors();

        let handles = processors.spawn_threads(move |processor| {
            let spawning_processor_id = processor.id();

            let mut task_handles = Vec::new();
            for _ in 0..TASKS_PER_PROCESSOR {
                let handle = scheduler.spawn(|| SystemHardware::current().current_processor_id());
                task_handles.push(handle);
            }

            for handle in task_handles {
                assert_eq!(block_on(handle).unwrap(), spawning_processor_id);
            }
        });

        for handle in handles {
            handle.join().unwrap();
        }
    });
}

#[test]
fn regional_tasks_use_only_submitting_processors() {
    assert_submitting_processor_affinity(WorkerPolicy::per_memory_region(nz!(2_u32)), true);
}

#[test]
fn system_tasks_use_only_submitting_processors() {
    assert_submitting_processor_affinity(WorkerPolicy::per_system(nz!(2_u32)), false);
}

/// Eager initializers observe the exact selected regional mask through the real OS adapter.
#[test]
fn eager_regional_initializers_have_selected_affinity() {
    assert_eager_affinity(WorkerPolicy::per_memory_region(nz!(1)), true);
}

/// System startup applies its full selection before invoking any user initialization.
#[test]
fn eager_system_initializers_have_selected_affinity() {
    assert_eager_affinity(WorkerPolicy::per_system(nz!(1)), false);
}

/// Bounded native coverage complements simulated sparse/multi-region unit tests.
fn assert_eager_affinity(policy: WorkerPolicy, regional: bool) {
    with_watchdog(move || {
        let hardware = SystemHardware::current();
        // A small selection tests multi-CPU affinity without creating machine-sized pools.
        let processors = hardware
            .processors()
            .take(nz!(2))
            .unwrap_or_else(|| hardware.processors());
        let expected: BTreeSet<_> = processors.iter().map(Processor::id).collect();
        let regions: BTreeSet<_> = processors.iter().map(Processor::memory_region_id).collect();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let pool = Pool::builder()
            .worker_policy(policy)
            .eager(processors.clone())
            .thread_init({
                let observations = Arc::clone(&observations);
                move || {
                    let permitted = hardware
                        .all_processors()
                        .to_builder()
                        .where_available_for_current_thread()
                        .take_all()
                        .unwrap();
                    observations.lock().unwrap().push((
                        hardware.current_memory_region_id(),
                        permitted.iter().map(Processor::id).collect::<BTreeSet<_>>(),
                    ));
                }
            })
            .build();
        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), if regional { regions.len() } else { 1 });
        for (region, actual) in observations.iter() {
            let expected = if regional {
                processors
                    .iter()
                    .filter(|processor| processor.memory_region_id() == *region)
                    .map(Processor::id)
                    .collect()
            } else {
                expected.clone()
            };
            assert_eq!(*actual, expected);
        }
        drop(pool);
    });
}

/// Verifies actual OS affinity, including cross-region masks on multi-region machines.
fn assert_submitting_processor_affinity(policy: WorkerPolicy, regional: bool) {
    with_watchdog(move || {
        let hardware = SystemHardware::current();
        let processors = hardware.all_processors();
        let pool = Pool::builder().worker_policy(policy).build();
        let scheduler = pool.scheduler();

        // Sequential CPU registration makes each expected cumulative mask deterministic.
        let mut submitted = BTreeSet::new();

        for processor in &processors {
            let caller = processors
                .to_builder()
                .filter(|p| p.id() == processor.id())
                .take_all()
                .unwrap();
            submitted.insert(processor.id());

            let expected_region = processor.memory_region_id();
            let expected: BTreeSet<_> = processors
                .iter()
                .filter(|p| {
                    (!regional || p.memory_region_id() == expected_region)
                        && submitted.contains(&p.id())
                })
                .map(Processor::id)
                .collect();

            caller
                .spawn_thread({
                    let scheduler = scheduler.clone();
                    move |_| {
                        block_on(scheduler.spawn(move || {
                            let hardware = SystemHardware::current();
                            if regional {
                                assert_eq!(hardware.current_memory_region_id(), expected_region);
                            }

                            let affinity = hardware
                                .all_processors()
                                .to_builder()
                                .where_available_for_current_thread()
                                .take_all()
                                .unwrap();
                            assert_eq!(
                                affinity.iter().map(Processor::id).collect::<BTreeSet<_>>(),
                                expected
                            );
                        }))
                        .unwrap();
                    }
                })
                .join()
                .unwrap();
        }
    });
}

::testing::set_allocator!();
