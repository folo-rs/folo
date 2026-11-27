//! Panic payload ownership and bounded disposal during task and pool cleanup.

use std::any::Any;
use std::mem::replace;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use tracing::error;

use crate::TASK_LOG_TARGET;

/// Carries caught panics to the boundary that can release superseded payloads.
#[derive(Debug)]
pub(crate) struct PanicFailures {
    earlier: Vec<Box<dyn Any + Send>>,
    latest: Box<dyn Any + Send>,
}

impl PanicFailures {
    pub(crate) fn new(payload: Box<dyn Any + Send>) -> Self {
        Self {
            earlier: Vec::new(),
            latest: payload,
        }
    }

    pub(crate) fn latest(&self) -> &Box<dyn Any + Send> {
        &self.latest
    }

    /// Appends later failures without running any payload destructors.
    pub(crate) fn append(&mut self, mut later: Self) {
        self.push(later.latest);
        self.earlier.append(&mut later.earlier);
    }

    /// Records failure at a boundary where cleanup may also succeed.
    pub(crate) fn accumulate(target: &mut Option<Self>, failures: Self) {
        if let Some(retained) = target {
            retained.append(failures);
        } else {
            *target = Some(failures);
        }
    }

    /// Lets diagnostics fail independently of the remaining cleanup work.
    pub(crate) fn record_diagnostic(&mut self, diagnostic: impl FnOnce()) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(diagnostic)) {
            self.push(payload);
        }
    }

    /// Releases superseded payloads once the owning boundary has completed cleanup.
    pub(crate) fn into_latest(self) -> Box<dyn Any + Send> {
        for payload in self.earlier {
            drop(payload);
        }
        self.latest
    }

    /// Propagates after cleanup, allowing a payload destructor panic to take precedence.
    pub(crate) fn resume(self) -> ! {
        resume_unwind(self.into_latest());
    }

    fn push(&mut self, payload: Box<dyn Any + Send>) {
        self.earlier.push(replace(&mut self.latest, payload));
    }
}

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
        let mut failures = PanicFailures::new(replacement);
        failures.record_diagnostic(|| {
            error!(
                target: TASK_LOG_TARGET,
                "panic payload destruction failed"
            );
        });
        failures.resume();
    }
}

/// Drops task state after a failure and returns any incidental cleanup panic.
pub(crate) fn discard_task_after_failure<T>(task: T) -> Option<PanicFailures> {
    let payload = catch_unwind(AssertUnwindSafe(|| drop(task))).err()?;
    let message = extract_panic_message(&payload);
    let mut failures = PanicFailures::new(payload);
    failures.record_diagnostic(|| {
        error!(
            target: TASK_LOG_TARGET,
            message,
            "task cleanup failed while handling an infrastructure failure"
        );
    });
    Some(failures)
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
