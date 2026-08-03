use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

use ohno::OhnoCore;

/// An error from a storage operation.
///
/// Callers can distinguish only the conditions that drive storage control flow:
/// [`is_not_found`](Self::is_not_found) identifies an absent object for cache and
/// fallback decisions, while [`already_existing_key`](Self::already_existing_key)
/// identifies a write-once collision. All concrete failures remain available in
/// the error's source chain.
#[derive(ohno::Error)]
#[no_constructors]
pub struct StorageError {
    decision: StorageErrorDecision,

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
    /// Returns whether the requested object did not exist.
    ///
    /// This lets cache reads treat absence as a miss and cache synchronization treat
    /// an absent invalidation marker as the stable genesis epoch.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self.decision, StorageErrorDecision::NotFound)
    }

    /// Returns the key involved in a write-once collision, if any.
    ///
    /// This lets callers skip or report duplicate results and lets an explicit
    /// overwrite retry only after the write-once probe confirms the object exists.
    #[must_use]
    pub fn already_existing_key(&self) -> Option<&str> {
        match &self.decision {
            StorageErrorDecision::AlreadyExists { key } => Some(key),
            StorageErrorDecision::Other | StorageErrorDecision::NotFound => None,
        }
    }
}

/// The complete internal state needed for storage control-flow decisions.
#[derive(Debug)]
enum StorageErrorDecision {
    /// The failure does not drive a caller decision.
    Other,
    /// The requested object does not exist.
    NotFound,
    /// A write-once operation found an existing object.
    AlreadyExists { key: String },
}

/// A requested storage object does not exist.
#[ohno::error]
#[display("object not found: {key}")]
pub(crate) struct ObjectNotFoundError {
    pub(crate) key: String,
}

/// A write-once operation found an object already stored at its key.
#[ohno::error]
#[display("object already exists: {key}")]
pub(crate) struct ObjectAlreadyExistsError {
    pub(crate) key: String,
}

/// A storage key cannot safely identify an object within a backend.
#[ohno::error]
#[display("invalid storage key: {key}")]
pub(crate) struct InvalidStorageKeyError {
    pub(crate) key: String,
}

/// Storage configuration cannot produce a usable backend.
#[ohno::error]
#[display("{message}")]
pub(crate) struct StorageConfigurationError {
    pub(crate) message: String,
}

/// Creating the parent directories for a local object failed.
#[ohno::error]
#[display(
    "could not create local storage parent directories at {}",
    path.display()
)]
pub(crate) struct CreateLocalParentDirectoriesError {
    pub(crate) path: PathBuf,
}

/// Inspecting whether a local object already exists failed.
#[ohno::error]
#[display("could not inspect local storage object at {}", path.display())]
pub(crate) struct InspectLocalObjectExistenceError {
    pub(crate) path: PathBuf,
}

/// Atomically writing a local object failed.
#[ohno::error]
#[display("could not write local storage object at {}", path.display())]
pub(crate) struct WriteLocalObjectError {
    pub(crate) path: PathBuf,
}

/// Reading a local object's stored bytes failed.
#[ohno::error]
#[display("could not read local storage object at {}", path.display())]
pub(crate) struct ReadLocalObjectError {
    pub(crate) path: PathBuf,
}

/// Decompressing a local object's stored bytes failed.
#[ohno::error]
#[display(
    "could not decompress local storage object at {}",
    path.display()
)]
pub(crate) struct DecompressLocalObjectError {
    pub(crate) path: PathBuf,
}

/// Opening a local directory for a listing failed.
#[ohno::error]
#[display(
    "could not open local storage listing directory at {}",
    path.display()
)]
pub(crate) struct OpenLocalListingDirectoryError {
    pub(crate) path: PathBuf,
}

/// Advancing a local directory listing failed.
#[ohno::error]
#[display(
    "could not advance local storage listing directory at {}",
    path.display()
)]
pub(crate) struct AdvanceLocalListingDirectoryError {
    pub(crate) path: PathBuf,
}

/// Inspecting a local directory entry during a listing failed.
#[ohno::error]
#[display(
    "could not inspect local storage listing entry at {}",
    path.display()
)]
pub(crate) struct InspectLocalListingEntryError {
    pub(crate) path: PathBuf,
}

/// Removing a local object failed.
#[ohno::error]
#[display("could not remove local storage object at {}", path.display())]
pub(crate) struct RemoveLocalObjectError {
    pub(crate) path: PathBuf,
}

/// An Azure Blob operation failed.
#[ohno::error]
#[display("{operation}")]
pub(crate) struct AzureBlobOperationError {
    pub(crate) operation: String,
}

/// Decompressing an Azure blob's stored bytes failed.
#[ohno::error]
#[display("could not decompress Azure blob {key:?}")]
pub(crate) struct DecompressAzureBlobError {
    pub(crate) key: String,
}

