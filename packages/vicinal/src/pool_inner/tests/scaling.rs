//! Scaling-specific failure cleanup at the in-process worker adapter boundary.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::FutureExt;
use many_cpus::ProcessorSet;
use new_zealand::nz;
use testing::{assert_panics, with_watchdog};

use super::helpers::{hardware, pin};
use crate::{Pool, ScalingPolicy, ScalingState, WorkerPolicy};

struct TogglePolicy(Arc<AtomicBool>);

impl ScalingPolicy for TogglePolicy {
    fn start_worker(&self, _state: &ScalingState) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[test]
fn growth_spawn_failure_cancels_previously_accepted_work() {
    with_watchdog(|| {
        let allowed = Arc::new(AtomicBool::new(false));
        let pool = Pool::builder()
            .hardware(hardware())
            .worker_policy(WorkerPolicy::per_system(nz!(1_u32)))
            .scaling_policy(TogglePolicy(Arc::clone(&allowed)))
            .build();
        pin(&pool.inner.hardware, 0);
        let scheduler = pool.scheduler();
        let accepted = scheduler.spawn(|| 42);
        let target = pool.inner.policy.destination(&pool.inner.hardware);
        let state = pool.inner.registry.get_or_init(target.group);

        allowed.store(true, Ordering::SeqCst);
        assert_panics(|| {
            pool.inner.grow_if_busy_with(
                target.group,
                state,
                |_, _| Err(io::Error::other("injected growth failure")),
                ProcessorSet::pin_current_thread_to,
            );
        });

        let error = accepted.now_or_never().unwrap().unwrap_err();
        assert!(error.is_cancel());
        assert!(!error.is_panic());
        assert!(pool.inner.poisoned.load(Ordering::Acquire));
        assert!(pool.inner.shutdown.load(Ordering::Acquire));
    });
}
