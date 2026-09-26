// Failure conditions owned by this application's component. Leaves remain implementation details.
// The immutable ohno source chains permit shared observation across unwind boundaries.
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

use crp_diag::Quotable as _;
/// A file could not be read.
#[ohno::error]
#[display("Failed to read '{}'", path.quoted())]
pub(crate) struct ReadFileError {
    path: PathBuf,
}

impl UnwindSafe for ReadFileError {}
impl RefUnwindSafe for ReadFileError {}

/// A file could not be written.
#[ohno::error]
#[display("Failed to write '{}'", path.quoted())]
pub(crate) struct WriteFileError {
    path: PathBuf,
}

impl UnwindSafe for WriteFileError {}
impl RefUnwindSafe for WriteFileError {}

/// A TOML document is not valid.
#[ohno::error]
#[display("Failed to parse '{}'", path.quoted())]
pub(crate) struct ParseTomlError {
    path: PathBuf,
}

impl UnwindSafe for ParseTomlError {}
impl RefUnwindSafe for ParseTomlError {}

/// `cargo metadata` JSON is not valid.
#[ohno::error]
#[display("Failed to parse cargo metadata JSON")]
pub(crate) struct ParseMetadataError;

impl UnwindSafe for ParseMetadataError {}
impl RefUnwindSafe for ParseMetadataError {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::Debug;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::path::Path;
    use std::{error, io};

    use static_assertions::assert_impl_all;

    use super::*;
    assert_impl_all!(ReadFileError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(WriteFileError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ParseTomlError: Send, Sync, Debug, error::Error, UnwindSafe, RefUnwindSafe);

    /// The same protection applies to the paths the file-access errors name.
    #[test]
    fn a_repository_controlled_path_is_escaped_in_the_message() {
        let error = ReadFileError::caused_by(Path::new("a\nb"), io::Error::other("x"));
        let message = error.to_string();
        let first_line = message.lines().next().unwrap();
        assert!(first_line.contains(r"a\nb"), "{message}");
    }
}
