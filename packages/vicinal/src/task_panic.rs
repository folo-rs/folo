//! Panic payload ownership and bounded disposal during task and pool cleanup.

use std::any::Any;
use std::mem::forget;
use std::panic::{AssertUnwindSafe, catch_unwind};

use tracing::error;

use crate::TASK_LOG_TARGET;

/// Keeps discarded task panics isolated from infrastructure, on either result-channel endpoint.
#[derive(Debug)]
pub(crate) struct TaskPanic {
    payload: Option<Box<dyn Any + Send>>,
}

impl TaskPanic {
    pub(crate) fn new(payload: Box<dyn Any + Send>) -> Self {
        Self {
            payload: Some(payload),
        }
    }

    pub(crate) fn into_payload(mut self) -> Box<dyn Any + Send> {
        self.payload.take().expect("a task panic is consumed once")
    }
}

impl Drop for TaskPanic {
    fn drop(&mut self) {
        if let Some(payload) = self.payload.take() {
            discard_panic(payload);
        }
    }
}

/// Extracts a human-readable message from the usual `&str` and `String` panic payloads.
pub(crate) fn extract_panic_message(payload: &Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Emits a failure diagnostic without allowing a subscriber panic to interrupt cleanup.
pub(crate) fn report_diagnostic(report: impl FnOnce()) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(report)) {
        // A subscriber panic is an unrelated diagnostic failure. Its payload may have a
        // panicking destructor, so retaining it is the only bounded cleanup path.
        forget(payload);
    }
}

/// Disposes of a suppressed failure without allowing its destructor to interrupt pool cleanup.
pub(crate) fn discard_panic(payload: Box<dyn Any + Send>) {
    if let Err(replacement) = catch_unwind(AssertUnwindSafe(|| drop(payload))) {
        // A replacement can itself have a panicking destructor. Retain it rather than
        // recursively disposing of arbitrary user payloads; see docs/implementation.md.
        forget(replacement);
        report_diagnostic(|| {
            error!(
                target: TASK_LOG_TARGET,
                "panic payload destruction failed; replacement payload retained"
            );
        });
    }
}

/// Releases task captures outside an active unwind while preserving an earlier failure.
pub(crate) fn discard_task_after_failure<T>(task: T) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(task))) {
        report_diagnostic(|| {
            error!(
                target: TASK_LOG_TARGET,
                message = extract_panic_message(&payload),
                "task cleanup failed while handling an infrastructure failure"
            );
        });
        discard_panic(payload);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

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
