//! Join handle for awaiting task completion.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use events_once::PooledReceiver;

use crate::{TaskError, TaskResult};

/// A handle to a spawned task that can be awaited to retrieve its result.
///
/// Awaiting returns `Ok(value)` when the task completes, or a [`TaskError`] for cancellation
/// or a task panic. Cancellation occurs before the task body is called, including work queued
/// during shutdown or an infrastructure failure and submissions after clean shutdown.
///
/// Dropping the handle does not cancel its task. A cancelled task's captures remain alive
/// until its handle, error or recovered body is consumed or dropped.
///
/// # Reentrancy
///
/// A completion wake callback may drop this handle or other handles and submit more work.
/// Submissions remain subject to the pool's shutdown and infrastructure-failure rules.
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
    type Output = Result<R, TaskError<R>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let receiver = &mut self.receiver;
        let pinned_receiver = Pin::new(receiver);

        match pinned_receiver.poll(cx) {
            Poll::Pending => Poll::Pending,
            // Every unexecuted wrapper sends cancellation before releasing its sender.
            Poll::Ready(result) => Poll::Ready(
                result.expect("joined tasks deliver completion or cancellation before dropping"),
            ),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use events_once::EventLake;
    use futures::executor::block_on;

    use super::*;

    static_assertions::assert_impl_all!(
        JoinHandle<String>: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn await_returns_value() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<i32>>();
        let handle = JoinHandle::new(receiver);

        sender.send(Ok(42));

        let result = block_on(handle).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn await_returns_unit() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let handle = JoinHandle::new(receiver);

        sender.send(Ok(()));

        block_on(handle).unwrap();
    }

    #[test]
    fn await_returns_panic() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let handle = JoinHandle::new(receiver);

        sender.send(Err(TaskError::panicked(Box::new("test panic"))));
        let error = block_on(handle).unwrap_err();
        assert!(error.is_panic());
        assert_eq!(
            *error.into_panic().unwrap().downcast::<&str>().unwrap(),
            "test panic"
        );
    }

    #[test]
    #[should_panic]
    fn missing_outcome_is_an_invariant_failure() {
        let lake = EventLake::new();
        let (sender, receiver) = lake.rent::<TaskResult<()>>();
        let handle = JoinHandle::new(receiver);

        // A disconnected channel without an outcome is not a valid cancellation.
        drop(sender);

        _ = block_on(handle);
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
