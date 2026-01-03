// Filesystem trait abstraction for mocking in tests.
//
// Higher-level operations are preferred over raw fs operations. If finer-grained mocking becomes
// necessary, we can split these into more atomic operations later.

use std::fmt::Debug;
use std::io;
use std::path::{Path, PathBuf};

/// Abstraction over filesystem operations used by cargo-detect-package.
///
/// This trait is automatically mocked by mockall in test builds, generating `MockFilesystem`.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Filesystem: Debug + Send + Sync + 'static {
    /// Returns `true` if a `Cargo.toml` file exists in the given directory.
    fn cargo_toml_exists(&self, dir: &Path) -> bool;

    /// Reads the contents of `Cargo.toml` in the given directory.
    ///
    /// Returns an error if the file does not exist or cannot be read.
    fn read_cargo_toml(&self, dir: &Path) -> io::Result<String>;

    /// Returns `true` if the given path is a file (not a directory).
    fn is_file(&self, path: &Path) -> bool;

    /// Returns the canonical, absolute form of a path.
    ///
    /// This resolves symbolic links and normalizes the path.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;

    /// Returns the current working directory.
    fn current_dir(&self) -> io::Result<PathBuf>;

    /// Returns `true` if the given path exists (file or directory).
    fn exists(&self, path: &Path) -> bool;
}
