use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

/// Malformed or incompatible inputs cannot select benchmark or publication work.
#[ohno::error]
#[display("Invalid action input {input}: {detail}")]
pub(crate) struct InvalidInput {
    pub(crate) input: String,
    detail: String,
}

/// The native adapter attaches the operation and path to filesystem failures.
#[ohno::error]
#[display("Action could not {operation}: {}", path.display())]
pub(crate) struct ActionIo {
    operation: &'static str,
    path: PathBuf,
}

/// A process failure must not produce success-shaped action outputs.
#[ohno::error]
#[display("Action process {program} failed: {detail}")]
pub(crate) struct ProcessFailure {
    program: String,
    detail: String,
}

/// Dedicated machine output must be usable before any successful outputs are appended.
#[ohno::error]
#[display("Invalid action evidence: {detail}")]
pub(crate) struct InvalidOutput {
    detail: String,
}

// These errors retain immutable context and sources without unwind-sensitive state.
impl UnwindSafe for InvalidInput {}
impl RefUnwindSafe for InvalidInput {}
impl UnwindSafe for ActionIo {}
impl RefUnwindSafe for ActionIo {}
impl UnwindSafe for ProcessFailure {}
impl RefUnwindSafe for ProcessFailure {}
impl UnwindSafe for InvalidOutput {}
impl RefUnwindSafe for InvalidOutput {}
