//! PAL failure kinds.
//!
//! Logic maps these into semantic `ohno` leaves. Tests match on `kind`, not
//! messages.

use std::error::Error;
use std::{fmt, io};

/// Failure produced by a PAL operation.
#[derive(Debug)]
pub(crate) struct PalError {
    kind: PalErrorKind,
    source: Option<io::Error>,
}

/// Distinguishes PAL failures that logic handles differently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum PalErrorKind {
    /// A bounded connect wait elapsed.
    Timeout,
    /// Job breakaway was denied.
    BreakawayDenied,
    /// Opening or querying a process handle failed.
    InspectFailed,
    /// The requested object does not exist.
    NotFound,
    /// The peer closed the connection.
    Disconnected,
    /// Any other platform failure.
    Other,
}

impl PalError {
    pub(crate) fn new(kind: PalErrorKind) -> Self {
        Self { kind, source: None }
    }

    pub(crate) fn with_source(kind: PalErrorKind, source: io::Error) -> Self {
        Self {
            kind,
            source: Some(source),
        }
    }

    pub(crate) fn kind(&self) -> PalErrorKind {
        self.kind
    }

    pub(crate) fn from_io(error: io::Error) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::UnexpectedEof => PalErrorKind::Disconnected,
            _ => PalErrorKind::Other,
        };
        Self::with_source(kind, error)
    }
}

// Error text is not an API contract.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl fmt::Display for PalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.kind {
            PalErrorKind::Timeout => "timed out",
            PalErrorKind::BreakawayDenied => "breakaway denied",
            PalErrorKind::InspectFailed => "failed to inspect the process",
            PalErrorKind::NotFound => "not found",
            PalErrorKind::Disconnected => "disconnected",
            PalErrorKind::Other => "platform error",
        };
        f.write_str(label)
    }
}

// Source chaining is not an API contract.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl Error for PalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self.source.as_ref() {
            Some(error) => Some(error),
            None => None,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn from_io_maps_broken_pipe_to_disconnected() {
        let error = PalError::from_io(io::Error::new(io::ErrorKind::BrokenPipe, "closed"));
        assert_eq!(error.kind(), PalErrorKind::Disconnected);
    }

    #[test]
    fn from_io_maps_other_kinds_to_other() {
        let error = PalError::from_io(io::Error::other("platform"));
        assert_eq!(error.kind(), PalErrorKind::Other);
    }
}
