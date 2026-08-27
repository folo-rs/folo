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
    BreakawayDenied,
    /// Opening or querying a process handle failed.
    InspectFailed,
    /// The requested object does not exist.
    NotFound,
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
        Self::with_source(PalErrorKind::Other, error)
    }
}

impl fmt::Display for PalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.kind {
            PalErrorKind::Timeout => "timed out",
            PalErrorKind::BreakawayDenied => "breakaway denied",
            PalErrorKind::InspectFailed => "process inspect failed",
            PalErrorKind::NotFound => "not found",
            PalErrorKind::Other => "platform error",
        };
        f.write_str(label)
    }
}

impl std::error::Error for PalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self.source.as_ref() {
            Some(error) => Some(error),
            None => None,
        }
    }
}
