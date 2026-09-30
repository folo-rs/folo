//! Filesystem observations for harvesting, without engine selection policy.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio::fs::{DirEntry, ReadDir};

/// Supplies native observations to the engine collectors.
///
/// Directory iteration and metadata acquisition remain lazy so irrelevant entries
/// are not read and errors retain their native operation order.
/// See docs/implementation.md for the harvesting boundary.
pub(crate) trait OutputFiles {
    type Directory: OutputDirectory;

    fn read_dir(&self, path: &Path) -> impl Future<Output = io::Result<Self::Directory>>;

    fn read_to_string(&self, path: &Path) -> impl Future<Output = io::Result<String>>;
}

/// Advances a directory scan without interpreting missing paths or selecting entries.
pub(crate) trait OutputDirectory {
    type Entry: OutputEntry;

    fn next_entry(&mut self) -> impl Future<Output = io::Result<Option<Self::Entry>>>;
}

/// Exposes entry observations without following links or reading unselected contents.
pub(crate) trait OutputEntry {
    fn path(&self) -> PathBuf;

    fn file_type(&self) -> impl Future<Output = io::Result<EntryType>>;

    fn modified(&self) -> impl Future<Output = io::Result<SystemTime>>;
}

/// The native file-type distinctions relevant to traversal and flat-file selection.
///
/// Symlinks and other non-regular entries are represented by `Other`. Recursive
/// collectors and flat collectors deliberately apply different selection policies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntryType {
    Directory,
    File,
    Other,
}

/// Tokio-backed acquisition; all selection and error interpretation belongs to the collectors.
pub(crate) struct TokioOutputFiles;

impl OutputFiles for TokioOutputFiles {
    type Directory = ReadDir;

    // Native directory acquisition is covered by tests/bench_output.rs.
    #[cfg_attr(test, mutants::skip)]
    async fn read_dir(&self, path: &Path) -> io::Result<ReadDir> {
        tokio::fs::read_dir(path).await
    }

    // Native contents/UTF-8 acquisition is covered by tests/bench_output.rs.
    #[cfg_attr(test, mutants::skip)]
    async fn read_to_string(&self, path: &Path) -> io::Result<String> {
        tokio::fs::read_to_string(path).await
    }
}

impl OutputDirectory for ReadDir {
    type Entry = DirEntry;

    // Native cursor advancement is covered by recursive and flat integration fixtures.
    #[cfg_attr(test, mutants::skip)]
    async fn next_entry(&mut self) -> io::Result<Option<DirEntry>> {
        self.next_entry().await
    }
}

impl OutputEntry for DirEntry {
    // Native entry identity is covered by the integration harvests' paths and contents.
    #[cfg_attr(test, mutants::skip)]
    fn path(&self) -> PathBuf {
        self.path()
    }

    // Native type flags are integration-tested; collectors interpret these facts in unit tests.
    #[cfg_attr(test, mutants::skip)]
    async fn file_type(&self) -> io::Result<EntryType> {
        let file_type = self.file_type().await?;
        Ok(if file_type.is_dir() {
            EntryType::Directory
        } else if file_type.is_file() {
            EntryType::File
        } else {
            EntryType::Other
        })
    }

    // Native entry metadata and timestamp support require filesystem integration.
    // Fixed-mtime integration fixtures exercise this; freshness decisions remain unit-tested.
    #[cfg_attr(test, mutants::skip)]
    async fn modified(&self) -> io::Result<SystemTime> {
        self.metadata().await?.modified()
    }
}
