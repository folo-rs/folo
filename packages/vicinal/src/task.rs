//! Task representation and the wrappers that adapt user-provided closures to it.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;

use events_once::PooledSender;
use fast_time::Instant;
use multitude::dst::pointee;
use multitude::{Arena, coerce};
use pin_project::pin_project;

use crate::metrics::{CLOCK, EXECUTION_TIME_MS, SCHEDULING_DELAY_MS};

/// Unit of work handed to a worker thread.
///
/// Every queued task is stored behind `dyn VicinalTask`, so this trait is what the worker
/// loop knows about a task: it can run it exactly once, in place, without knowing the
/// closure type or return type it was built from. The `Send` supertrait is what allows the
/// erased form to travel from the spawning thread to the worker thread.
///
/// The `pointee` attribute supplies the pointer-metadata implementation that
/// [`multitude::Box`] requires of its unsized targets. `pointee` is `ptr_meta`'s macro,
/// re-exported by `multitude`, and it emits `::ptr_meta::*` paths unless told otherwise;
/// the `crate` argument redirects those at `multitude`'s re-export so this package needs
/// no direct dependency on `ptr_meta`. Depending on `ptr_meta` directly would be worse: the
/// generated impls must target the exact `ptr_meta` instance `multitude` was built against,
/// so a version skew would surface as an unrelated-looking trait mismatch.
#[pointee(crate = ::multitude::dst)]
pub(crate) trait VicinalTask: Send + 'static {
    fn call(self: Pin<&mut Self>);
}

/// Owning handle to a type-erased task, as stored in a processor's task queue.
///
/// The handle owns its slice of the arena outright and keeps the backing storage alive on
/// its own, so queueing, dequeuing and executing a task need no further contact with the
/// arena or the lock that guards it.
pub(crate) type ErasedTaskHandle = Pin<multitude::Box<dyn VicinalTask>>;

/// Moves `task` into the arena and erases its type.
pub(crate) fn alloc_task(arena: &Arena, task: impl VicinalTask) -> ErasedTaskHandle {
    let handle = arena.alloc_box(task);

    multitude::Box::into_pin(multitude::Box::unsize(handle, coerce!(dyn VicinalTask)))
}

/// Outcome of running a task to completion: either its return value or the payload of the
/// panic that unwound out of it.
pub(crate) type TaskResult<R> = Result<R, Box<dyn Any + Send>>;

/// Extracts a human-readable message from a panic payload.
///
/// Handles both `&str` and `String` panic payloads, which are the most common
/// panic types produced by the `panic!` macro.
fn extract_panic_message(payload: &Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[pin_project]
struct TaskWrapper<F>
where
    F: FnOnce() + Send + 'static,
{
    task: Option<F>,
    spawn_time: Instant,
}

impl<F> TaskWrapper<F>
where
    F: FnOnce() + Send + 'static,
{
    fn new(task: F, spawn_time: Instant) -> Self {
        Self {
            task: Some(task),
            spawn_time,
        }
    }
}

impl<F> VicinalTask for TaskWrapper<F>
where
    F: FnOnce() + Send + 'static,
{
    fn call(self: Pin<&mut Self>) {
        let this = self.project();

        let Some(task) = this.task.take() else { return };

        // Record scheduling delay: time from spawn to execution start.
        let scheduling_delay = CLOCK.with_borrow_mut(|clock| this.spawn_time.elapsed(clock));
        SCHEDULING_DELAY_MS.with(|e| e.observe_millis(scheduling_delay));

        // Execute the task and record execution time.
        let start = CLOCK.with_borrow_mut(fast_time::Clock::now);
        task();
        let elapsed = CLOCK.with_borrow_mut(|clock| start.elapsed(clock));
        EXECUTION_TIME_MS.with(|e| e.observe_millis(elapsed));
    }
}

pub(crate) fn wrap_task<R, F>(
    task: F,
    sender: PooledSender<TaskResult<R>>,
    spawn_time: Instant,
) -> impl VicinalTask
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    TaskWrapper::new(
        move || {
            let result = panic::catch_unwind(AssertUnwindSafe(task));
            sender.send(result);
        },
        spawn_time,
    )
}

