use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crp_diag::Quotable as _;
use ohno::AppError;

use crate::ReadFileError;
use crate::snapshot_command::{capture, git};

/// Acquires repository facts for a caller's fixed source verification.
///
/// Each operation performs a live query. The caller binds observations to its source
/// identity and verifies immutability around the acquisition window.
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

    /// Verifies exact repository-relative files with one live index query.
    ///
    /// The caller resolves containment and verifies source immutability around this phase.
    /// Paths must not be absolute or contain parent components; duplicates are harmless.
    /// Index membership does not establish worktree presence or publication eligibility.
    #[cfg_attr(test, mutants::skip)] // The index query is covered by real Git boundary tests.
    pub fn tracked_paths(&self, relatives: &[PathBuf]) -> Result<(), AppError> {
        if relatives.is_empty() {
            return Ok(());
        }
        // Reading the index once avoids both per-file processes and native argv size limits.
        let index = capture("git", ["ls-files", "-z"], &self.root)?;
        require_tracked_paths(&index, relatives)
    }
}

fn require_tracked_paths(index: &[u8], relatives: &[PathBuf]) -> Result<(), AppError> {
    if !index.is_empty() && !index.ends_with(b"\0") {
        return Err(MalformedTrackedIndex::new().into());
    }
    let tracked: BTreeSet<_> = index
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .collect();
    let mut checked = BTreeSet::new();
    for path in relatives {
        let key = tracked_path_key(path)?;
        if checked.contains(&key) {
            continue;
        }
        if !tracked.contains(key.as_slice()) {
            return Err(UntrackedSourcePath::new(path).into());
        }
        _ = checked.insert(key);
    }
    Ok(())
}

fn tracked_path_key(path: &Path) -> Result<Vec<u8>, AppError> {
    let mut key = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                if !key.is_empty() {
                    key.push(b'/');
                }
                key.extend_from_slice(part.as_encoded_bytes());
            }
            Component::CurDir => {}
            _ => return Err(InvalidTrackedSourcePath::new(path).into()),
        }
    }
    if key.is_empty() {
        return Err(InvalidTrackedSourcePath::new(path).into());
    }
    Ok(key)
}

/// One of the caller's required source files is absent from the observed index.
#[ohno::error]
#[display("required source path is not tracked: '{}'", path.quoted())]
struct UntrackedSourcePath {
    path: PathBuf,
}

/// Batch membership checks accept normalized file paths within the repository.
#[ohno::error]
#[display("tracked source path must be a nonempty repository-relative file: '{}'", path.quoted())]
struct InvalidTrackedSourcePath {
    path: PathBuf,
}

/// Truncated index output cannot establish complete membership.
#[ohno::error]
#[display("Git supplied an incomplete NUL-delimited tracked-file index")]
struct MalformedTrackedIndex;

/// A source location has no containing directory for repository discovery.
#[ohno::error]
#[display("{reason}")]
struct SnapshotLocationError {
    reason: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn tracked_membership_is_literal_complete_and_duplicate_tolerant() {
        let paths = ["Cargo.toml", "package/file[one].toml", "Cargo.toml"].map(PathBuf::from);
        require_tracked_paths(b"Cargo.toml\0package/file[one].toml\0", &paths).unwrap();
        let error = require_tracked_paths(b"Cargo.toml\0", &paths).unwrap_err();
        assert!(error.find_source::<UntrackedSourcePath>().is_some());
        let error = require_tracked_paths(
            b"package/file-one.toml\0",
            &[PathBuf::from("package/file*.toml")],
        )
        .unwrap_err();
        assert!(error.find_source::<UntrackedSourcePath>().is_some());
        require_tracked_paths(b"", &[]).unwrap();
    }

    #[test]
    fn tracked_membership_rejects_invalid_paths_and_truncated_output() {
        for path in ["", ".", "../Cargo.toml", "package/../Cargo.toml"] {
            let error = require_tracked_paths(b"Cargo.toml\0", &[PathBuf::from(path)]).unwrap_err();
            assert!(error.find_source::<InvalidTrackedSourcePath>().is_some());
        }
        let error =
            require_tracked_paths(b"Cargo.toml", &[PathBuf::from("Cargo.toml")]).unwrap_err();
        assert!(error.find_source::<MalformedTrackedIndex>().is_some());
    }
}
