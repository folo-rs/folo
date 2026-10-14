//! Recoverable outcomes for joined tasks that did not return a value.

use std::any::{Any, type_name};
use std::error::Error;
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
/// When recovery is unnecessary, [`into_diagnostic()`][Self::into_diagnostic] discards recovery
/// data and retains a thread-safe error for reporting.
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
    /// # use vicinal::TaskError;
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
    /// # Ok::<(), TaskError<i32>>(())
    /// # }).unwrap();
    /// ```
    pub fn into_body(self) -> Result<impl FnOnce() -> R + Send + 'static, Self>
    where
        R: 'static,
    {
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

    /// Discards recovery data and retains the error's diagnostics.
    ///
    /// Drops the cancelled task body or captured panic payload. The returned error retains
    /// the diagnostic message, source chain, backtrace and enrichment.
    ///
    /// Use this when recovery is unnecessary and error reporting requires `Send + Sync`.
    ///
    /// # Example
    ///
    /// ```
    /// # use std::error::Error;
    /// use vicinal::{Pool, TaskError};
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let pool = Pool::new();
    /// let value = pool
    ///     .scheduler()
    ///     .spawn(|| 42)
    ///     .await
    ///     .map_err(TaskError::into_diagnostic)?;
    /// assert_eq!(value, 42);
    /// # Ok::<(), Box<dyn Error + Send + Sync>>(())
    /// # }).unwrap();
    /// ```
    #[must_use]
    pub fn into_diagnostic(self) -> impl Error + Send + Sync + 'static {
        let Self { failure, core } = self;
        drop(failure);
        TaskDiagnostic { core }
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

// Identify the outcome without inspecting recovery values or requiring R: Debug.
// Recovery state remains opaque; see docs/implementation.md.
impl<R> fmt::Debug for TaskFailure<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Cancelled(_) => "cancelled",
            Self::Panicked(_) => "panicked",
        };
        f.debug_tuple(type_name::<Self>()).field(&kind).finish()
    }
}

/// Carries the original diagnostics after recovery state is discarded.
#[derive(ohno::Error)]
#[no_constructors]
struct TaskDiagnostic {
    #[error]
    core: OhnoCore,
}

// The core's erased source blocks inference; inspecting diagnostics cannot expose partial mutation.
impl UnwindSafe for TaskDiagnostic {}
impl RefUnwindSafe for TaskDiagnostic {}

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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{ptr, thread};

    use ohno::{AppError, Enrichable, EnrichmentEntry, ErrorExt};
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::pool_inner::{OnDrop, PanickingPayload};

    assert_impl_all!(TaskError<Cell<()>>: Error, Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(TaskError<()>: Sync);
    assert_impl_all!(TaskDiagnostic: Error, Send, Sync, UnwindSafe, RefUnwindSafe);

    /// Recovery data is released during conversion, not when the diagnostic is dropped.
    ///
    /// Ref: docs/implementation.md, "Task storage".
    #[test]
    fn diagnostic_conversion_releases_recovery_data() {
        for panicked in [false, true] {
            let dropped = Arc::new(AtomicUsize::new(0));
            let capture = OnDrop(Some(Box::new({
                let dropped = Arc::clone(&dropped);
                move || {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            })));
            let error = if panicked {
                TaskError::panicked(Box::new(capture))
            } else {
                TaskError::cancelled(move || drop(capture))
            };

            let diagnostic = error.into_diagnostic();
            assert_eq!(dropped.load(Ordering::Relaxed), 1);
            if panicked {
                assert!(diagnostic.source().unwrap().is::<TaskPanicked>());
            } else {
                assert!(diagnostic.source().unwrap().is::<TaskCancelled>());
            }
            drop(diagnostic);
            assert_eq!(dropped.load(Ordering::Relaxed), 1);
        }
    }

    /// Preserves the source and backtrace objects, not just matching diagnostic text.
    ///
    /// Ref: docs/implementation.md, "Task storage".
    #[test]
    fn diagnostic_conversion_preserves_the_original_core() {
        let mut error = TaskError::cancelled(|| {});
        error.add_enrichment(EnrichmentEntry::new("diagnostic canary", file!(), line!()));
        let message = error.to_string();
        let source = ptr::from_ref(error.source().unwrap());
        let backtrace = ptr::from_ref(error.backtrace());

        let diagnostic = error.into_diagnostic();
        assert_eq!(diagnostic.to_string(), message);
        assert!(diagnostic.to_string().contains("diagnostic canary"));
        assert!(ptr::addr_eq(diagnostic.source().unwrap(), source));
        let diagnostic: &dyn Error = &diagnostic;
        let diagnostic = diagnostic.downcast_ref::<TaskDiagnostic>().unwrap();
        assert!(ptr::eq(diagnostic.core.backtrace(), backtrace));
    }

    #[test]
    fn diagnostic_conversion_contains_panicking_payload_disposal() {
        let diagnostic = TaskError::<()>::panicked(Box::new(PanickingPayload)).into_diagnostic();
        assert!(diagnostic.source().unwrap().is::<TaskPanicked>());
    }

    #[test]
    fn diagnostic_converts_to_application_error_without_flattening() {
        let error: AppError = TaskError::<()>::panicked(Box::new(()))
            .into_diagnostic()
            .into();
        assert!(error.find_source::<TaskDiagnostic>().is_some());
        assert!(error.find_source::<TaskPanicked>().is_some());
    }

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
    fn recovered_body_can_run_on_another_thread() {
        let capture = Cell::new(String::from("transferred capture"));
        let body = TaskError::cancelled(move || capture.into_inner())
            .into_body()
            .unwrap();

        assert_eq!(thread::spawn(body).join().unwrap(), "transferred capture");
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

        let error = TaskError::<Output>::panicked(Box::new(()));
        assert!(!format!("{error:?}").is_empty());
    }
}
