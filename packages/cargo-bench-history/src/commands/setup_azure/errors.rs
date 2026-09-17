use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

/// A caller-supplied setup value does not satisfy the deployment contract.
#[ohno::error]
#[display("invalid Azure setup parameter {parameter}: {reason}")]
pub(crate) struct SetupParameterError {
    pub(crate) parameter: &'static str,
    pub(crate) reason: &'static str,
}

/// A bundle filesystem operation failed, with its original I/O cause retained.
#[ohno::error]
#[display("failed to {operation} Azure deployment bundle at {}", path.display())]
pub(crate) struct SetupBundleError {
    operation: &'static str,
    path: PathBuf,
}

/// The OS could not allocate the directory owned by this setup execution.
#[ohno::error]
#[display("could not create an owned temporary Azure deployment directory")]
pub(crate) struct SetupTemporaryDirectoryError;

/// A deployment tool could not be started or exited unsuccessfully.
#[ohno::error]
#[display(
    "Azure setup {operation} failed; argv: {arguments:?}; exit code: {code:?}\nstdout:\n{stdout}\nstderr:\n{stderr}\nAny completed Azure changes remain in place."
)]
pub(crate) struct SetupProcessError {
    operation: &'static str,
    arguments: Vec<OsString>,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Cleanup failed; an earlier deployment failure remains attached as its source.
#[ohno::error]
#[display("could not remove owned Azure deployment directory {}: {cleanup}", path.display())]
pub(crate) struct SetupCleanupError {
    path: PathBuf,
    cleanup: io::Error,
}

/// Serializing literal parameters into the embedded template failed.
#[ohno::error]
#[display("could not prepare Azure deployment parameters")]
pub(crate) struct SetupParametersEncodingError;
