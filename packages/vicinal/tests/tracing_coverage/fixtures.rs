//! Deterministic public-API fixtures shared by this integration binary.

use std::panic;

use many_cpus::SystemHardware;
use many_cpus::fake::HardwareBuilder;
use new_zealand::nz;
use vicinal::Pool;
use vicinal::worker_policy::WorkerPolicy;

/// Provides deterministic simulated destinations for locality scenarios without real OS affinity.
pub(crate) fn pool(policy: impl WorkerPolicy, name: &str) -> Pool {
    let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
    Pool::builder()
        .hardware(hardware)
        .worker_policy(policy)
        .name(name)
        .build()
        .unwrap()
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
        // Reusing a zero-sized replacement keeps recursive destructor failure deterministic.
        panic::resume_unwind(Box::new(Self));
    }
}
