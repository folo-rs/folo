//! Shared fake topology, affinity observations and destructor probes for lifecycle tests.

use std::collections::BTreeSet;
use std::panic;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::FutureExt;
use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
use many_cpus::{Processor, SystemHardware};

use crate::{ErasedTaskHandle, Scheduler, VicinalTask, WorkerGroup, init_task};

/// Sparse regions with multiple eligible CPUs and a quota that excludes some of them.
pub(crate) fn hardware() -> SystemHardware {
    SystemHardware::fake(
        HardwareBuilder::new()
            .processor(ProcessorBuilder::new().id(0).memory_region(2))
            .processor(ProcessorBuilder::new().id(1).memory_region(2))
            .processor(ProcessorBuilder::new().id(3).memory_region(5))
            .max_processor_time(1.0),
    )
}

/// Records the permitted mask, rather than a CPU observed by chance.
pub(crate) fn affinity(hardware: &SystemHardware) -> BTreeSet<u32> {
    hardware
        .all_processors()
        .to_builder()
        .where_available_for_current_thread()
        .take_all()
        .unwrap()
        .iter()
        .map(Processor::id)
        .collect()
}

/// Startup-race work may legitimately execute before shutdown closes admission.
pub(crate) struct NoopTask;

impl VicinalTask for NoopTask {
    fn call(self: Pin<&mut Self>) {}
}

/// Detects abandoned captures and can re-enter the pool during destruction.
pub(crate) struct OnDrop(pub(crate) Option<Box<dyn FnOnce() + Send>>);

impl Drop for OnDrop {
    fn drop(&mut self) {
        if let Some(callback) = self.0.take() {
            callback();
        }
    }
}

impl VicinalTask for OnDrop {
    fn call(self: Pin<&mut Self>) {
        panic!("an abandoned task must not execute");
    }
}

/// Places a probe between allocation and admission without injecting production-only branches.
pub(crate) fn probe(
    state: &WorkerGroup,
    callback: impl FnOnce() + Send + 'static,
) -> ErasedTaskHandle {
    let slot = state.task_pool.lock().unwrap().alloc_uninit_box();
    init_task(slot, OnDrop(Some(Box::new(callback))))
}

/// All variants abandon synchronously after the owner stops the pool.
pub(crate) fn assert_retained_scheduler(scheduler: &Scheduler) {
    let dropped = Arc::new(AtomicUsize::new(0));
    let executed = Arc::new(AtomicUsize::new(0));
    let task = || {
        let capture = OnDrop(Some(Box::new({
            let dropped = Arc::clone(&dropped);
            move || {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        })));
        let executed = Arc::clone(&executed);
        move || {
            executed.fetch_add(1, Ordering::Relaxed);
            drop(capture);
        }
    };
    // Joined captures remain recoverable; fire-and-forget captures are released immediately.
    let ordinary = scheduler.spawn(task());
    let urgent = scheduler.spawn_urgent(task());
    scheduler.spawn_and_forget(task());
    scheduler.spawn_urgent_and_forget(task());
    assert_eq!(dropped.load(Ordering::Relaxed), 2);
    assert_eq!(executed.load(Ordering::Relaxed), 0);
    for handle in [ordinary, urgent] {
        let error = handle.now_or_never().unwrap().unwrap_err();
        assert!(error.is_cancel());
        error.into_body().unwrap()();
    }
    assert_eq!(dropped.load(Ordering::Relaxed), 4);
    assert_eq!(executed.load(Ordering::Relaxed), 2);
}

/// A larger region than the submitter set, plus a distinct sparse region.
pub(crate) fn regional_hardware() -> SystemHardware {
    let mut builder = HardwareBuilder::new().max_processor_time(1.0);
    for id in [0, 1, 2, 3, 7] {
        builder = builder.processor(ProcessorBuilder::new().id(id).memory_region(2));
    }
    builder = builder.processor(ProcessorBuilder::new().id(8).memory_region(5));
    SystemHardware::fake(builder)
}

/// Pins submitters through the same fake PAL used to inspect workers' configured masks.
pub(crate) fn pin(hardware: &SystemHardware, processor: u32) {
    hardware
        .all_processors()
        .to_builder()
        .filter(|p| p.id() == processor)
        .take_all()
        .unwrap()
        .pin_current_thread_to();
}

/// Tracks cleanup of queued work after a worker fails its affinity refresh.
pub(crate) struct CountOnDrop {
    pub(crate) dropped: Arc<AtomicUsize>,
}

impl Drop for CountOnDrop {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Release);
    }
}

/// Exercises recursive destructor failure without leaking an allocation under Miri.
///
/// Zero-sized payloads still run `Drop`, but retaining their replacement needs no heap storage.
pub(crate) struct PanickingPayload;

impl Drop for PanickingPayload {
    fn drop(&mut self) {
        // Only unwind containment is under test, not panic-hook output or backtrace collection.
        panic::resume_unwind(Box::new(Self));
    }
}
