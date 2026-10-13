//! Submission, execution and shutdown diagnostics for each locality policy.

use std::any::Any;
use std::panic;
use std::sync::mpsc;

use futures::executor::block_on;
use new_zealand::nz;
use testing::TracingCapture;
use vicinal::WorkerPolicy;

use crate::fixtures::{PanickingPayload, pool};

/// Exercises both destination formatters, priorities and task-panic payload representations.
pub(crate) fn exercise(capture: &TracingCapture) {
    // One worker gives each started task exclusive execution until its logging completes.
    // Empty and multiline names also exercise diagnostic field escaping.
    let borrowed: Box<dyn Any + Send> = Box::new("borrowed panic canary");
    for (policy, name, group, payloads, markers) in [
        (
            WorkerPolicy::fixed_per_processor(nz!(1)),
            "",
            "Processor",
            [borrowed, Box::new(String::from("owned panic canary"))],
            &["borrowed panic canary", "owned panic canary"][..],
        ),
        (
            WorkerPolicy::fixed_per_memory_region(nz!(1)),
            "logging\ncanary",
            "MemoryRegion",
            [Box::new(()), Box::new(PanickingPayload)],
            &[][..],
        ),
    ] {
        let first_entry = capture.snapshot().len();
        let pool = pool(policy, name);
        let scheduler = pool.scheduler();
        assert_eq!(block_on(scheduler.spawn(|| "regular")), "regular");
        assert_eq!(block_on(scheduler.spawn_urgent(|| "urgent")), "urgent");

        let expected_panics = payloads.len();
        for (urgent, payload) in [false, true].into_iter().zip(payloads) {
            let (started, receiver) = mpsc::channel();
            let task = move || {
                started.send(()).unwrap();
                panic::resume_unwind(payload);
            };
            if urgent {
                scheduler.spawn_urgent_and_forget(task);
            } else {
                scheduler.spawn_and_forget(task);
            }
            // A task that has started cannot be abandoned by the subsequent pool drop.
            receiver.recv().unwrap();
        }

        // Result delivery and the start signals precede the worker's final log statements.
        drop(pool);
        // The abandonment event is synchronous; awaiting the disconnected handle would
        // only repeat the separate scheduler panic-contract tests.
        drop(scheduler.spawn(|| {}));

        let snapshot = capture.snapshot();
        let entries = snapshot.get(first_entry..).unwrap();
        for message in [
            "worker thread started",
            "worker thread exiting",
            "executed task",
        ] {
            assert!(entries.iter().any(|entry| {
                entry.contains(message)
                    && entry.contains(&format!("group={group}"))
                    && entry.contains("worker_index=")
                    && entry.contains("pool_id=")
            }));
        }
        for urgent in [false, true] {
            assert!(entries.iter().any(|entry| {
                entry.contains("spawned task") && entry.contains(&format!("urgent={urgent}"))
            }));
        }
        assert!(entries.iter().any(|entry| entry.contains("abandoned task")));
        for marker in markers {
            assert!(
                entries
                    .iter()
                    .any(|entry| { entry.contains("fire-and-forget") && entry.contains(marker) })
            );
        }
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.contains("fire-and-forget"))
                .count(),
            expected_panics
        );
    }
    assert!(
        capture
            .snapshot()
            .iter()
            .any(|entry| entry.contains("payload destruction failed"))
    );
}
