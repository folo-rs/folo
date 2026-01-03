// Real filesystem implementation that delegates to std::fs and std::env.
//
// This is a trivial forwarder to system APIs and is excluded from coverage and mutation testing.

use std::io;
use std::path::{Path, PathBuf};

use crate::pal::Filesystem;

/// Real filesystem implementation that uses the operating system's filesystem.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetFilesystem;

// Trivial forwarder to system APIs - not worth testing.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl Filesystem for BuildTargetFilesystem {
    fn cargo_toml_exists(&self, dir: &Path) -> bool {
        dir.join("Cargo.toml").exists()
    }

    fn read_cargo_toml(&self, dir: &Path) -> io::Result<String> {
        std::fs::read_to_string(dir.join("Cargo.toml"))
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        path.canonicalize()
    }

    fn current_dir(&self) -> io::Result<PathBuf> {
        std::env::current_dir()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}
