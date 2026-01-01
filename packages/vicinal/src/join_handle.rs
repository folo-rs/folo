//! Join handle for awaiting task completion.

use std::future::Future;
use std::panic;
use std::pin::Pin;
use std::task::{Context, Poll};

use events_once::PooledReceiver;

use crate::TaskResult;

/// A handle to a spawned task that can be awaited to retrieve its result.
///
/// Awaiting the join handle returns the task's return value. If the task panicked,
/// awaiting the join handle will re-throw the panic.
///
/// # Panics
///
/// Awaiting the join handle panics if:
/// - The task panicked (the panic is re-thrown).
/// - The pool was shut down before the task could execute (the channel is disconnected).
#[derive(Debug)]
pub struct JoinHandle<R>
where
    R: Send + 'static,
{
    receiver: PooledReceiver<TaskResult<R>>,
}

impl<R> JoinHandle<R>
where
    R: Send + 'static,
{
    /// Creates a new join handle from a receiver.
    pub(crate) fn new(receiver: PooledReceiver<TaskResult<R>>) -> Self {
        Self { receiver }
    }
}

impl<R> Future for JoinHandle<R>
where
    R: Send + 'static,
{
    type Output = R;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let receiver = &mut self.receiver;
        let pinned_receiver = Pin::new(receiver);

        match pinned_receiver.poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => match result {
                Ok(Ok(value)) => Poll::Ready(value),
                Ok(Err(panic_payload)) => {
                    panic::resume_unwind(panic_payload);
                }
                Err(_disconnected) => {
                    // Channel disconnected - pool was shut down.
                    panic!("task was abandoned because the pool was shut down");
                }
            },
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::AssertUnwindSafe;

    use events_once::EventLake;
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn await_returns_value() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<i32>>();
        let handle = JoinHandle::new(receiver);

        sender.send(Ok(42));

        let result = block_on(handle);
        assert_eq!(result, 42);
    }

    #[test]
    fn await_returns_unit() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let handle = JoinHandle::new(receiver);

        sender.send(Ok(()));

        block_on(handle);
    }

    #[test]
    #[should_panic]
    fn await_rethrows_panic() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let handle = JoinHandle::new(receiver);

        // Capture a real panic to get a proper payload.
        let panic_result = panic::catch_unwind(AssertUnwindSafe(|| {
            panic!("test panic");
        }));

        sender.send(Err(panic_result.unwrap_err()));

        block_on(handle);
    }

    #[test]
    #[should_panic]
    fn await_panics_on_disconnect() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let handle = JoinHandle::new(receiver);

        // Drop sender without sending, simulating pool shutdown.
        drop(sender);

        block_on(handle);
    }

    #[test]
    fn drop_without_await_does_not_panic() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<i32>>();
        let handle = JoinHandle::new(receiver);

        sender.send(Ok(42));

        // Just drop the handle without awaiting.
        drop(handle);
    }
}