/// Wraps a task for "fire-and-forget" execution without a result channel.
///
/// Panics are caught and logged instead of being forwarded through a channel.
/// Metrics for scheduling delay and execution time are still recorded.
pub(crate) fn wrap_task_and_forget<F>(task: F, spawn_time: Instant) -> impl VicinalTask
where
    F: FnOnce() + Send + 'static,
{
    TaskWrapper::new(
        move || {
            let result = panic::catch_unwind(AssertUnwindSafe(task));
            if let Err(payload) = result {
                let message = extract_panic_message(&payload);
                tracing::error!(message, "fire-and-forget task panicked");
            }
        },
        spawn_time,
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use events_once::EventLake;
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn wrap_task_sends_return_value() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<i32>>();
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let wrapped = wrap_task(|| 42, sender, spawn_time);
        Box::pin(wrapped).as_mut().call();

        let result = block_on(receiver).unwrap();
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn wrap_task_captures_panic() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let wrapped = wrap_task(
            || {
                panic!("test panic");
            },
            sender,
            spawn_time,
        );
        Box::pin(wrapped).as_mut().call();

        let result = block_on(receiver).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn wrap_task_captures_panic_with_payload() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let wrapped = wrap_task(
            || {
                panic!("specific message");
            },
            sender,
            spawn_time,
        );
        Box::pin(wrapped).as_mut().call();

        let result = block_on(receiver).unwrap();
        let panic_payload = result.unwrap_err();

        // The panic payload should contain our message.
        let message = panic_payload
            .downcast_ref::<&str>()
            .copied()
            .unwrap_or("unknown");
        assert_eq!(message, "specific message");
    }

    #[test]
    fn call_only_executes_once() {
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let lake = EventLake::new();
        let (sender, _receiver) = lake.rent::<TaskResult<()>>();
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let wrapped = wrap_task(
            || {
                COUNTER.fetch_add(1, Ordering::Relaxed);
            },
            sender,
            spawn_time,
        );

        let mut pinned = Box::pin(wrapped);
        pinned.as_mut().call();
        pinned.as_mut().call(); // Second call should be a no-op.

        assert_eq!(COUNTER.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn wrap_task_and_forget_executes() {
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let wrapped = wrap_task_and_forget(
            || {
                COUNTER.fetch_add(1, Ordering::Relaxed);
            },
            spawn_time,
        );

        Box::pin(wrapped).as_mut().call();

        assert_eq!(COUNTER.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn wrap_task_and_forget_catches_panic_str() {
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let wrapped = wrap_task_and_forget(
            || {
                panic!("test panic str");
            },
            spawn_time,
        );

        // The panic should be caught and logged, not propagated.
        Box::pin(wrapped).as_mut().call();
        // If we reach here, the panic was caught successfully.
    }

    #[test]
    fn wrap_task_and_forget_catches_panic_string() {
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let wrapped = wrap_task_and_forget(
            || {
                panic!("{}", "test panic String".to_string());
            },
            spawn_time,
        );

        // The panic should be caught and logged, not propagated.
        Box::pin(wrapped).as_mut().call();
        // If we reach here, the panic was caught successfully.
    }

    #[test]
    fn extract_panic_message_handles_str_ref() {
        let payload: Box<dyn Any + Send> = Box::new("panic message");
        let message = extract_panic_message(&payload);
        assert_eq!(message, "panic message");
    }

    #[test]
    fn extract_panic_message_handles_string() {
        let payload: Box<dyn Any + Send> = Box::new("owned panic".to_string());
        let message = extract_panic_message(&payload);
        assert_eq!(message, "owned panic");
    }

    #[test]
    fn extract_panic_message_handles_unknown_type() {
        let payload: Box<dyn Any + Send> = Box::new(42_i32);
        let message = extract_panic_message(&payload);
        assert_eq!(message, "unknown panic payload");
    }
}
