//! Panic payload ownership and bounded disposal during task and pool cleanup.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

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
        self.payload
            .take()
            .expect("the consuming method takes ownership before Drop can access the payload")
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

/// Drops a captured panic and re-raises a destructor panic after reporting it.
pub(crate) fn discard_panic(payload: Box<dyn Any + Send>) {
    if let Err(replacement) = catch_unwind(AssertUnwindSafe(|| drop(payload))) {
        let mut failures = vec![replacement];
        record_diagnostic_panic(&mut failures, || {
            error!(
                target: TASK_LOG_TARGET,
                "panic payload destruction failed"
            );
        });
        resume_latest_failure(failures);
    }
}

/// Releases task captures outside an active unwind while preserving an earlier failure.
pub(crate) fn discard_task_after_failure<T>(task: T) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(task))) {
        let message = extract_panic_message(&payload);
        let mut failures = vec![payload];
        record_diagnostic_panic(&mut failures, || {
            error!(
                target: TASK_LOG_TARGET,
                message,
                "task cleanup failed while handling an infrastructure failure"
            );
        });
        resume_latest_failure(failures);
    }
}

/// Records an incidental diagnostic panic without interrupting the required cleanup.
pub(crate) fn record_diagnostic_panic(
    failures: &mut Vec<Box<dyn Any + Send>>,
    diagnostic: impl FnOnce(),
) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(diagnostic)) {
        failures.push(payload);
    }
}

/// Re-raises the most recent incidental panic after releasing earlier payloads.
pub(crate) fn resume_latest_failure(mut failures: Vec<Box<dyn Any + Send>>) -> ! {
    let latest = failures
        .pop()
        .expect("failure cleanup retains at least the initiating panic");
    for failure in failures {
        drop(failure);
    }
    resume_unwind(latest);
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
