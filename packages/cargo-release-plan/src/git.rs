// Git access via `git` subprocesses.
//
// The design forbids git2/gix; every read of history, trees, and diffs goes
// through this type. Ref: docs/implementation.md, "Subprocess boundaries".

use std::collections::HashSet;
use std::mem;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use ohno::AppError;

use crate::command::{run_capture, run_capture_bytes, run_capture_ok, run_capture_os_bytes};
use crate::{
    CommandFailedError, NonUtf8BlobError, NonUtf8PathError, PathTooLongError, UnresolvedBaseError,
};

/// Name Cargo requires for a manifest.
const MANIFEST_FILE_NAME: &str = "Cargo.toml";

/// Pathspec matching manifests below the repository root.
///
/// The `glob` magic makes `**` cross directory boundaries, which the default
/// pathspec syntax does not do.
const MANIFEST_GLOB_PATHSPEC: &str = ":(glob)**/Cargo.toml";

/// Tree mode Git records for a symbolic link.
///
/// Ref: the `git ls-tree` documentation, which fixes the set of modes a tree
/// entry may carry.
const SYMLINK_TREE_MODE: &str = "120000";

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

    /// Text at `commit:rel_path`, or `None` if the path is absent.
    ///
    /// Callers parse the result as TOML. Replacing invalid bytes would turn
    /// content Cargo could never have parsed into a different, parseable
    /// document and classify a package against text Git does not store, so a
    /// blob that is not valid UTF-8 is reported instead.
    pub(crate) fn show_file(
        &self,
        commit: &str,
        rel_path: &str,
    ) -> Result<Option<String>, AppError> {
        match self.show_file_bytes(commit, rel_path)? {
            Some(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| NonUtf8BlobError::caused_by(commit, rel_path, error).into()),
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
        let stdout = run_capture_bytes(
            "git",
            &["ls-files", "-z", "--", &dir_pathspec(pathspec)],
            &self.root,
        )?;
        split_z(&stdout)
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
        let stdout = run_capture_os_bytes("git", &args, &self.root)?;
        Ok(split_z(&stdout)?.into_iter().collect())
    }

    /// Untracked, non-ignored paths under `pathspec`.
    // Advisory-only listing; classification does not fail on untracked files.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn ls_untracked(&self, pathspec: &str) -> Result<Vec<String>, AppError> {
        let stdout = run_capture_bytes(
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
        split_z(&stdout)
    }

    /// Tree entries under `pathspecs` at `commit`.
    ///
    /// The mode and object id are kept alongside the path because both answer
    /// questions the path cannot. A link's blob holds its target path, which is
    /// indistinguishable from a regular file's content once read, so the mode is
    /// the only place that distinction survives; and the object id is the
    /// content identity Git itself compares by, which is what a work-tree file
    /// has to be compared against once a filter stands between the two.
    pub(crate) fn ls_tree(
        &self,
        commit: &str,
        pathspecs: &[&str],
    ) -> Result<Vec<TreeEntry>, AppError> {
        let mut args = vec![
            "ls-tree".to_string(),
            "-r".to_string(),
            "-z".to_string(),
            commit.to_string(),
            "--".to_string(),
        ];
        args.extend(pathspecs.iter().map(|pathspec| dir_pathspec(pathspec)));
        let stdout = run_capture_os_bytes("git", &args, &self.root)?;
        Ok(split_z(&stdout)?
            .iter()
            .filter_map(|record| TreeEntry::parse(record))
            .collect())
    }

    /// Object ids the work-tree files at `rel_paths` would be stored under.
    ///
    /// Git converts content on its way into the object database, so a file on
    /// disk and the blob recording it need not hold the same bytes: an LFS
    /// pointer and a line-ending rule both make the two diverge. Asking Git to
    /// hash the work-tree file applies the same conversion the file would get if
    /// it were staged, which puts both ends of a comparison in one
    /// representation. Ids come back in the order the paths were given.
    pub(crate) fn hash_objects(&self, rel_paths: &[&str]) -> Result<Vec<String>, AppError> {
        let mut ids = Vec::with_capacity(rel_paths.len());
        for chunk in command_line_batches(rel_paths)? {
            // The paths are handed to the child borrowed: they outlive the call
            // and copying them would duplicate every path in the request.
            let args = ["hash-object", "--"].into_iter().chain(chunk);
            let stdout = run_capture_os_bytes("git", args, &self.root)?;
            ids.extend(
                String::from_utf8_lossy(&stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(ToOwned::to_owned),
            );
        }
        Ok(ids)
    }

    /// Manifest paths at `commit`, used to reconstruct historical workspace members.
    ///
    /// `git ls-tree` matches its path arguments literally, so it cannot select
    /// manifests by pattern: the tree is listed once and filtered here.
    pub(crate) fn ls_tree_manifests(&self, commit: &str) -> Result<Vec<String>, AppError> {
        let stdout = run_capture_bytes(
            "git",
            &["ls-tree", "-r", "--name-only", "-z", commit],
            &self.root,
        )?;
        Ok(split_z(&stdout)?
            .into_iter()
            .filter(|path| is_manifest_path(path))
            .collect())
    }
}

/// Whether a repository-relative tree path names a Cargo manifest.
fn is_manifest_path(path: &str) -> bool {
    path.rsplit('/').next() == Some(MANIFEST_FILE_NAME)
}

/// Command-line budget one `git hash-object` invocation may spend on paths.
///
/// Windows renders a child's arguments into a single command line that
/// `CreateProcessW` caps at 32,767 UTF-16 code units, which is the smallest
/// limit among supported platforms. The remainder of that cap is left for the
/// executable path, the fixed arguments, and separators.
const PATH_ARG_BUDGET: usize = 30_000;

/// Splits `rel_paths` into batches whose rendered command line fits the budget.
///
/// A path count cannot bound a command line, because paths vary in length. Each
/// path is charged its worst-case rendered cost, so an ordinary package still
/// takes one round trip while a deeply nested one is split instead of failing
/// to spawn.
fn command_line_batches<'p>(rel_paths: &[&'p str]) -> Result<Vec<Vec<&'p str>>, AppError> {
    let mut batches = Vec::new();
    let mut current: Vec<&'p str> = Vec::new();
    let mut spent = 0_usize;
    for path in rel_paths {
        let cost = rendered_arg_cost(path);
        if cost > PATH_ARG_BUDGET {
            return Err(PathTooLongError::new(*path).into());
        }
        if spent.saturating_add(cost) > PATH_ARG_BUDGET && !current.is_empty() {
            batches.push(mem::take(&mut current));
            spent = 0;
        }
        current.push(path);
        spent = spent.saturating_add(cost);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    Ok(batches)
}

