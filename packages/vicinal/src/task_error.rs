//! Recoverable outcomes for joined tasks that did not return a value.

use std::any::{Any, type_name};
use std::fmt;
use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::OhnoCore;

use crate::TaskPanic;

/// A task that was cancelled before execution or panicked while executing.
///
/// Use [`is_cancel()`][Self::is_cancel] to decide whether to recover and execute the original
/// task through [`into_body()`][Self::into_body]. Use [`is_panic()`][Self::is_panic] and
/// [`into_panic()`][Self::into_panic] to inspect or resume a task's panic.
///
/// A cancelled task retains its captures until the error or recovered body is dropped or the
/// body is executed. Dropping a panic error discards the payload without resuming the panic.
#[derive(ohno::Error)]
#[no_constructors]
pub struct TaskError<R> {
    failure: TaskFailure<R>,
    #[error]
    core: OhnoCore,
}

// Inspection does not expose or mutate user state; extraction transfers exclusive ownership.
// The opaque captures and panic payload cannot be observed through a partially updated error.
impl<R> UnwindSafe for TaskError<R> {}
impl<R> RefUnwindSafe for TaskError<R> {}

impl<R> TaskError<R> {
    /// Returns whether the task was cancelled before its body was called.
    #[must_use]
    pub fn is_cancel(&self) -> bool {
        matches!(self.failure, TaskFailure::Cancelled(_))
    }

    /// Returns whether the task panicked while executing.
    #[must_use]
    pub fn is_panic(&self) -> bool {
        matches!(self.failure, TaskFailure::Panicked(_))
    }

    /// Recovers the original body of a cancelled task.
    ///
    /// The returned body can be executed directly or submitted to another scheduler.
    /// Returns the unchanged error if the task panicked.
    ///
    /// # Example
    ///
    /// ```
    /// use vicinal::Pool;
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let pool = Pool::new();
    /// let scheduler = pool.scheduler();
    /// drop(pool);
    ///
    /// let result = scheduler.spawn(|| 42).await;
    /// let value = match result {
    ///     Ok(value) => value,
    ///     Err(error) => error.into_body()?(),
    /// };
    /// assert_eq!(value, 42);
    /// # Ok::<(), vicinal::TaskError<i32>>(())
    /// # }).unwrap();
    /// ```
    pub fn into_body(self) -> Result<Box<dyn FnOnce() -> R + Send>, Self> {
        match self {
            Self {
                failure: TaskFailure::Cancelled(body),
                ..
            } => Ok(body),
            error => Err(error),
        }
    }

    /// Recovers the original panic payload of a task.
    ///
    /// The payload can be inspected with [`Any`] or passed to [`std::panic::resume_unwind`].
    /// Returns the unchanged error if the task was cancelled.
    pub fn into_panic(self) -> Result<Box<dyn Any + Send>, Self> {
        match self {
            Self {
                failure: TaskFailure::Panicked(payload),
                ..
            } => Ok(payload.into_payload()),
            error => Err(error),
        }
    }

    pub(crate) fn cancelled(body: impl FnOnce() -> R + Send + 'static) -> Self {
        Self {
            failure: TaskFailure::Cancelled(Box::new(body)),
            core: OhnoCore::from(TaskCancelled::new()),
        }
    }

    pub(crate) fn panicked(payload: Box<dyn Any + Send>) -> Self {
        Self {
            failure: TaskFailure::Panicked(TaskPanic::new(payload)),
            core: OhnoCore::from(TaskPanicked::new()),
        }
    }
}

/// Keeps recovery state outside the shared diagnostic source chain.
///
/// Captures and panic payloads need only be `Send`, and extraction must preserve ownership.
enum TaskFailure<R> {
    Cancelled(Box<dyn FnOnce() -> R + Send>),
    Panicked(TaskPanic),
}

impl<R> fmt::Debug for TaskFailure<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Cancelled(_) => "cancelled",
            Self::Panicked(_) => "panicked",
        };
        f.debug_tuple(type_name::<Self>()).field(&kind).finish()
    }
}

/// Diagnoses cancellation independently of the task's recoverable captures.
#[ohno::error]
#[display("task cancelled before execution")]
struct TaskCancelled;

/// Diagnoses a task panic without formatting or sharing its arbitrary payload.
#[ohno::error]
#[display("task panicked")]
struct TaskPanicked;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::error::Error;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::pool_inner::PanickingPayload;

    assert_impl_all!(TaskError<Cell<()>>: Error, Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(TaskError<()>: Sync);

    #[test]
    fn cancelled_body_survives_wrong_extraction() {
        let capture = Cell::new(String::from("captured value"));
        let error = TaskError::cancelled(move || capture.into_inner());
        assert!(error.is_cancel());
        assert!(!error.is_panic());
        let error = error.into_panic().unwrap_err();
        assert!(error.is_cancel());
        assert_eq!(error.into_body().unwrap()(), "captured value");
    }

    #[test]
    fn panic_payload_survives_wrong_extraction() {
        let error = TaskError::<()>::panicked(Box::new(Cell::new(String::from("panic payload"))));
        assert!(error.is_panic());
        assert!(!error.is_cancel());
        let error = error.into_body().err().unwrap();
        let payload = error.into_panic().unwrap();
        assert_eq!(
            payload.downcast::<Cell<String>>().unwrap().into_inner(),
            "panic payload"
        );
    }

    #[test]
    fn discarded_panic_contains_payload_destructor_failure() {
        drop(TaskError::<()>::panicked(Box::new(PanickingPayload)));
    }

    #[test]
    fn result_needs_no_debug_implementation() {
        struct Output;

        let error = TaskError::cancelled(|| Output);
        assert!(!format!("{error:?}").is_empty());
        let _: Output = error.into_body().unwrap()();
    }
}
