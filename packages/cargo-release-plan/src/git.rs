// Git access via `git` subprocesses.
//
// The design forbids git2/gix; every read of history, trees, and diffs goes
// through this type.

use std::path::{Path, PathBuf};

use ohno::AppError;

use crate::UnresolvedBaseError;
use crate::command::{run_capture, run_capture_ok};

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
    /// A true root has no `parent` header. A shallow-boundary commit still has
    /// that header even when Git cannot resolve `commit^`.
    pub(crate) fn has_resolvable_parent(&self, commit: &str) -> Result<bool, AppError> {
        if !self.commit_has_parent_header(commit)? {
            return Ok(false);
        }
        let spec = format!("{commit}^");
        if run_capture_ok("git", &["rev-parse", "--verify", &spec], &self.root)?.is_some() {
            return Ok(true);
        }
        self.is_shallow()
    }

    fn commit_has_parent_header(&self, commit: &str) -> Result<bool, AppError> {
        // `cat-file -p` prints the `parent` header even when the parent object
        // was not fetched (shallow boundary). `rev-list --parents` omits that
        // parent when it cannot be resolved.
        let stdout = run_capture("git", &["cat-file", "-p", commit], &self.root)?;
        Ok(stdout.lines().any(|line| line.starts_with("parent ")))
    }

    /// First-parent commits reachable from `rev` that touch a `Cargo.toml`.
    ///
    /// Version and membership can change only on those commits, so classification
    /// reconstructs historical workspaces from this subset rather than every
    /// first-parent commit.
    pub(crate) fn first_parent_manifest_commits(&self, rev: &str) -> Result<Vec<String>, AppError> {
        let all = self.first_parent_commits(rev)?;
        let stdout = run_capture(
            "git",
            &[
                "rev-list",
                "--first-parent",
                rev,
                "--",
                "Cargo.toml",
                ":(glob)**/Cargo.toml",
            ],
            &self.root,
        )?;
        let touching: std::collections::HashSet<&str> = stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        // Keep the base revision and the oldest first-parent commit even when they
        // do not touch a manifest, so the timeline still observes HEAD and can
        // distinguish a true root from truncated history.
        let newest = all.first().cloned();
        let oldest = all.last().cloned();
        Ok(all
            .into_iter()
            .filter(|commit| {
                touching.contains(commit.as_str())
                    || newest.as_deref() == Some(commit.as_str())
                    || oldest.as_deref() == Some(commit.as_str())
            })
            .collect())
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
        match crate::command::run_capture_bytes("git", &["show", &spec], &self.root) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) => {
                if error
                    .find_source::<crate::CommandFailedError>()
                    .is_some_and(|failed| is_absent_git_path(failed.stderr()))
                {
                    Ok(None)
                } else {
                    Err(error)
                }
            }
        }
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

fn is_absent_git_path(stderr: &str) -> bool {
    let stderr = stderr.to_ascii_lowercase();
    stderr.contains("does not exist")
        || stderr.contains("exists on disk, but not in")
        || stderr.contains("did not match")
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
    fn absent_git_path_matches_known_stderr() {
        assert!(is_absent_git_path(
            "fatal: path 'x' does not exist in 'abc'"
        ));
        assert!(is_absent_git_path(
            "fatal: path 'x' exists on disk, but not in 'abc'"
        ));
        assert!(is_absent_git_path(
            "fatal: Path 'x' did not match any files"
        ));
        assert!(!is_absent_git_path("fatal: bad object abc"));
    }

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