impl From<ObjectNotFoundError> for StorageError {
    fn from(error: ObjectNotFoundError) -> Self {
        Self {
            decision: StorageErrorDecision::NotFound,
            core: OhnoCore::from(error),
        }
    }
}

impl From<ObjectAlreadyExistsError> for StorageError {
    fn from(error: ObjectAlreadyExistsError) -> Self {
        let key = error.key.clone();
        Self {
            decision: StorageErrorDecision::AlreadyExists { key },
            core: OhnoCore::from(error),
        }
    }
}

macro_rules! impl_other_storage_error {
    ($($source:ty),+ $(,)?) => {
        $(
            impl From<$source> for StorageError {
                fn from(error: $source) -> Self {
                    Self {
                        decision: StorageErrorDecision::Other,
                        core: OhnoCore::from(error),
                    }
                }
            }
        )+
    };
}

impl_other_storage_error!(
    InvalidStorageKeyError,
    StorageConfigurationError,
    CreateLocalParentDirectoriesError,
    InspectLocalObjectExistenceError,
    WriteLocalObjectError,
    ReadLocalObjectError,
    DecompressLocalObjectError,
    OpenLocalListingDirectoryError,
    AdvanceLocalListingDirectoryError,
    InspectLocalListingEntryError,
    RemoveLocalObjectError,
    AzureBlobOperationError,
    DecompressAzureBlobError,
);

/// A non-semantic storage failure used to exercise propagation in unit tests.
#[cfg(any(test, feature = "private-test-util"))]
#[doc(hidden)]
#[derive(Default, ohno::Error)]
#[no_constructors]
#[display("test storage failure")]
pub struct TestStorageError {
    #[error]
    core: OhnoCore,
}

#[cfg(any(test, feature = "private-test-util"))]
impl UnwindSafe for TestStorageError {}
#[cfg(any(test, feature = "private-test-util"))]
impl RefUnwindSafe for TestStorageError {}

#[cfg(any(test, feature = "private-test-util"))]
impl TestStorageError {
    /// Creates a non-semantic failure for propagation tests.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(any(test, feature = "private-test-util"))]
impl_other_storage_error!(TestStorageError);

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error;
    use std::fmt::Debug;
    use std::io;

    use ohno::ErrorExt as _;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(StorageError: Send, Sync, Debug, Error, UnwindSafe, RefUnwindSafe);

    #[test]
    fn not_found_decision_retains_the_leaf_and_cause() {
        let error: StorageError =
            ObjectNotFoundError::caused_by("v1/x".to_owned(), io::Error::other("missing")).into();

        assert!(error.is_not_found());
        assert_eq!(error.already_existing_key(), None);
        let leaf = error.find_source::<ObjectNotFoundError>().unwrap();
        assert_eq!(leaf.key, "v1/x");
        assert!(error.find_source::<io::Error>().is_some());
        assert!(matches!(error.decision, StorageErrorDecision::NotFound));
    }

    #[test]
    fn already_exists_decision_retains_the_key_in_both_places() {
        let error: StorageError = ObjectAlreadyExistsError::new("v1/dup".to_owned()).into();

        assert!(!error.is_not_found());
        assert_eq!(error.already_existing_key(), Some("v1/dup"));
        let leaf = error.find_source::<ObjectAlreadyExistsError>().unwrap();
        assert_eq!(leaf.key, "v1/dup");
        assert!(matches!(
            error.decision,
            StorageErrorDecision::AlreadyExists { ref key } if key == "v1/dup"
        ));
    }

    #[test]
    fn other_decision_retains_the_operation_leaf_and_cause() {
        let path = PathBuf::from("root/object");
        let error: StorageError =
            ReadLocalObjectError::caused_by(path.clone(), io::Error::other("unreadable")).into();

        assert!(!error.is_not_found());
        assert_eq!(error.already_existing_key(), None);
        let leaf = error.find_source::<ReadLocalObjectError>().unwrap();
        assert_eq!(leaf.path, path);
        assert!(error.find_source::<io::Error>().is_some());
        assert!(matches!(error.decision, StorageErrorDecision::Other));
    }

    #[test]
    fn invalid_key_is_an_other_decision_with_a_typed_leaf() {
        let error: StorageError = InvalidStorageKeyError::new("v1/../escape".to_owned()).into();

        assert!(matches!(error.decision, StorageErrorDecision::Other));
        let leaf = error.find_source::<InvalidStorageKeyError>().unwrap();
        assert_eq!(leaf.key, "v1/../escape");
    }

    #[test]
    fn configuration_is_an_other_decision_with_a_typed_leaf() {
        let error: StorageError =
            StorageConfigurationError::new("both backends selected".to_owned()).into();

        assert!(matches!(error.decision, StorageErrorDecision::Other));
        assert!(error.find_source::<StorageConfigurationError>().is_some());
    }
}
