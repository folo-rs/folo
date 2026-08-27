//! PAL failure kinds.
//!
//! Logic maps these into semantic `ohno` leaves. Tests match on `kind`, not
//! messages.

use std::fmt;

/// Failure produced by a PAL operation.
#[derive(Debug)]
pub(crate) struct PalError {
    kind: PalErrorKind,
    source: Option<std::io::Error>,
}

/// Distinguishes PAL failures that logic handles differently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum PalErrorKind {
    /// A bounded connect wait elapsed.
    Timeout,
    /// Job breakaway was denied.
    #[cfg_attr(
        not(any(windows, test)),
        expect(dead_code, reason = "produced by the Windows process PAL and tests")
    )]
    BreakawayDenied,
    /// Opening or querying a process handle failed.
    #[cfg_attr(
        not(windows),
        expect(dead_code, reason = "produced by the Windows process PAL")
    )]
    InspectFailed,
    /// The requested object does not exist.
    #[cfg_attr(
        not(any(windows, test)),
        expect(dead_code, reason = "produced by the Windows PAL and tests")
    )]
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

    pub(crate) fn with_source(kind: PalErrorKind, source: std::io::Error) -> Self {
        Self {
            kind,
            source: Some(source),
        }
    }

    pub(crate) fn kind(&self) -> PalErrorKind {
        self.kind
    }

    pub(crate) fn from_io(error: std::io::Error) -> Self {
        let kind = match error.kind() {
            std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::UnexpectedEof => PalErrorKind::Disconnected,
            _ => PalErrorKind::Other,
        };
        Self::with_source(kind, error)
    }
}

// Error text is not an API contract.
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
#[cfg_attr(test, mutants::skip)]
impl std::error::Error for PalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self.source.as_ref() {
            Some(error) => Some(error),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_io_maps_broken_pipe_to_disconnected() {
        let error = PalError::from_io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "closed",
        ));
        assert_eq!(error.kind(), PalErrorKind::Disconnected);
    }

    #[test]
    fn from_io_maps_other_kinds_to_other() {
        let error = PalError::from_io(std::io::Error::other("platform"));
        assert_eq!(error.kind(), PalErrorKind::Other);
    }
}
