//! Task representation and the wrappers that adapt user-provided closures to it.

use std::mem::MaybeUninit;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;

use events_once::PooledSender;
use fast_time::Instant;
use pin_project::pin_project;
use plurality::{Box as PoolBox, coerce};
use tracing::error;

use crate::metrics::{CLOCK, EXECUTION_TIME_MS, SCHEDULING_DELAY_MS};
use crate::{TaskError, discard_panic, extract_panic_message};

/// Groups task execution and panic-disposal diagnostics under the task owner.
pub(crate) const TASK_LOG_TARGET: &str = module_path!();

/// Unit of work handed to a worker thread.
///
/// Every queued task is stored behind `dyn VicinalTask`, so this trait is what the worker
/// loop knows about a task: it can run it exactly once, in place, without knowing the
/// closure type or return type it was built from. The `Send` supertrait is what allows the
/// erased form to travel from the spawning thread to the worker thread.
pub(crate) trait VicinalTask: Send + 'static {
    fn call(self: Pin<&mut Self>);
}

/// Owning handle to a type-erased task, as stored in a worker group's task queue.
///
/// The handle owns its pool slot, so queueing, dequeuing and executing a task need no further
/// contact with the pool or the lock that guards allocation.
pub(crate) type ErasedTaskHandle = PoolBox<dyn VicinalTask>;

/// A reserved task slot that can be initialized after releasing the pool lock.
pub(crate) type UninitTaskHandle<T> = PoolBox<MaybeUninit<T>>;

/// Initializes a reserved slot with `task` and erases its type.
pub(crate) fn init_task<T: VicinalTask>(
    mut handle: UninitTaskHandle<T>,
    task: T,
) -> ErasedTaskHandle {
    {
        let slot = handle.as_pin_mut();
        // SAFETY: The uniquely owned slot is still uninitialized, so there is no pinned `T`
        // that could be moved through this mutable reference. The task is written in place.
        let slot = unsafe { Pin::get_unchecked_mut(slot) };
        slot.write(task);
    }

    // SAFETY: The task was written into this slot immediately above.
    let handle = unsafe { handle.assume_init() };

    PoolBox::unsize(handle, coerce!(dyn VicinalTask))
}

/// A joined task's success, cancellation or panic.
pub(crate) type TaskResult<R> = Result<R, TaskError<R>>;

/// Adds timing around a once-only task body without changing its completion or panic handling.
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

        if this.task.is_none() {
            return;
        }

        // Record scheduling delay: time from spawn to execution start.
        let scheduling_delay = CLOCK.with_borrow_mut(|clock| this.spawn_time.elapsed(clock));
        SCHEDULING_DELAY_MS.with(|event| event.observe_millis(scheduling_delay));

        let start = CLOCK.with_borrow_mut(fast_time::Clock::now);

        // Leave captures in their owning handle until fallible metric preparation completes.
        let task = this
            .task
            .take()
            .expect("an unexecuted task remains available");
        task();

        let elapsed = CLOCK.with_borrow_mut(|clock| start.elapsed(clock));
        EXECUTION_TIME_MS.with(|event| event.observe_millis(elapsed));
    }
}

/// Preserves the original body until execution or cancellation transfers it to the caller.
struct JoinedTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    pending: Option<(F, PooledSender<TaskResult<R>>)>,
}

impl<F, R> JoinedTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    // Omitting execution strands tests waiting for task-provided synchronization signals.
    #[cfg_attr(test, mutants::skip)]
    fn run(mut self) {
        let (task, sender) = self.pending.take().expect("a joined task is executed once");
        let result = panic::catch_unwind(AssertUnwindSafe(task)).map_err(TaskError::panicked);

        sender.send(result);
    }
}

impl<F, R> Drop for JoinedTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    fn drop(&mut self) {
        if let Some((task, sender)) = self.pending.take() {
            // Only cancellation boxes the body; successful dispatch retains pooled storage.
            // Queue draining and rejected admission release tasks outside pool locks.
            sender.send(Err(TaskError::cancelled(task)));
        }
    }
}

