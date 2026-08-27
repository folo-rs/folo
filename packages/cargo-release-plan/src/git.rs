// Git access via `git` subprocesses.
//
// The design forbids git2/gix; every read of history, trees, and diffs goes
// through this type.

use std::path::{Path, PathBuf};

use ohno::AppError;

use crate::UnresolvedBaseError;
use crate::command::{run_capture, run_capture_ok, run_capture_ok_bytes};

/// A Git repository rooted at `root`.
#[derive(Clone, Debug)]
pub(crate) struct GitRepo {
    root: PathBuf,
}

impl GitRepo {
    /// Discovers the repository containing `start` using `git rev-parse`.
    pub(crate) fn discover(start: &Path) -> Result<Self, AppError> {
        let dir = if start.is_file() {
            start.parent().unwrap_or(start)
        } else {
            start
        };
        let root = run_capture("git", &["rev-parse", "--show-toplevel"], dir)?;
        Ok(Self {
            root: PathBuf::from(root.trim()),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn rev_parse(&self, rev: &str) -> Result<String, AppError> {
        match run_capture("git", &["rev-parse", "--verify", rev], &self.root) {
            Ok(stdout) => Ok(stdout.trim().to_string()),
            Err(error) => Err(UnresolvedBaseError::caused_by(rev, error).into()),
        }
    }

    // HEAD is only used as a report label; tests do not pin the exact SHA string.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn head(&self) -> Result<String, AppError> {
        Ok(run_capture("git", &["rev-parse", "HEAD"], &self.root)?
            .trim()
            .to_string())
    }

    /// First-parent commits reachable from `rev`, newest first, as full hashes.
    pub(crate) fn first_parent_commits(&self, rev: &str) -> Result<Vec<String>, AppError> {
        let stdout = run_capture("git", &["rev-list", "--first-parent", rev], &self.root)?;
        Ok(stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }

    /// Whether `commit` has a first parent that Git can resolve.
    ///
    /// A shallow-boundary commit has a parent that was not fetched. Git cannot
    /// resolve `commit^`, but that is truncated history, not a true root.
    pub(crate) fn has_resolvable_parent(&self, commit: &str) -> Result<bool, AppError> {
        let spec = format!("{commit}^");
        if run_capture_ok("git", &["rev-parse", "--verify", &spec], &self.root)?.is_some() {
            return Ok(true);
        }
        self.is_shallow()
    }

    fn is_shallow(&self) -> Result<bool, AppError> {
        let stdout = run_capture("git", &["rev-parse", "--is-shallow-repository"], &self.root)?;
        Ok(stdout.trim() == "true")
    }

    /// File contents at `commit:rel_path`, or `None` if the path is absent.
    pub(crate) fn show_file(
        &self,
        commit: &str,
        rel_path: &str,
    ) -> Result<Option<String>, AppError> {
        match self.show_file_bytes(commit, rel_path)? {
            Some(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            None => Ok(None),
        }
    }

    /// Raw bytes at `commit:rel_path`, or `None` if the path is absent.
    pub(crate) fn show_file_bytes(
        &self,
        commit: &str,
        rel_path: &str,
    ) -> Result<Option<Vec<u8>>, AppError> {
        let spec = format!("{}:{}", commit, git_path(rel_path));
        run_capture_ok_bytes("git", &["show", &spec], &self.root)
    }

    /// Git-tracked paths under `pathspec` in the work tree / index.
    pub(crate) fn ls_files(&self, pathspec: &str) -> Result<Vec<String>, AppError> {
        let stdout = run_capture(
            "git",
            &["ls-files", "-z", "--", &git_path(pathspec)],
            &self.root,
        )?;
        Ok(split_z(&stdout))
    }

    /// Untracked, non-ignored paths under `pathspec`.
    // Advisory-only listing; classification does not fail on untracked files.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn ls_untracked(&self, pathspec: &str) -> Result<Vec<String>, AppError> {
        let stdout = run_capture(
            "git",
            &[
                "ls-files",
                "-z",
                "--others",
                "--exclude-standard",
                "--",
                &git_path(pathspec),
            ],
            &self.root,
        )?;
        Ok(split_z(&stdout))
    }

    /// Tree paths under `pathspec` at `commit`.
    pub(crate) fn ls_tree(&self, commit: &str, pathspec: &str) -> Result<Vec<String>, AppError> {
        let stdout = run_capture(
            "git",
            &[
                "ls-tree",
                "-r",
                "--name-only",
                "-z",
                commit,
                "--",
                &git_path(pathspec),
            ],
            &self.root,
        )?;
        Ok(split_z(&stdout))
    }

    /// Every path at `commit` (used to find `Cargo.toml` files when listing members).
    pub(crate) fn ls_tree_all(&self, commit: &str) -> Result<Vec<String>, AppError> {
        let stdout = run_capture(
            "git",
            &["ls-tree", "-r", "--name-only", "-z", commit],
            &self.root,
        )?;
        Ok(split_z(&stdout))
    }
}

/// Git pathspecs use `/` even on Windows.
pub(crate) fn git_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// Joins a workspace-root-relative path onto the git-root-relative workspace
/// prefix so `git ls-files` / `git show` pathspecs match the repository.
pub(crate) fn join_git_rel(git_root: &Path, workspace_root: &Path, workspace_rel: &str) -> String {
    let prefix = git_path(
        &workspace_root
            .strip_prefix(git_root)
            .unwrap_or_else(|_| Path::new(""))
            .to_string_lossy(),
    );
    let prefix = prefix.trim_end_matches('/');
    let rel = git_path(workspace_rel);
    let rel = rel.trim_end_matches('/');
    if prefix.is_empty() || prefix == "." {
        rel.to_string()
    } else if rel.is_empty() || rel == "." {
        prefix.to_string()
    } else {
        format!("{prefix}/{rel}")
    }
}

fn split_z(stdout: &str) -> Vec<String> {
    stdout
        .split('\0')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn git_path_normalizes_backslashes() {
        assert_eq!(
            git_path(r"packages\foo\Cargo.toml"),
            "packages/foo/Cargo.toml"
        );
    }

    #[test]
    fn join_git_rel_prefixes_when_workspace_is_nested() {
        let git_root = Path::new("/repo");
        let workspace = Path::new("/repo/inner");
        assert_eq!(
            join_git_rel(git_root, workspace, "packages/foo"),
            "inner/packages/foo"
        );
        assert_eq!(
            join_git_rel(git_root, git_root, "packages/foo"),
            "packages/foo"
        );
        assert_eq!(join_git_rel(git_root, workspace, ""), "inner");
        assert_eq!(join_git_rel(git_root, workspace, "."), "inner");
    }

    #[test]
    fn split_z_drops_empty_trailing_field() {
        assert_eq!(split_z("a\0b\0"), vec!["a", "b"]);
        assert!(split_z("").is_empty());
    }
}
