// Git access via `git` subprocesses.
//
// The design forbids git2/gix; every read of history, trees, and diffs goes
// through this type. Ref: docs/implementation.md, "Subprocess boundaries".

use std::collections::HashSet;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use ohno::AppError;

use crate::command::{run_capture, run_capture_bytes, run_capture_ok, run_capture_os};
use crate::{CommandFailedError, UnresolvedBaseError};

/// Name Cargo requires for a manifest.
const MANIFEST_FILE_NAME: &str = "Cargo.toml";

/// Pathspec matching manifests below the repository root.
///
/// The `glob` magic makes `**` cross directory boundaries, which the default
/// pathspec syntax does not do.
const MANIFEST_GLOB_PATHSPEC: &str = ":(glob)**/Cargo.toml";

/// A Git repository rooted at `root`.
#[derive(Clone, Debug)]
pub(crate) struct GitRepo {
    root: PathBuf,
    prefix: String,
}

impl GitRepo {
    /// Discovers the repository containing `start` using `git rev-parse`.
    pub(crate) fn discover(start: &Path) -> Result<Self, AppError> {
        let dir = if start.is_file() {
            start.parent().unwrap_or(start)
        } else {
            start
        };
        // Both answers come from one invocation so they describe the same
        // directory. Deriving the prefix instead, by stripping the root from a
        // path Cargo reported, compares two spellings of the same directory
        // that need not match: Windows hands out 8.3 short names for some
        // paths, and symlinked or substituted roots differ on every platform.
        let stdout = run_capture(
            "git",
            &["rev-parse", "--show-toplevel", "--show-prefix"],
            dir,
        )?;
        let mut lines = stdout.lines();
        let root = lines.next().unwrap_or_default().trim();
        // The prefix line is empty when the repository root is the directory
        // itself, and Git may drop the trailing newline that would carry it.
        let prefix = lines.next().unwrap_or_default().trim();
        Ok(Self {
            root: PathBuf::from(root),
            prefix: prefix.trim_end_matches('/').to_string(),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The repository-relative directory the repository was discovered from.
    ///
    /// Empty when that directory is the repository root.
    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
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

    /// Whether `commit` has a parent that the walk must not treat as a true root.
    ///
    /// A true root commit has no `parent` header. A shallow-boundary commit has
    /// one even though Git cannot resolve `commit^`, and reports `true` here so
    /// the caller reports truncated history instead of a root.
    pub(crate) fn has_parent_or_is_shallow_boundary(&self, commit: &str) -> Result<bool, AppError> {
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
        // A blank line terminates the header block and the message follows, so
        // only the headers are inspected: a message body line that happens to
        // start with `parent ` must not make a root commit look parented.
        Ok(stdout
            .lines()
            .take_while(|line| !line.is_empty())
            .any(|line| line.starts_with("parent ")))
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
                MANIFEST_FILE_NAME,
                MANIFEST_GLOB_PATHSPEC,
            ],
            &self.root,
        )?;
        let touching: HashSet<&str> = stdout
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
        let spec = format!("{commit}:{rel_path}");
        match run_capture_bytes("git", &["show", &spec], &self.root) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) => {
                if error
                    .find_source::<CommandFailedError>()
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
            &["ls-files", "-z", "--", &dir_pathspec(pathspec)],
            &self.root,
        )?;
        Ok(split_z(&stdout))
    }

