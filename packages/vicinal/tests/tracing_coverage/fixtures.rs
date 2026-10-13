//! Deterministic public-API fixtures shared by this integration binary.

use std::panic;

use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use vicinal::{Pool, WorkerPolicy};

/// Provides deterministic simulated destinations for locality scenarios without real OS affinity.
pub(crate) fn pool(policy: WorkerPolicy, name: &str) -> Pool {
    let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    Pool::builder()
        .hardware(hardware)
        .worker_policy(policy)
        .name(name)
        .build()
}

/// Fails disposal of a user-owned result or capture, outside the task's panic boundary.
pub(crate) struct PanicOnDrop(pub(crate) &'static str);

impl Drop for PanicOnDrop {
    fn drop(&mut self) {
        // Resume directly: these tests exercise containment, not the panic hook.
        panic::resume_unwind(Box::new(self.0));
    }
}

/// Exercises recursive payload destruction without retaining an allocation under Miri.
pub(crate) struct PanickingPayload;

impl Drop for PanickingPayload {
    fn drop(&mut self) {
        // The replacement is zero-sized, so the pool's deliberate retention leaks no storage.
        panic::resume_unwind(Box::new(Self));
    }
}

/// Releases a running worker when shutdown abandons a queued capture.
pub(crate) struct OnDrop<F: FnOnce()>(pub(crate) Option<F>);

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(action) = self.0.take() {
            action();
        }
    }
}
