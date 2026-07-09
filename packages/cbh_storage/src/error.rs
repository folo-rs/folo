use std::error::Error;
use std::{fmt, io};

/// An error from a storage operation.
#[derive(Debug)]
pub enum StorageError {
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
    /// An underlying I/O error occurred.
    Io(io::Error),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { key } => write!(f, "object not found: {key}"),
            Self::InvalidKey { key } => write!(f, "invalid storage key: {key}"),
            Self::AlreadyExists { key } => write!(f, "object already exists: {key}"),
            Self::Config { message } => write!(f, "storage configuration error: {message}"),
            Self::Io(error) => write!(f, "storage I/O error: {error}"),
        }
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NotFound { .. }
            | Self::InvalidKey { .. }
            | Self::AlreadyExists { .. }
            | Self::Config { .. } => None,
            Self::Io(error) => Some(error),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn not_found_display_includes_key() {
        let error = StorageError::NotFound {
            key: "v1/x".to_owned(),
        };
        assert_eq!(error.to_string(), "object not found: v1/x");
    }

    #[test]
    fn io_display_and_source() {
        let error = StorageError::Io(io::Error::other("disk gone"));
        assert!(error.to_string().contains("disk gone"));
        assert!(error.source().is_some());
    }

    #[test]
    fn not_found_has_no_source() {
        let error = StorageError::NotFound {
            key: "k".to_owned(),
        };
        assert!(error.source().is_none());
    }

    #[test]
    fn invalid_key_display_and_no_source() {
        let error = StorageError::InvalidKey {
            key: "v1/../escape".to_owned(),
        };
        assert!(error.to_string().contains("v1/../escape"), "{error}");
        assert!(error.source().is_none());
    }

    #[test]
    fn already_exists_display_and_no_source() {
        let error = StorageError::AlreadyExists {
            key: "v1/dup".to_owned(),
        };
        assert!(error.to_string().contains("v1/dup"), "{error}");
        assert!(error.source().is_none());
    }

    #[test]
    fn config_display_and_no_source() {
        let error = StorageError::Config {
            message: "both keys set".to_owned(),
        };
        assert!(error.to_string().contains("both keys set"), "{error}");
        assert!(error.to_string().contains("configuration"), "{error}");
        assert!(error.source().is_none());
    }
}