/// Worst-case command-line cost of one path argument, in UTF-16 code units.
///
/// Quoting can at most double a path's length (every character escaped) and
/// adds surrounding quotes and a separator, so charging that upper bound keeps
/// the estimate on the safe side of the platform cap without modelling any
/// particular quoting rule.
fn rendered_arg_cost(path: &str) -> usize {
    path.encode_utf16()
        .count()
        .saturating_mul(2)
        .saturating_add(3)
}

/// One file recorded in a commit's tree.
///
/// Classification needs more of a tree record than the path: the mode says
/// whether the entry is a symbolic link, and the object id is the content
/// identity a work-tree file is compared against.
#[derive(Clone, Debug)]
pub(crate) struct TreeEntry {
    pub(crate) path: String,
    pub(crate) id: String,
    mode: String,
}

impl TreeEntry {
    /// Reads a `<mode> <type> <object>\t<path>` record, skipping anything else.
    fn parse(record: &str) -> Option<Self> {
        let (metadata, path) = record.split_once('\t')?;
        let mut fields = metadata.split(' ');
        let mode = fields.next()?;
        let _kind = fields.next()?;
        let id = fields.next()?;
        Some(Self {
            path: path.to_string(),
            id: id.to_string(),
            mode: mode.to_string(),
        })
    }

    /// Whether the entry is a symbolic link, which Git marks by a fixed mode.
    pub(crate) fn is_symlink(&self) -> bool {
        self.mode == SYMLINK_TREE_MODE
    }
}

