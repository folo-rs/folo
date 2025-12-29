//! Task wrapper for executing user-provided closures.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;

use events_once::PooledSender;
use fast_time::Instant;
use infinity_pool::define_pooled_dyn_cast;
use pin_project::pin_project;

use crate::metrics::{CLOCK, EXECUTION_TIME_MS, SCHEDULING_DELAY_MS};

pub(crate) trait VicinalTask: Send + 'static {
    fn call(self: Pin<&mut Self>);
}

// Enable casting pooled items to VicinalTask trait objects.
define_pooled_dyn_cast!(VicinalTask);

pub(crate) type TaskResult<R> = Result<R, Box<dyn Any + Send>>;

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

pub(crate) fn wrap_task_and_forget<F>(task: F, spawn_time: Instant) -> impl VicinalTask
where
    F: FnOnce() + Send + 'static,
{
    TaskWrapper::new(
        move || {
            let result = panic::catch_unwind(AssertUnwindSafe(task));
            if let Err(panic_payload) = result {
                // Log the panic instead of forwarding it.
                let message = if let Some(&s) = panic_payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic payload".to_string()
                };
                tracing::error!("Task panicked: {}", message);
            }
        },
        spawn_time,
    )
}

#[cfg(test)]
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
    fn wrap_task_and_forget_executes_task() {
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
    fn wrap_task_and_forget_logs_panic() {
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let wrapped = wrap_task_and_forget(
            || {
                panic!("test panic in and_forget");
            },
            spawn_time,
        );

        // The panic should be caught and logged, not propagated.
        Box::pin(wrapped).as_mut().call();
        // If we reach here, the panic was successfully caught.
    }
}
