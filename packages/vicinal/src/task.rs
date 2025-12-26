//! Task wrapper for executing user-provided closures.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

use events_once::PooledSender;
use fast_time::{Clock, Instant};
use infinity_pool::define_pooled_dyn_cast;

use crate::metrics::{EXECUTION_TIME_MS, SCHEDULING_DELAY_MS};

pub(crate) trait VicinalTask: Send + Unpin + 'static {
    fn call(&mut self);
}

// Enable casting pooled items to VicinalTask trait objects.
define_pooled_dyn_cast!(VicinalTask);

pub(crate) type TaskResult<R> = Result<R, Box<dyn Any + Send>>;

struct TaskWrapper<F>
where
    F: FnOnce() + Send + Unpin + 'static,
{
    task: Option<F>,
    spawn_time: Instant,
}

impl<F> TaskWrapper<F>
where
    F: FnOnce() + Send + Unpin + 'static,
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
    F: FnOnce() + Send + Unpin + 'static,
{
    fn call(&mut self) {
        if let Some(task) = self.task.take() {
            let mut clock = Clock::new();

            // Record scheduling delay: time from spawn to execution start.
            let scheduling_delay = self.spawn_time.elapsed(&mut clock);
            SCHEDULING_DELAY_MS.with(|e| e.observe_millis(scheduling_delay));

            // Execute the task and record execution time.
            EXECUTION_TIME_MS.with(|e| e.observe_duration_millis(task));
        }
    }
}

pub(crate) fn wrap_task<R, F>(
    task: F,
    sender: PooledSender<TaskResult<R>>,
    spawn_time: Instant,
) -> impl VicinalTask
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + Unpin + 'static,
{
    TaskWrapper::new(
        move || {
            let result = panic::catch_unwind(AssertUnwindSafe(task));
            sender.send(result);
        },
        spawn_time,
    )
}

#[cfg(test)]
mod tests {
    use events_once::EventLake;
    use fast_time::Clock;
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn wrap_task_sends_return_value() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<i32>>();
        let mut clock = Clock::new();
        let spawn_time = clock.now();

        let mut wrapped = wrap_task(|| 42, sender, spawn_time);
        wrapped.call();

        let result = block_on(receiver).unwrap();
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn wrap_task_captures_panic() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let mut clock = Clock::new();
        let spawn_time = clock.now();

        let mut wrapped = wrap_task(
            || {
                panic!("test panic");
            },
            sender,
            spawn_time,
        );
        wrapped.call();

        let result = block_on(receiver).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn wrap_task_captures_panic_with_payload() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let mut clock = Clock::new();
        let spawn_time = clock.now();

        let mut wrapped = wrap_task(
            || {
                panic!("specific message");
            },
            sender,
            spawn_time,
        );
        wrapped.call();

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
        let mut clock = Clock::new();
        let spawn_time = clock.now();

        let mut wrapped = wrap_task(
            || {
                COUNTER.fetch_add(1, Ordering::Relaxed);
            },
            sender,
            spawn_time,
        );

        wrapped.call();
        wrapped.call(); // Second call should be a no-op.

        assert_eq!(COUNTER.load(Ordering::Relaxed), 1);
    }
}