/// Rewrites an operating-system path into the `/`-separated form Git reports.
///
/// Only the platform's own separator is rewritten. A backslash is an ordinary
/// character in a file name on Unix, so rewriting one would name a different
/// file, and paths Git itself reports already use `/` on every platform and are
/// therefore taken verbatim.
pub(crate) fn os_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if MAIN_SEPARATOR == '/' {
        path.into_owned()
    } else {
        path.replace(MAIN_SEPARATOR, "/")
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

/// Whether Git reported that a path is simply absent from the revision asked
/// about, rather than failing for an operational reason.
///
/// Git has no machine-readable signal for this, so the wording is matched. That
/// is only sound because [`crate::command`] pins the child locale to `C`; a
/// translated Git would report a routine package creation or deletion as an
/// operational error.
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
///
/// # Errors
///
/// Returns [`NonUtf8PathError`] if any field is not valid UTF-8.
fn split_z(stdout: &[u8]) -> Result<Vec<String>, AppError> {
    stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| {
            str::from_utf8(part)
                .map(ToOwned::to_owned)
                .map_err(|_ignored| {
                    NonUtf8PathError::new(String::from_utf8_lossy(part).into_owned()).into()
                })
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fs;

    use tempfile::tempdir;

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

    /// The mode and object id are read off the record's own fields, so a path
    /// that itself looks like a mode cannot be mistaken for one.
    #[test]
    fn tree_records_are_parsed_into_mode_and_object() {
        let link = TreeEntry::parse("120000 blob abc\tpackages/foo/link.txt").unwrap();
        assert!(link.is_symlink());
        assert_eq!(link.path, "packages/foo/link.txt");
        assert_eq!(link.id, "abc");

        let file = TreeEntry::parse("100644 blob def\tpackages/foo/real.txt").unwrap();
        assert!(!file.is_symlink());
        assert_eq!(file.id, "def");

        // A regular file whose mode merely starts with the link mode's digits.
        assert!(
            !TreeEntry::parse("1200001 blob abc\tx")
                .unwrap()
                .is_symlink()
        );
        // Not a record at all: no field separator.
        assert!(TreeEntry::parse("120000 blob abc packages/foo").is_none());
        assert!(TreeEntry::parse("120000 blob\tpackages/foo").is_none());
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

    /// Ordinary packages must still take one subprocess, or every classification
    /// would pay for extra round trips.
    #[test]
    fn short_paths_all_fit_one_batch() {
        let paths: Vec<&str> = vec!["packages/foo/src/lib.rs"; 256];
        let batches = command_line_batches(&paths).unwrap();
        assert_eq!(batches.len(), 1);
        let only = batches.first().expect("one batch was just asserted");
        assert_eq!(only.len(), paths.len());
    }

    /// Batching is bounded by the rendered command line, not by a path count.
    #[test]
    fn long_paths_are_split_across_batches() {
        // Long enough that two paths cannot share a command line.
        let long = "x".repeat(PATH_ARG_BUDGET.div_euclid(3));
        let paths: Vec<&str> = vec![long.as_str(); 4];
        let batches = command_line_batches(&paths).unwrap();
        assert!(batches.len() > 1);
        assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), paths.len());
    }

    #[test]
    fn an_empty_request_produces_no_batches() {
        assert!(command_line_batches(&[]).unwrap().is_empty());
    }

    /// A path that cannot fit alone would otherwise loop or fail to spawn with
    /// an operating-system message that names no path.
    #[test]
    fn a_path_longer_than_the_budget_is_rejected() {
        let huge = "x".repeat(PATH_ARG_BUDGET);
        let error = command_line_batches(&[huge.as_str()]).unwrap_err();
        assert!(error.find_source::<PathTooLongError>().is_some());
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
        let temp = tempdir().unwrap();
        run_capture("git", &["init", "-q"], temp.path()).unwrap();
        let nested = temp.path().join("inner");
        fs::create_dir_all(&nested).unwrap();

        let repo = GitRepo::discover(&nested.join("..").join("inner")).unwrap();

        assert_eq!(repo.prefix(), "inner");
    }

    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn discover_reports_an_empty_prefix_at_the_repository_root() {
        let temp = tempdir().unwrap();
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
        let temp = tempdir().unwrap();
        run_capture("git", &["init", "-q"], temp.path()).unwrap();
        let nested = temp.path().join("inner");
        fs::create_dir_all(&nested).unwrap();
        let manifest = nested.join(MANIFEST_FILE_NAME);
        fs::write(&manifest, "").unwrap();

        let repo = GitRepo::discover(&manifest).unwrap();

        assert_eq!(repo.prefix(), "inner");
    }

    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn discover_fails_outside_a_repository() {
        let temp = tempdir().unwrap();

        GitRepo::discover(temp.path()).unwrap_err();
    }

    /// Every listing runs `git` in the repository root, so a root that is not a
    /// repository must surface the failure rather than an empty listing.
    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn listings_fail_when_the_root_is_not_a_repository() {
        let temp = tempdir().unwrap();
        let repo = GitRepo {
            root: temp.path().to_path_buf(),
            prefix: String::new(),
        };

        repo.first_parent_commits("HEAD").unwrap_err();
        repo.ls_files("").unwrap_err();
        repo.ls_untracked("").unwrap_err();
        repo.ls_tree("HEAD", &[""]).unwrap_err();
        repo.ls_tree_manifests("HEAD").unwrap_err();
        repo.hash_objects(&["Cargo.toml"]).unwrap_err();
        repo.rev_parse("HEAD").unwrap_err();
    }

    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn a_commit_with_a_reachable_parent_is_not_a_root() {
        let temp = tempdir().unwrap();
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
        let temp = tempdir().unwrap();
        let root = temp.path();
        run_capture("git", &["init", "-q"], root).unwrap();
        run_capture("git", &["config", "user.name", "test"], root).unwrap();
        run_capture("git", &["config", "user.email", "test@example.com"], root).unwrap();
        run_capture("git", &["config", "commit.gpgsign", "false"], root).unwrap();
        fs::write(root.join("first.txt"), "one\n").unwrap();
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
        let temp = tempdir().unwrap();
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
        fs::write(root.join("second.txt"), "two\n").unwrap();
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
        fs::write(root.join("first.txt"), "one\n").unwrap();
        run_capture("git", &["add", "-A"], root).unwrap();
        run_capture("git", &["commit", "-q", "-m", "first"], root).unwrap();
        GitRepo::discover(root).unwrap()
    }

    #[test]
    fn split_z_drops_empty_trailing_field() {
        assert_eq!(split_z(b"a\0b\0").unwrap(), vec!["a", "b"]);
        assert!(split_z(b"").unwrap().is_empty());
    }

    #[test]
    fn split_z_preserves_whitespace_inside_paths() {
        // With `-z` these characters are filename data, not record separators.
        assert_eq!(
            split_z(b" leading.rs\0trailing.rs \0mid\nline.rs\0").unwrap(),
            vec![" leading.rs", "trailing.rs ", "mid\nline.rs"]
        );
    }

    #[test]
    fn split_z_rejects_a_path_that_is_not_utf8() {
        // A Unix file name is an arbitrary byte string. Substituting the invalid
        // byte would name a file that is not the one Git reported.
        let error = split_z(b"ok.rs\0bad\xffname.rs\0").unwrap_err();
        assert!(error.find_source::<NonUtf8PathError>().is_some());
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
        let temp = tempdir().unwrap();
        let root = temp.path();
        // `de[m]o` is a legal directory name on every supported platform and is
        // also a glob that matches its sibling `demo`.
        for dir in ["packages/de[m]o", "packages/demo"] {
            fs::create_dir_all(root.join(dir)).unwrap();
            fs::write(root.join(dir).join("lib.rs"), "x").unwrap();
        }
        let repo = init_repo(root);

        assert_eq!(
            repo.ls_files("packages/de[m]o").unwrap(),
            vec!["packages/de[m]o/lib.rs".to_string()]
        );
        let entries = repo.ls_tree("HEAD", &["packages/de[m]o"]).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            vec!["packages/de[m]o/lib.rs".to_string()]
        );
    }

    /// Git converts content on its way into the object database, so the id a
    /// work-tree file hashes to is the representation both ends of a comparison
    /// have to be expressed in. It must agree with the id the tree records for
    /// an unmodified file.
    #[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
    #[test]
    fn a_work_tree_file_hashes_to_the_id_its_tree_entry_records() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("packages/demo")).unwrap();
        fs::write(root.join("packages/demo/lib.rs"), "x").unwrap();
        fs::write(root.join("packages/demo/other.rs"), "y").unwrap();
        let repo = init_repo(root);

        let entries = repo.ls_tree("HEAD", &["packages/demo"]).unwrap();
        let recorded: Vec<String> = entries.iter().map(|entry| entry.id.clone()).collect();
        let hashed = repo
            .hash_objects(&["packages/demo/lib.rs", "packages/demo/other.rs"])
            .unwrap();

        assert_eq!(hashed, recorded);
    }
}