    /// Which of `paths` Git tracks, as a subset of the input.
    ///
    /// Files a manifest names from outside its package directory are not
    /// covered by any per-directory listing, so their tracked state is asked
    /// for by exact path. An empty input answers without invoking Git, because
    /// `git ls-files` with no pathspec lists the whole repository.
    pub(crate) fn tracked_paths(&self, paths: &[&str]) -> Result<HashSet<String>, AppError> {
        if paths.is_empty() {
            return Ok(HashSet::new());
        }
        let mut args = vec!["ls-files".to_string(), "-z".to_string(), "--".to_string()];
        args.extend(paths.iter().map(|path| dir_pathspec(path)));
        let stdout = run_capture_os("git", &args, &self.root)?;
        Ok(split_z(&stdout).into_iter().collect())
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
                &dir_pathspec(pathspec),
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
                &dir_pathspec(pathspec),
            ],
            &self.root,
        )?;
        Ok(split_z(&stdout))
    }

    /// Manifest paths at `commit`, used to reconstruct historical workspace members.
    ///
    /// `git ls-tree` matches its path arguments literally, so it cannot select
    /// manifests by pattern: the tree is listed once and filtered here.
    pub(crate) fn ls_tree_manifests(&self, commit: &str) -> Result<Vec<String>, AppError> {
        let stdout = run_capture(
            "git",
            &["ls-tree", "-r", "--name-only", "-z", commit],
            &self.root,
        )?;
        Ok(split_z(&stdout)
            .into_iter()
            .filter(|path| is_manifest_path(path))
            .collect())
    }
}

/// Whether a repository-relative tree path names a Cargo manifest.
fn is_manifest_path(path: &str) -> bool {
    path.rsplit('/').next() == Some(MANIFEST_FILE_NAME)
}

/// Rewrites an operating-system path into the `/`-separated form Git reports.
///
/// Only the platform's own separator is rewritten. A backslash is an ordinary
/// character in a file name on Unix, so rewriting one would name a different
/// file, and paths Git itself reports already use `/` on every platform and are
/// therefore taken verbatim.
pub(crate) fn os_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if MAIN_SEPARATOR == '/' {
        text.into_owned()
    } else {
        text.replace(MAIN_SEPARATOR, "/")
    }
}

/// Turns a directory into a pathspec Git matches literally.
///
/// Directory names come out of the repository, so a name containing pathspec
/// syntax — a `*`, or a leading `:` Git would read as magic — would otherwise
/// select a sibling package's files or fail to select the package's own,
/// producing a release verdict for content that is not the package's.
/// `:(literal)` turns the whole value back into a plain path.
///
/// The repository root is the empty string in every path this tool computes,
/// but Git rejects an empty pathspec, so the root becomes `.`.
fn dir_pathspec(dir: &str) -> String {
    let dir = if dir.is_empty() { "." } else { dir };
    format!(":(literal){dir}")
}

