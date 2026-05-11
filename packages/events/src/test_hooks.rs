//! Deterministic race-condition testing infrastructure.
//!
//! Race-prone branches in `poll_wait()` only fire when another thread
//! mutates shared state inside a small window during the poll. Pure
//! stress tests can only cover them statistically. This module lets a
//! test install a closure at a specific callsite inside production
//! code; when the production thread reaches the callsite it runs the
//! closure, which is typically a pair of barriers ("entered" and
//! "proceed"). The test thread waits on `entered`, performs the racing
//! operation, then releases `proceed`. The branch is then guaranteed
//! to fire.
//!
//! Only threads that have explicitly opted in by setting
//! [`HOOK_PARTICIPANT`] to `true` will trigger the hooks, so concurrent
//! unrelated tests are unaffected.
//!
//! Modelled on the equivalent mechanism in `events_once::core::sync`.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Barrier, Mutex};

pub(crate) type HookFn = dyn Fn() + Send + Sync;
pub(crate) type HookSlot = Mutex<Option<Arc<HookFn>>>;

/// Held while a hook-using test runs to serialize hook installation
/// across tests.
pub(crate) static HOOK_SERIALIZATION_MUTEX: Mutex<()> = Mutex::new(());

/// Auto-reset: after the pre-mutex `take_notification()` check, before
/// `inner.slow.lock()`. Targets the "post-mutex `take_notification` →
/// Ready" branch.
pub(crate) static AUTO_PRE_MUTEX: HookSlot = Mutex::new(None);

/// Auto-reset: after the post-mutex `take_notification()` check,
/// before the second `try_wait` call. Targets the "post-mutex
/// `try_wait` → Ready" branch.
pub(crate) static AUTO_PRE_TRY_WAIT: HookSlot = Mutex::new(None);

/// Auto-reset: after the post-mutex `try_wait` call, before
/// `fetch_or(HAS_WAITERS)`. Targets the "post-`fetch_or` `try_wait` →
/// Ready" branch.
pub(crate) static AUTO_PRE_FETCH_OR: HookSlot = Mutex::new(None);

/// Manual-reset: equivalent to [`AUTO_PRE_MUTEX`].
pub(crate) static MANUAL_PRE_MUTEX: HookSlot = Mutex::new(None);

/// Manual-reset: equivalent to [`AUTO_PRE_TRY_WAIT`].
pub(crate) static MANUAL_PRE_LOAD: HookSlot = Mutex::new(None);

/// Manual-reset: equivalent to [`AUTO_PRE_FETCH_OR`].
pub(crate) static MANUAL_PRE_FETCH_OR: HookSlot = Mutex::new(None);

thread_local! {
    /// Marks the current thread as a participant in a hook-based test.
    /// Only threads with this flag set will trigger hook callsites.
    pub(crate) static HOOK_PARTICIPANT: Cell<bool> = const { Cell::new(false) };
}

/// Runs the hook installed in `slot`, if any and the current thread is
/// a participant. Called from production code at hook callsites.
pub(crate) fn run(slot: &HookSlot) {
    if !HOOK_PARTICIPANT.with(Cell::get) {
        return;
    }
    let hook = slot.lock().expect(crate::NEVER_POISONED).clone();
    if let Some(hook) = hook {
        hook();
    }
}

/// Installs `closure` in `slot`, runs `body`, then clears the slot.
/// Holds [`HOOK_SERIALIZATION_MUTEX`] for the duration so concurrent
/// hook tests do not interfere with one another. Panics from `body`
/// are caught and resumed after cleanup to avoid mutex poisoning.
pub(crate) fn with_hook(slot: &HookSlot, closure: Arc<HookFn>, body: impl FnOnce()) {
    let guard = HOOK_SERIALIZATION_MUTEX
        .lock()
        .expect(crate::NEVER_POISONED);
    *slot.lock().expect(crate::NEVER_POISONED) = Some(closure);

    let result = catch_unwind(AssertUnwindSafe(body));

    *slot.lock().expect(crate::NEVER_POISONED) = None;
    drop(guard);

    if let Err(payload) = result {
        resume_unwind(payload);
    }
}

/// A pair of barriers wired to a hook closure. The hooked production
/// thread waits on `entered` once it reaches the callsite, allowing
/// the test thread to perform the racing operation. After the test
/// thread releases `proceed`, the production thread resumes.
pub(crate) struct BarrierHook {
    pub(crate) entered: Arc<Barrier>,
    pub(crate) proceed: Arc<Barrier>,
    pub(crate) hook: Arc<HookFn>,
}

pub(crate) fn barrier_hook() -> BarrierHook {
    let entered = Arc::new(Barrier::new(2));
    let proceed = Arc::new(Barrier::new(2));
    let hook: Arc<HookFn> = Arc::new({
        let entered = Arc::clone(&entered);
        let proceed = Arc::clone(&proceed);
        move || {
            entered.wait();
            proceed.wait();
        }
    });
    BarrierHook {
        entered,
        proceed,
        hook,
    }
}
