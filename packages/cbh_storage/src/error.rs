use std::panic::{RefUnwindSafe, UnwindSafe};
use std::{fmt, io};

use ohno::OhnoCore;

/// An error from a storage operation.
///
/// The [`kind`](Self::kind) carries the category (and its key or message), which callers
/// branch on to drive fallbacks such as treating a missing object as a cache miss or
/// treating an occupied key as "already written".
#[derive(ohno::Error)]
#[no_constructors]
#[display("{kind}")]
pub struct StorageError {
    kind: StorageErrorKind,

    #[error]
    core: OhnoCore,
}

// The OhnoCore field holds an Arc<dyn Error + Send + Sync>, which is !UnwindSafe because Arc
// requires T: RefUnwindSafe and trait objects are !RefUnwindSafe. However, ohno error types are
// immutable after construction — no &self method mutates internal state — so observing them
// through a shared reference during unwind is harmless.
impl UnwindSafe for StorageError {}
impl RefUnwindSafe for StorageError {}

impl StorageError {
    /// No object exists at `key`.
    #[must_use]
    pub fn not_found(key: impl Into<String>) -> Self {
        Self::of_kind(StorageErrorKind::NotFound { key: key.into() })
    }

    /// `key` is not a well-formed storage key.
    #[must_use]
    pub fn invalid_key(key: impl Into<String>) -> Self {
        Self::of_kind(StorageErrorKind::InvalidKey { key: key.into() })
    }

    /// An object is already stored at `key`.
    #[must_use]
    pub fn already_exists(key: impl Into<String>) -> Self {
        Self::of_kind(StorageErrorKind::AlreadyExists { key: key.into() })
    }

    /// The storage backend is misconfigured, as described by `message`.
    #[must_use]
    pub fn config(message: impl Into<String>) -> Self {
        Self::of_kind(StorageErrorKind::Config {
            message: message.into(),
        })
    }

    /// An I/O operation failed; `error` becomes the source of the returned error.
    #[must_use]
    pub fn io(error: io::Error) -> Self {
        Self {
            kind: StorageErrorKind::Io,
            core: OhnoCore::from(error),
        }
    }

    /// The category of the failure, carrying the key or message it concerns.
    #[must_use]
    pub fn kind(&self) -> &StorageErrorKind {
        &self.kind
    }

    fn of_kind(kind: StorageErrorKind) -> Self {
        Self {
            kind,
            core: OhnoCore::default(),
        }
    }
}

/// The category of a [`StorageError`].
#[derive(Debug)]
pub enum StorageErrorKind {
    /// No object exists at the requested key.
    NotFound {
        /// The key that was not found.
        key: String,
    },
    /// The key was not a valid storage key (it contained an empty, `.`, or `..`
    /// segment, or a platform-absolute segment, that could escape the storage
    /// root).
    InvalidKey {
        /// The rejected key.
        key: String,
    },
    /// An object already exists at the requested key. Storage is write-once, so
    /// an existing object is never overwritten.
    AlreadyExists {
        /// The key that was already occupied.
        key: String,
    },
    /// The storage backend is misconfigured (e.g. an Azure endpoint that is not a
    /// valid HTTPS URL).
    Config {
        /// Human-readable description of the misconfiguration.
        message: String,
    },
    /// An underlying I/O error occurred. The offending [`io::Error`] is the
    /// [`source`](std::error::Error::source) of the [`StorageError`], so the
    /// kind itself carries no data.
    Io,
}

impl fmt::Display for StorageErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { key } => write!(f, "object not found: {key}"),
            Self::InvalidKey { key } => write!(f, "invalid storage key: {key}"),
            Self::AlreadyExists { key } => write!(f, "object already exists: {key}"),
            Self::Config { message } => write!(f, "storage configuration error: {message}"),
            Self::Io => write!(f, "storage I/O error"),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error;
    use std::fmt::Debug;

    use ohno::ErrorExt as _;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(StorageError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(StorageErrorKind: Send, Sync, Debug, UnwindSafe, RefUnwindSafe);

    #[test]
    fn not_found_display_includes_key() {
        let error = StorageError::not_found("v1/x");

        assert!(error.message().contains("v1/x"));
        assert!(matches!(error.kind(), StorageErrorKind::NotFound { key } if key == "v1/x"));
    }

    #[test]
    fn io_display_and_source() {
        let error = StorageError::io(io::Error::other("disk gone"));

        assert!(error.message().contains("disk gone"));
        assert!(matches!(error.kind(), StorageErrorKind::Io));
        assert!(error.source().is_some());
    }

    #[test]
    fn not_found_has_no_source() {
        let error = StorageError::not_found("k");

        assert!(error.source().is_none());
    }

    #[test]
    fn invalid_key_display_and_no_source() {
        let error = StorageError::invalid_key("v1/../escape");

        assert!(error.message().contains("v1/../escape"));
        assert!(matches!(error.kind(), StorageErrorKind::InvalidKey { .. }));
        assert!(error.source().is_none());
    }

    #[test]
    fn already_exists_display_and_no_source() {
        let error = StorageError::already_exists("v1/dup");

        assert!(error.message().contains("v1/dup"));
        assert!(matches!(error.kind(), StorageErrorKind::AlreadyExists { key } if key == "v1/dup"));
        assert!(error.source().is_none());
    }

    #[test]
    fn config_display_and_no_source() {
        let error = StorageError::config("both keys set");

        assert!(error.message().contains("both keys set"));
        assert!(error.message().contains("configuration"));
        assert!(matches!(error.kind(), StorageErrorKind::Config { .. }));
        assert!(error.source().is_none());
    }
}
