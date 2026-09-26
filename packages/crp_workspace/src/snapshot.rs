use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use ohno::AppError;

use crate::ReadFileError;
use crate::snapshot_command::{capture, git};

/// Acquires repository facts for a caller's fixed source verification.
#[derive(Debug)]
pub struct SourceSnapshot {
    root: PathBuf,
}

impl SourceSnapshot {
    // Filesystem and repository discovery are covered by the boundary integration suite.
    #[cfg_attr(test, mutants::skip)]
    pub fn discover(manifest: &Path) -> Result<Self, AppError> {
        let manifest = manifest
            .canonicalize()
            .map_err(|error| ReadFileError::caused_by(manifest, error))?;
        let directory = manifest.parent().ok_or_else(|| {
            SnapshotLocationError::new("candidate manifest must have a parent directory")
        })?;
        let root = git(["rev-parse", "--show-toplevel"], directory)?;
        let root = Path::new(root.trim_end_matches(['\r', '\n']));
        let root = root
            .canonicalize()
            .map_err(|error| ReadFileError::caused_by(root, error))?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[cfg_attr(test, mutants::skip)] // Captures native Git status, with policy owned by the caller.
    pub fn status(&self) -> Result<Vec<u8>, AppError> {
        capture(
            "git",
            [
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
            &self.root,
        )
    }

    #[cfg_attr(test, mutants::skip)] // Captures index flags without interpreting source eligibility.
    pub fn index(&self) -> Result<Vec<u8>, AppError> {
        capture("git", ["ls-files", "-v", "-z"], &self.root)
    }

    #[cfg_attr(test, mutants::skip)] // Resolves source identity using the caller's installed Git.
    pub fn head(&self) -> Result<String, AppError> {
        git(["rev-parse", "--verify", "HEAD"], &self.root)
    }

    #[cfg_attr(test, mutants::skip)] // Git owns revision resolution, not the calling policy.
    pub fn resolve(&self, commit: &str) -> Result<String, AppError> {
        git(
            [
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{commit}^{{commit}}"),
            ],
            &self.root,
        )
    }

    #[cfg_attr(test, mutants::skip)] // Membership requirements belong to the caller of this walk.
    pub fn first_parent(&self, revision: &str) -> Result<String, AppError> {
        git(["rev-list", "--first-parent", revision, "--"], &self.root)
    }

    #[cfg_attr(test, mutants::skip)] // The caller has resolved and checked path containment.
    pub fn tracked(&self, relative: &Path) -> Result<(), AppError> {
        git(
            [
                OsStr::new("--literal-pathspecs"),
                OsStr::new("ls-files"),
                OsStr::new("--error-unmatch"),
                OsStr::new("--"),
                relative.as_os_str(),
            ],
            &self.root,
        )?;
        Ok(())
    }
}

/// A source location has no containing directory for repository discovery.
#[ohno::error]
#[display("{reason}")]
struct SnapshotLocationError {
    reason: String,
}