/// Adds completion and cancellation delivery without boxing the submitted body.
pub(crate) fn wrap_task<R, F>(
    task: F,
    sender: PooledSender<TaskResult<R>>,
    spawn_time: Instant,
) -> impl VicinalTask
where
    R: Send + 'static,
    F: FnOnce() -> R + Send + 'static,
{
    let joined = JoinedTask {
        pending: Some((task, sender)),
    };
    TaskWrapper::new(move || joined.run(), spawn_time)
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
                error!(message, "fire-and-forget task panicked");
                discard_panic(payload);
            }
        },
        spawn_time,
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::hint::black_box;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

    use events_once::EventLake;
    use futures::FutureExt;
    use futures::executor::block_on;
    use plurality::MultiPool;

    use super::*;
    use crate::pool_inner::OnDrop;

    /// Makes capacity growth observable after live tasks fill an initial chunk.
    const TEST_CHUNK_SIZE: u32 = 2;

    /// Uses a meaningfully different task layout without making the test expensive.
    const LARGE_TASK_WORDS: usize = 8;

    /// The reuse test deliberately exercises unrelated allocation layouts.
    const EXPECTED_LAYOUT_COUNT: usize = 2;

    /// A compact task used to exercise one internal layout pool.
    struct SmallTask(u8);

    impl VicinalTask for SmallTask {
        fn call(self: Pin<&mut Self>) {
            _ = black_box(self.0);
        }
    }

    /// A larger task used to exercise an independent internal layout pool.
    struct LargeTask([u64; LARGE_TASK_WORDS]);

    impl VicinalTask for LargeTask {
        fn call(self: Pin<&mut Self>) {
            _ = black_box(self.0);
        }
    }

    fn alloc_test_task<T: VicinalTask>(pool: &MultiPool, task: T) -> ErasedTaskHandle {
        init_task(pool.alloc_uninit_box(), task)
    }

    #[test]
    fn released_task_slots_are_reused_per_layout() {
        let pool = MultiPool::builder().chunk_size(TEST_CHUNK_SIZE).build();
        let small_held = alloc_test_task(&pool, SmallTask(u8::default()));
        let small_released = alloc_test_task(&pool, SmallTask(u8::default()));
        let large_held = alloc_test_task(&pool, LargeTask([u64::default(); LARGE_TASK_WORDS]));
        let large_released = alloc_test_task(&pool, LargeTask([u64::default(); LARGE_TASK_WORDS]));

        assert_eq!(pool.layouts(), EXPECTED_LAYOUT_COUNT);
        assert_eq!(pool.capacity_of::<SmallTask>(), u64::from(TEST_CHUNK_SIZE));
        assert_eq!(pool.capacity_of::<LargeTask>(), u64::from(TEST_CHUNK_SIZE));

        drop((small_released, large_released));
        let small_replacement = alloc_test_task(&pool, SmallTask(u8::default()));
        let large_replacement =
            alloc_test_task(&pool, LargeTask([u64::default(); LARGE_TASK_WORDS]));

        assert_eq!(pool.capacity_of::<SmallTask>(), u64::from(TEST_CHUNK_SIZE));
        assert_eq!(pool.capacity_of::<LargeTask>(), u64::from(TEST_CHUNK_SIZE));

        drop((small_held, small_replacement, large_held, large_replacement));
        assert!(pool.is_empty());
    }

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
    fn unexecuted_task_returns_original_body() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<String>>();
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);
        let capture = String::from("owned task capture");

        drop(wrap_task(move || capture, sender, spawn_time));

        let error = receiver.now_or_never().unwrap().unwrap().unwrap_err();

        assert!(error.is_cancel());
        assert_eq!(error.into_body().unwrap()(), "owned task capture");
    }

    #[test]
    fn cancellation_retains_captures_until_result_is_dropped() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let capture = OnDrop(Some(Box::new({
            let dropped = Arc::clone(&dropped);
            move || {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        })));
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        drop(wrap_task(move || drop(capture), sender, spawn_time));
        assert_eq!(dropped.load(Ordering::Relaxed), 0);

        let error = receiver.now_or_never().unwrap().unwrap().unwrap_err();
        assert_eq!(dropped.load(Ordering::Relaxed), 0);

        drop(error);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn dropping_receiver_does_not_cancel_execution() {
        let executed = Arc::new(AtomicBool::new(false));
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);
        let wrapped = wrap_task(
            {
                let executed = Arc::clone(&executed);
                move || executed.store(true, Ordering::Relaxed)
            },
            sender,
            spawn_time,
        );

        drop(receiver);
        Box::pin(wrapped).as_mut().call();

        assert!(executed.load(Ordering::Relaxed));
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
        let panic_payload = result.unwrap_err().into_panic().unwrap();

        // The panic payload should contain our message.
        let message = panic_payload
            .downcast_ref::<&str>()
            .copied()
            .unwrap_or("unknown");
        assert_eq!(message, "specific message");
    }

    #[test]
    fn call_only_executes_once() {
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
    }
}
