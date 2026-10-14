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
///
/// Tracing targets must follow their owning component so internal helpers share its log filters.
pub(crate) fn exercise(capture: &TracingCapture) {
    // The worker configuration prevents a later task's start signal from overtaking
    // an earlier task's remaining diagnostics.
    // Empty and multiline names also exercise diagnostic field escaping.
    let borrowed: Box<dyn Any + Send> = Box::new("borrowed panic canary");
    for (policy, name, group, payloads, markers) in [
        (
            WorkerPolicy::per_processor(nz!(1)),
            "",
            "Processor",
            [borrowed, Box::new(String::from("owned panic canary"))],
            &["borrowed panic canary", "owned panic canary"][..],
        ),
        (
            WorkerPolicy::per_memory_region(nz!(1)),
            "logging\ncanary",
            "MemoryRegion",
            [Box::new(()), Box::new(PanickingPayload)],
            &[][..],
        ),
    ] {
        let first_entry = capture.snapshot().len();
        let pool = pool(policy, name);
        let scheduler = pool.scheduler();
        assert_eq!(block_on(scheduler.spawn(|| "regular")).unwrap(), "regular");
        assert_eq!(
            block_on(scheduler.spawn_urgent(|| "urgent")).unwrap(),
            "urgent"
        );

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
        // Submission after shutdown emits the abandonment event synchronously.
        drop(scheduler.spawn(|| {}));

        let snapshot = capture.snapshot();
        let entries = snapshot.get(first_entry..).unwrap();
        let group_field = format!("group={group}");
        for message in [
            "worker thread started",
            "worker thread exiting",
            "executed task",
        ] {
            assert!(entries.iter().any(|entry| {
                entry.contains(message)
                    && entry.contains("vicinal::pool_inner::inner:")
                    && entry.contains(&group_field)
                    && entry.contains("worker_index=")
                    && entry.contains("pool_id=")
            }));
        }
        for urgent in [false, true] {
            let urgent_field = format!("urgent={urgent}");
            assert!(entries.iter().any(|entry| {
                entry.contains("spawned task")
                    && entry.contains("vicinal::pool_inner::inner:")
                    && entry.contains(&urgent_field)
            }));
        }

        assert!(entries.iter().any(|entry| {
            entry.contains("abandoned task") && entry.contains("vicinal::pool_inner::inner:")
        }));
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
    assert!(capture.snapshot().iter().any(|entry| {
        entry.contains("payload destruction failed") && entry.contains("vicinal::task:")
    }));
}
