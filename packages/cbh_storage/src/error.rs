use std::error::Error;
use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::OhnoCore;

/// An error from a storage operation.
///
/// Callers may distinguish a missing object and retrieve the key of an
/// already-existing object. Every other condition remains an implementation detail
/// represented by the diagnostic and source chain.
#[derive(ohno::Error)]
#[no_constructors]
pub struct StorageError {
    decision: StorageDecision,

    #[error]
    core: OhnoCore,
}

/// The complete internal decision state carried by [`StorageError`].
#[derive(Debug)]
enum StorageDecision {
    Other,
    NotFound,
    AlreadyExists { key: String },
}

/// No object exists at the requested storage key.
#[ohno::error]
#[display("object not found: {key}")]
pub(crate) struct ObjectNotFoundError {
    key: String,
}

/// An object already exists at a write-once storage key.
#[ohno::error]
#[display("object already exists: {key}")]
pub(crate) struct ObjectAlreadyExistsError {
    key: String,
}

/// A storage key is not a sequence of ordinary relative path segments.
#[ohno::error]
#[display("invalid storage key: {key}")]
pub(crate) struct InvalidStorageKeyError {
    key: String,
}

/// Storage configuration is absent or invalid.
#[ohno::error]
#[display("{message}")]
pub(crate) struct StorageConfigurationError {
    message: String,
}

impl StorageError {
    /// Whether the requested object does not exist.
    ///
    /// Cache implementations use this result to select their cache-miss path.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self.decision, StorageDecision::NotFound)
    }

    /// The occupied key when a write-once operation found an existing object.
    ///
    /// Collection uses the key to report or skip duplicate result objects.
    #[must_use]
    pub fn already_existing_key(&self) -> Option<&str> {
        match &self.decision {
            StorageDecision::AlreadyExists { key } => Some(key),
            StorageDecision::Other | StorageDecision::NotFound => None,
        }
    }

    pub(crate) fn other(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            decision: StorageDecision::Other,
            core: OhnoCore::from(error),
        }
    }
}

impl From<ObjectNotFoundError> for StorageError {
    fn from(error: ObjectNotFoundError) -> Self {
        Self {
            decision: StorageDecision::NotFound,
            core: OhnoCore::from(error),
        }
    }
}

impl From<ObjectAlreadyExistsError> for StorageError {
    fn from(error: ObjectAlreadyExistsError) -> Self {
        let key = error.key.clone();
        Self {
            decision: StorageDecision::AlreadyExists { key },
            core: OhnoCore::from(error),
        }
    }
}

impl From<InvalidStorageKeyError> for StorageError {
    fn from(error: InvalidStorageKeyError) -> Self {
        Self::other(error)
    }
}

impl From<StorageConfigurationError> for StorageError {
    fn from(error: StorageConfigurationError) -> Self {
        Self::other(error)
    }
}

// Every error type in this file is immutable after construction, so observing it
// through a shared reference while unwinding is harmless.
impl UnwindSafe for StorageError {}
impl RefUnwindSafe for StorageError {}
impl UnwindSafe for ObjectNotFoundError {}
impl RefUnwindSafe for ObjectNotFoundError {}
impl UnwindSafe for ObjectAlreadyExistsError {}
impl RefUnwindSafe for ObjectAlreadyExistsError {}
impl UnwindSafe for InvalidStorageKeyError {}
impl RefUnwindSafe for InvalidStorageKeyError {}
impl UnwindSafe for StorageConfigurationError {}
impl RefUnwindSafe for StorageConfigurationError {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::Debug;
    use std::io;

    use ohno::ErrorExt as _;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(StorageError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);

    #[test]
    fn not_found_exposes_only_the_required_decision() {
        let error = StorageError::from(ObjectNotFoundError::new("v1/x"));

        assert!(error.is_not_found());
        assert_eq!(error.already_existing_key(), None);
    }

    #[test]
    fn already_exists_retains_the_decision_key() {
        let error = StorageError::from(ObjectAlreadyExistsError::new("v1/dup"));

        assert!(!error.is_not_found());
        assert_eq!(error.already_existing_key(), Some("v1/dup"));
    }

    #[test]
    fn private_leaf_sources_preserve_exact_mappings() {
        let error = StorageError::from(ObjectNotFoundError::caused_by(
            "v1/x",
            io::Error::other("disk gone"),
        ));

        assert!(error.find_source::<ObjectNotFoundError>().is_some());
        assert!(error.find_source::<io::Error>().is_some());
    }

    #[test]
    fn other_conditions_expose_no_decision() {
        let error = StorageError::from(InvalidStorageKeyError::new("v1/../escape"));

        assert!(!error.is_not_found());
        assert_eq!(error.already_existing_key(), None);
        assert!(error.find_source::<InvalidStorageKeyError>().is_some());
    }
}
