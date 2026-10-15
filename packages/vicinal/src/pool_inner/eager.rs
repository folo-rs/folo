//! Construction-time selection, affinity seeding and full worker readiness.

use std::collections::BTreeSet;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::{self, JoinHandle as ThreadJoinHandle};

use many_cpus::{Processor, ProcessorSet, SystemHardware};

use crate::pool_inner::WorkerEntry;
use crate::{GroupId, NEVER_POISONED, PolicyKind, PoolInner};

impl PoolInner {
    /// Starts the selected destinations without synthetic submissions or task-storage allocation.
    pub(crate) fn start_eager(self: &Arc<Self>, processors: ProcessorSet) {
        self.start_eager_with(
            processors,
            thread::Builder::spawn,
            ProcessorSet::pin_current_thread_to,
        );
    }

    /// Keeps failure injection on the same construction path as real spawning and pinning.
    // Startup and completion mutations can strand initializing workers.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn start_eager_with(
        self: &Arc<Self>,
        processors: ProcessorSet,
        mut spawn: impl FnMut(thread::Builder, WorkerEntry) -> io::Result<ThreadJoinHandle<()>>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
    ) {
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            let groups = eager_groups(&self.hardware, self.policy.kind(), processors);
            let mut initialized = Vec::new();
            let mut handles = self.worker_handles.lock().expect(NEVER_POISONED);
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                // Publish complete masks before any initializer runs. The pool is not exposed
                // yet, so there is no task admission to coordinate with these initial members.
                // Ref: docs/implementation.md, "Eager construction".
                for (destination, affinity) in &groups {
                    let state = self.registry.get_or_init(*destination);
                    if self.policy.shares_processors() {
                        *state.affinity.lock().expect(NEVER_POISONED) = Some(Arc::clone(affinity));
                        // Release publishes the snapshot; membership readers acquire these flags.
                        state.affinity_revision.fetch_add(1, Ordering::Release);
                        for processor in affinity.iter() {
                            self.membership(processor.id())
                                .store(true, Ordering::Release);
                        }
                    }
                }
                for (destination, affinity) in groups {
                    let state = self.registry.get_or_init(destination);
                    Self::start_group(state, || {
                        self.start_workers_on(
                            destination,
                            &affinity,
                            &mut handles,
                            &mut spawn,
                            pin.clone(),
                            |receiver| initialized.push(receiver),
                        );
                    });
                }
            }));
            if result.is_err() {
                self.stop_admission();
            }
            drop(handles);
            result.unwrap_or_else(|payload| panic::resume_unwind(payload));

            // All workers have started, and no registration lock is held while user callbacks
            // finish. Receiving their acknowledgments makes initialization effects visible.
            for receiver in initialized {
                match receiver.recv() {
                    Ok(Ok(())) => {}
                    Ok(Err(payload)) => panic::resume_unwind(payload),
                    Err(error) => {
                        // Join retrieves the original initializer panic rather than replacing
                        // it with a channel error. Unexpected early exits remain explicit failures.
                        self.join_all_workers();
                        panic!("worker exited before initialization completed: {error}");
                    }
                }
            }
            // A worker may have failed after acknowledging initialization. Acquire observes the
            // failure boundary's terminal publication before returning the constructed pool.
            if self.poisoned.load(Ordering::Acquire) {
                self.join_all_workers();
                panic!("worker pool failed during eager initialization");
            }
        }));
        if let Err(payload) = result {
            self.poison(payload);
        }
    }
}

/// Validates hardware provenance before partitioning the exact, quota-independent selection.
fn eager_groups(
    hardware: &SystemHardware,
    policy: PolicyKind,
    processors: ProcessorSet,
) -> Vec<(GroupId, Arc<ProcessorSet>)> {
    let eligible: BTreeSet<_> = hardware.all_processors().into_iter().collect();
    // take_exact can combine handles with a different backing hardware used for pinning.
    // Rebuilding resolves processor identities from that backing hardware rather than the handles.
    let backing_processors = processors
        .to_builder()
        .take_all()
        .expect("a nonempty selection retains candidates on its backing hardware");
    // Processor equality includes its hardware instance: equal numeric IDs are not sufficient.
    assert!(
        processors
            .iter()
            .chain(backing_processors.iter())
            .all(|processor| eligible.contains(processor))
    );

    match policy {
        PolicyKind::Processor => {
            // Repeated processor entries still denote one destination and one startup outcome.
            let mut destinations = BTreeSet::new();
            processors
                .decompose()
                .into_iter()
                .filter(|set| destinations.insert(set.processors().first().id()))
                .map(|set| {
                    (
                        GroupId::Processor(set.processors().first().id()),
                        Arc::new(set),
                    )
                })
                .collect()
        }
        PolicyKind::MemoryRegion => processors
            .iter()
            .map(Processor::memory_region_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|region| {
                let affinity = processors
                    .filter(|processor| processor.memory_region_id() == region)
                    .expect("each selected region contains a selected processor");
                (GroupId::MemoryRegion(region), Arc::new(affinity))
            })
            .collect(),
        PolicyKind::System => vec![(GroupId::System, Arc::new(processors))],
    }
}