/// Joins a workspace-relative path onto the workspace's git prefix.
///
/// The result is a pathspec that `git ls-files` and `git show` resolve against
/// the repository root. Both operands are already in Git's `/`-separated space.
pub(crate) fn join_git_rel(prefix: &str, workspace_rel: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let rel = workspace_rel.trim_end_matches('/');
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

/// Splits NUL-terminated `git` path output into paths.
///
/// With `-z` the NUL already delimits every record, so whitespace inside a field
/// is filename data and must survive. Only the empty field after the final
/// terminator is dropped.
fn split_z(stdout: &str) -> Vec<String> {
    stdout
        .split('\0')
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
    fn manifest_paths_are_selected_by_file_name() {
        assert!(is_manifest_path("Cargo.toml"));
        assert!(is_manifest_path("packages/foo/Cargo.toml"));
        assert!(!is_manifest_path("packages/foo/Cargo.toml.bak"));
        assert!(!is_manifest_path("packages/foo/src/lib.rs"));
        assert!(!is_manifest_path("packages/Cargo.toml/inner.rs"));
    }

    /// A backslash is an ordinary character in a file name on Unix, so only the
    /// platform's own separator may be rewritten when an operating-system path
    /// enters Git's path space.
    #[test]
    fn os_path_rewrites_only_the_platform_separator() {
        let native = PathBuf::from("packages").join("foo").join("Cargo.toml");
        assert_eq!(os_path(&native), "packages/foo/Cargo.toml");

        if MAIN_SEPARATOR != '\\' {
            assert_eq!(os_path(Path::new(r"odd\name.rs")), r"odd\name.rs");
        }
    }

    #[test]
    fn join_git_rel_prefixes_when_workspace_is_nested() {
        assert_eq!(join_git_rel("inner", "packages/foo"), "inner/packages/foo");
        assert_eq!(join_git_rel("", "packages/foo"), "packages/foo");
        assert_eq!(join_git_rel("inner", ""), "inner");
        assert_eq!(join_git_rel("inner", "."), "inner");
        assert_eq!(join_git_rel("inner/", "packages/foo"), "inner/packages/foo");
        assert_eq!(join_git_rel("", ""), "");
        assert_eq!(join_git_rel(".", "packages/foo"), "packages/foo");
    }

    /// Cargo and Git need not spell the same directory identically: Windows
    /// hands out 8.3 short names for some paths and both tools accept
    /// uncanonical spellings, so the prefix must come from Git rather than from
    /// subtracting one reported path from the other.
    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn discover_reports_a_prefix_for_an_uncanonical_directory() {
        let temp = tempfile::tempdir().unwrap();
        run_capture("git", &["init", "-q"], temp.path()).unwrap();
        let nested = temp.path().join("inner");
        std::fs::create_dir_all(&nested).unwrap();

        let repo = GitRepo::discover(&nested.join("..").join("inner")).unwrap();

        assert_eq!(repo.prefix(), "inner");
    }

    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn discover_reports_an_empty_prefix_at_the_repository_root() {
        let temp = tempfile::tempdir().unwrap();
        run_capture("git", &["init", "-q"], temp.path()).unwrap();

        let repo = GitRepo::discover(temp.path()).unwrap();

        assert_eq!(repo.prefix(), "");
        assert!(
            repo.root().ends_with(
                temp.path()
                    .file_name()
                    .expect("a temporary directory always has a final component")
            )
        );
    }

    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn discover_starts_from_the_directory_holding_a_file() {
        let temp = tempfile::tempdir().unwrap();
        run_capture("git", &["init", "-q"], temp.path()).unwrap();
        let nested = temp.path().join("inner");
        std::fs::create_dir_all(&nested).unwrap();
        let manifest = nested.join(MANIFEST_FILE_NAME);
        std::fs::write(&manifest, "").unwrap();

        let repo = GitRepo::discover(&manifest).unwrap();

        assert_eq!(repo.prefix(), "inner");
    }

    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn discover_fails_outside_a_repository() {
        let temp = tempfile::tempdir().unwrap();

        GitRepo::discover(temp.path()).unwrap_err();
    }

    /// Every listing runs `git` in the repository root, so a root that is not a
    /// repository must surface the failure rather than an empty listing.
    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn listings_fail_when_the_root_is_not_a_repository() {
        let temp = tempfile::tempdir().unwrap();
        let repo = GitRepo {
            root: temp.path().to_path_buf(),
            prefix: String::new(),
        };

        repo.first_parent_commits("HEAD").unwrap_err();
        repo.ls_files("").unwrap_err();
        repo.ls_untracked("").unwrap_err();
        repo.ls_tree("HEAD", "").unwrap_err();
        repo.ls_tree_manifests("HEAD").unwrap_err();
        repo.rev_parse("HEAD").unwrap_err();
    }

    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn a_commit_with_a_reachable_parent_is_not_a_root() {
        let temp = tempfile::tempdir().unwrap();
        let repo = init_repo_with_two_commits(temp.path());
        let head = repo.rev_parse("HEAD").unwrap();
        let root = repo.rev_parse("HEAD~1").unwrap();

        assert!(repo.has_parent_or_is_shallow_boundary(&head).unwrap());
        assert!(!repo.has_parent_or_is_shallow_boundary(&root).unwrap());
    }

    /// The commit message follows the headers in `cat-file -p` output, so a
    /// message body that mentions a parent must not be read as a header.
    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn a_root_commit_whose_message_mentions_a_parent_is_still_a_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        run_capture("git", &["init", "-q"], root).unwrap();
        run_capture("git", &["config", "user.name", "test"], root).unwrap();
        run_capture("git", &["config", "user.email", "test@example.com"], root).unwrap();
        run_capture("git", &["config", "commit.gpgsign", "false"], root).unwrap();
        std::fs::write(root.join("first.txt"), "one\n").unwrap();
        run_capture("git", &["add", "-A"], root).unwrap();
        run_capture(
            "git",
            &[
                "commit",
                "-q",
                "-m",
                "subject",
                "-m",
                "parent 0123456789012345678901234567890123456789",
            ],
            root,
        )
        .unwrap();
        let repo = GitRepo::discover(root).unwrap();
        let head = repo.rev_parse("HEAD").unwrap();

        assert!(!repo.has_parent_or_is_shallow_boundary(&head).unwrap());
    }

    /// A path absent at a commit is an ordinary answer, while any other `git
    /// show` failure is a real error the caller must see.
    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn show_file_distinguishes_an_absent_path_from_a_failure() {
        let temp = tempfile::tempdir().unwrap();
        let repo = init_repo_with_two_commits(temp.path());

        assert_eq!(
            repo.show_file("HEAD", "first.txt").unwrap().as_deref(),
            Some("one\n")
        );
        assert_eq!(repo.show_file("HEAD", "absent.txt").unwrap(), None);
        repo.show_file("no-such-revision", "first.txt").unwrap_err();
    }

    fn init_repo_with_two_commits(root: &Path) -> GitRepo {
        let repo = init_repo(root);
        std::fs::write(root.join("second.txt"), "two\n").unwrap();
        run_capture("git", &["add", "-A"], root).unwrap();
        run_capture("git", &["commit", "-q", "-m", "second"], root).unwrap();
        repo
    }

    /// Initialises a hermetic repository and commits whatever `root` holds.
    fn init_repo(root: &Path) -> GitRepo {
        run_capture("git", &["init", "-q"], root).unwrap();
        run_capture("git", &["config", "user.name", "test"], root).unwrap();
        run_capture("git", &["config", "user.email", "test@example.com"], root).unwrap();
        run_capture("git", &["config", "commit.gpgsign", "false"], root).unwrap();
        std::fs::write(root.join("first.txt"), "one\n").unwrap();
        run_capture("git", &["add", "-A"], root).unwrap();
        run_capture("git", &["commit", "-q", "-m", "first"], root).unwrap();
        GitRepo::discover(root).unwrap()
    }

    #[test]
    fn split_z_drops_empty_trailing_field() {
        assert_eq!(split_z("a\0b\0"), vec!["a", "b"]);
        assert!(split_z("").is_empty());
    }

    #[test]
    fn split_z_preserves_whitespace_inside_paths() {
        // With `-z` these characters are filename data, not record separators.
        assert_eq!(
            split_z(" leading.rs\0trailing.rs \0mid\nline.rs\0"),
            vec![" leading.rs", "trailing.rs ", "mid\nline.rs"]
        );
    }

    #[test]
    fn dir_pathspec_is_literal_and_names_the_repository_root() {
        assert_eq!(dir_pathspec(""), ":(literal).");
        assert_eq!(dir_pathspec("packages/foo"), ":(literal)packages/foo");
        // Without the magic prefix this would select every sibling package.
        assert_eq!(dir_pathspec("packages/foo*"), ":(literal)packages/foo*");
    }

    /// The literal pathspec must survive the round trip through Git itself: the
    /// escaping is only correct if Git reads it back as one plain path.
    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn a_directory_named_like_a_pattern_lists_only_its_own_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // `de[m]o` is a legal directory name on every supported platform and is
        // also a glob that matches its sibling `demo`.
        for dir in ["packages/de[m]o", "packages/demo"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("lib.rs"), "x").unwrap();
        }
        let repo = init_repo(root);

        assert_eq!(
            repo.ls_files("packages/de[m]o").unwrap(),
            vec!["packages/de[m]o/lib.rs".to_string()]
        );
        assert_eq!(
            repo.ls_tree("HEAD", "packages/de[m]o").unwrap(),
            vec!["packages/de[m]o/lib.rs".to_string()]
        );
    }
}
