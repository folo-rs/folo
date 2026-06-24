//! The git-history port: read-only access to a repository's commit topology.
//!
//! `analyze` no longer orders a series by timestamp; it resolves *which* commits
//! belong to a series, and in *what order*, from live git history (see the
//! `analyze` command in `DESIGN.md`). This module defines the [`GitHistory`] port
//! for that — a real adapter
//! that shells out to `git` (with the command output parsed by pure, unit-tested
//! helpers) and an in-memory fake (in `#[cfg(test)]`) that models a canned commit
//! graph so the query logic is testable without a real repository.

use std::future::Future;
use std::io;
use std::path::PathBuf;

use crate::process::capture;

/// Read-only access to a repository's commit topology.
///
/// All methods report a missing ref (or a path that is not a repository) as an
/// absent value (`Ok(None)` / an empty list), reserving `Err` for an unexpected
/// failure to invoke `git`.
pub(crate) trait GitHistory {
    /// Resolves a ref (branch, tag, `HEAD`, or SHA) to its full commit SHA.
    ///
    /// Returns `Ok(None)` when the ref does not resolve — an absent branch, or a
    /// path that is not a git repository at all (which is how `analyze` detects
    /// the "no repo" condition).
    fn resolve(&self, reference: &str) -> impl Future<Output = io::Result<Option<String>>>;

    /// Detects the repository's default branch (for example `main` or `master`).
    ///
    /// Returns `Ok(None)` when it cannot be determined, leaving the caller to
    /// fall back to a configured value.
    fn default_branch(&self) -> impl Future<Output = io::Result<Option<String>>>;

    /// Returns the merge-base (best common ancestor) of two commits, or
    /// `Ok(None)` when they share no history.
    fn merge_base(&self, a: &str, b: &str) -> impl Future<Output = io::Result<Option<String>>>;

    /// Returns the first-parent ancestry of `reference`, **oldest commit first**.
    ///
    /// This is the linear mainline of the ref — the timeline `analyze` orders a
    /// series by. An unresolvable ref yields an empty list.
    fn first_parent(&self, reference: &str) -> impl Future<Output = io::Result<Vec<String>>>;

    /// Reports whether the working tree currently has uncommitted changes
    /// (tracked modifications, staged changes, or untracked files).
    ///
    /// `analyze` uses this to decide whether dirty snapshots on the base
    /// branch's tip are the user's current work — admitted, with a warning — or
    /// stale leftovers to be excluded (see the `analyze` command in `DESIGN.md`).
    fn is_dirty(&self) -> impl Future<Output = io::Result<bool>>;
}

/// The real [`GitHistory`], shelling out to `git` in a fixed repository directory.
#[derive(Clone, Debug)]
pub(crate) struct SystemGitHistory {
    /// The repository working directory all `git` invocations run against.
    repo: PathBuf,
}

impl SystemGitHistory {
    /// Creates a history bound to the repository rooted at `repo`.
    pub(crate) fn new(repo: impl Into<PathBuf>) -> Self {
        Self { repo: repo.into() }
    }

    /// Runs `git -C <repo> <args>`, returning its stdout on success or `None`
    /// when `git` exits non-zero (an absent ref, or not a repository).
    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; the parsing it delegates to is tested.
    async fn run(&self, args: &[&str]) -> io::Result<Option<String>> {
        let repo = self.repo.to_string_lossy().into_owned();
        let mut full: Vec<&str> = vec!["-C", repo.as_str()];
        full.extend_from_slice(args);
        let output = capture("git", &full).await?;
        if output.status.success() {
            Ok(Some(output.stdout))
        } else {
            Ok(None)
        }
    }
}

impl GitHistory for SystemGitHistory {
    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; parsing delegated to `parse_sha`.
    async fn resolve(&self, reference: &str) -> io::Result<Option<String>> {
        // `^{commit}` peels tags/refs to the commit they name; `--verify --quiet`
        // makes an unknown ref a clean non-zero exit rather than noisy output.
        let spec = format!("{reference}^{{commit}}");
        let output = self
            .run(&["rev-parse", "--verify", "--quiet", &spec])
            .await?;
        Ok(output.and_then(|stdout| parse_sha(&stdout)))
    }

    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; parsing delegated to `branch_from_symbolic_ref`.
    async fn default_branch(&self) -> io::Result<Option<String>> {
        // The remote's published default is the authoritative answer when set.
        if let Some(stdout) = self
            .run(&["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])
            .await?
            && let Some(branch) = branch_from_symbolic_ref(&stdout)
        {
            return Ok(Some(branch));
        }
        // Otherwise fall back to whichever conventional name exists locally.
        for candidate in ["main", "master"] {
            if self.resolve(candidate).await?.is_some() {
                return Ok(Some(candidate.to_owned()));
            }
        }
        Ok(None)
    }

    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; parsing delegated to `parse_sha`.
    async fn merge_base(&self, a: &str, b: &str) -> io::Result<Option<String>> {
        let output = self.run(&["merge-base", a, b]).await?;
        Ok(output.and_then(|stdout| parse_sha(&stdout)))
    }

    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; parsing delegated to `parse_rev_list`.
    async fn first_parent(&self, reference: &str) -> io::Result<Vec<String>> {
        let output = self
            .run(&["rev-list", "--first-parent", "--reverse", reference])
            .await?;
        Ok(output
            .map(|stdout| parse_rev_list(&stdout))
            .unwrap_or_default())
    }

    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; the dirty test is delegated to `porcelain_is_dirty`.
    async fn is_dirty(&self) -> io::Result<bool> {
        let output = self.run(&["status", "--porcelain"]).await?;
        // A non-repository path exits non-zero (`None`); treat it as not dirty —
        // `analyze` already errors out earlier when the ref does not resolve.
        Ok(output.as_deref().is_some_and(porcelain_is_dirty))
    }
}

/// Reports whether `git status --porcelain` output indicates an unclean working
/// tree. The command prints one line per changed or untracked path and nothing at
/// all for a clean tree, so any non-blank content means dirty.
fn porcelain_is_dirty(stdout: &str) -> bool {
    !stdout.trim().is_empty()
}

/// Extracts a single commit SHA from `git rev-parse` / `git merge-base` output:
/// the first non-empty trimmed line, or `None` when there is none.
fn parse_sha(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

/// Parses `git rev-list` output into commit SHAs in line order, dropping blank
/// lines. With `--reverse` the input is already oldest-first.
fn parse_rev_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Extracts the branch name from `git symbolic-ref refs/remotes/origin/HEAD`
/// output (for example `refs/remotes/origin/main` -> `main`), or `None` when the
/// output does not name a remote-tracking branch.
fn branch_from_symbolic_ref(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    trimmed
        .strip_prefix("refs/remotes/origin/")
        .filter(|branch| !branch.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
pub(crate) use fake::FakeGitHistory;

#[cfg(test)]
mod fake {
    use std::collections::HashMap;
    use std::future::{Future, ready};
    use std::io;

    use super::GitHistory;

    /// An in-memory [`GitHistory`] over a canned first-parent commit graph.
    ///
    /// The graph is modeled by each commit's **first parent** (`None` for a root),
    /// which is exactly the topology `analyze` consults: first-parent ancestry and
    /// the merge-base along it. Named refs (including `HEAD`) point at commits.
    ///
    /// It deliberately cannot represent a merge commit (a commit has a single
    /// parent), so its [`merge_base`](GitHistory::merge_base) walks first-parent
    /// chains only and diverges from real `git merge-base` (full DAG) on
    /// merge-laden histories — for example a feature branch that merged its base in,
    /// whose real merge-base sits off the first-parent chain. Cover such merge
    /// topologies with the real-git integration tests, not this fake.
    #[derive(Clone, Debug, Default)]
    pub(crate) struct FakeGitHistory {
        /// Ref name (branch/tag/`HEAD`) -> the commit SHA it points at.
        refs: HashMap<String, String>,
        /// Commit SHA -> its first parent (`None` for a root commit).
        parents: HashMap<String, Option<String>>,
        /// The detected default branch, if the repository advertises one.
        default_branch: Option<String>,
        /// Whether the working tree has uncommitted changes.
        dirty: bool,
    }

    impl FakeGitHistory {
        /// Creates an empty history (no commits, no refs).
        pub(crate) fn new() -> Self {
            Self {
                refs: HashMap::new(),
                parents: HashMap::new(),
                default_branch: None,
                dirty: false,
            }
        }

        /// Records a commit `sha` with the given first `parent` (`None` = root).
        pub(crate) fn commit(&mut self, sha: &str, parent: Option<&str>) -> &mut Self {
            self.parents
                .insert(sha.to_owned(), parent.map(ToOwned::to_owned));
            self
        }

        /// Points a named ref at a commit.
        pub(crate) fn branch(&mut self, name: &str, sha: &str) -> &mut Self {
            self.refs.insert(name.to_owned(), sha.to_owned());
            self
        }

        /// Points `HEAD` at the commit a ref or SHA resolves to.
        pub(crate) fn head(&mut self, reference: &str) -> &mut Self {
            let sha = self.resolve_sync(reference).unwrap();
            self.refs.insert("HEAD".to_owned(), sha);
            self
        }

        /// Sets the advertised default branch.
        pub(crate) fn mark_default(&mut self, name: &str) -> &mut Self {
            self.default_branch = Some(name.to_owned());
            self
        }

        /// Marks the working tree as having uncommitted changes.
        pub(crate) fn mark_dirty(&mut self) -> &mut Self {
            self.dirty = true;
            self
        }

        /// Resolves a ref or raw SHA to a commit SHA, without async.
        fn resolve_sync(&self, reference: &str) -> Option<String> {
            if let Some(sha) = self.refs.get(reference) {
                return Some(sha.clone());
            }
            if self.parents.contains_key(reference) {
                return Some(reference.to_owned());
            }
            None
        }

        /// The first-parent chain of `sha`, newest commit first.
        fn chain(&self, sha: &str) -> Vec<String> {
            let mut chain = Vec::new();
            let mut current = Some(sha.to_owned());
            while let Some(commit) = current {
                let next = self.parents.get(&commit).cloned().flatten();
                chain.push(commit);
                current = next;
            }
            chain
        }

        /// The merge-base of two refs along their first-parent chains, or `None`
        /// when either is unresolvable or they share no history.
        fn merge_base_sync(&self, a: &str, b: &str) -> Option<String> {
            let a = self.resolve_sync(a)?;
            let b = self.resolve_sync(b)?;
            let ancestors_a: std::collections::HashSet<String> =
                self.chain(&a).into_iter().collect();
            // Walk b's first-parent chain newest-first; the first commit also in
            // a's chain is the best common ancestor along the mainline.
            self.chain(&b)
                .into_iter()
                .find(|commit| ancestors_a.contains(commit))
        }
    }

    impl GitHistory for FakeGitHistory {
        fn resolve(&self, reference: &str) -> impl Future<Output = io::Result<Option<String>>> {
            ready(Ok(self.resolve_sync(reference)))
        }

        fn default_branch(&self) -> impl Future<Output = io::Result<Option<String>>> {
            ready(Ok(self.default_branch.clone()))
        }

        fn merge_base(&self, a: &str, b: &str) -> impl Future<Output = io::Result<Option<String>>> {
            let base = self.merge_base_sync(a, b);
            ready(Ok(base))
        }

        fn first_parent(&self, reference: &str) -> impl Future<Output = io::Result<Vec<String>>> {
            let Some(sha) = self.resolve_sync(reference) else {
                return ready(Ok(Vec::new()));
            };
            let mut chain = self.chain(&sha);
            chain.reverse(); // oldest-first, matching `rev-list --reverse`.
            ready(Ok(chain))
        }

        fn is_dirty(&self) -> impl Future<Output = io::Result<bool>> {
            ready(Ok(self.dirty))
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn parse_sha_takes_first_non_empty_line() {
        assert_eq!(
            parse_sha("  \n abc123 \n def \n"),
            Some("abc123".to_owned())
        );
        assert_eq!(parse_sha("   \n  \n"), None);
        assert_eq!(parse_sha(""), None);
    }

    #[test]
    fn parse_rev_list_keeps_order_and_drops_blanks() {
        let parsed = parse_rev_list("c0\nc1\n\n  c2  \n");
        assert_eq!(parsed, vec!["c0", "c1", "c2"]);
        assert!(parse_rev_list("\n   \n").is_empty());
    }

    #[test]
    fn porcelain_is_dirty_detects_any_nonblank_content() {
        assert!(porcelain_is_dirty(" M src/lib.rs\n"));
        assert!(porcelain_is_dirty("?? new_file\n"));
        assert!(!porcelain_is_dirty(""));
        assert!(!porcelain_is_dirty("   \n  \n"));
    }

    #[test]
    fn branch_from_symbolic_ref_strips_remote_prefix() {
        assert_eq!(
            branch_from_symbolic_ref("refs/remotes/origin/main\n"),
            Some("main".to_owned())
        );
        assert_eq!(
            branch_from_symbolic_ref("refs/remotes/origin/release/v2"),
            Some("release/v2".to_owned())
        );
        assert_eq!(branch_from_symbolic_ref("refs/heads/main"), None);
        assert_eq!(branch_from_symbolic_ref("refs/remotes/origin/"), None);
    }

    /// Builds the canonical fixture graph used by the topology tests:
    ///
    /// ```text
    /// master: c0 - c1 - c2 - c3
    ///                \
    /// feature:        f1 - f2
    /// ```
    fn fixture() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .commit("f1", Some("c1"))
            .commit("f2", Some("f1"))
            .branch("master", "c3")
            .branch("feature", "f2")
            .head("feature")
            .mark_default("master");
        git
    }

    #[test]
    fn fake_resolves_refs_and_raw_shas() {
        let git = fixture();
        assert_eq!(
            block_on(git.resolve("master")).unwrap(),
            Some("c3".to_owned())
        );
        assert_eq!(
            block_on(git.resolve("HEAD")).unwrap(),
            Some("f2".to_owned())
        );
        assert_eq!(block_on(git.resolve("c1")).unwrap(), Some("c1".to_owned()));
        assert_eq!(block_on(git.resolve("absent")).unwrap(), None);
    }

    #[test]
    fn fake_reports_default_branch() {
        assert_eq!(
            block_on(fixture().default_branch()).unwrap(),
            Some("master".to_owned())
        );
        assert_eq!(
            block_on(FakeGitHistory::new().default_branch()).unwrap(),
            None
        );
    }

    #[test]
    fn fake_reports_working_tree_dirtiness() {
        assert!(!block_on(fixture().is_dirty()).unwrap());
        let mut dirty = fixture();
        dirty.mark_dirty();
        assert!(block_on(dirty.is_dirty()).unwrap());
    }

    #[test]
    fn fake_first_parent_is_oldest_first() {
        let git = fixture();
        assert_eq!(
            block_on(git.first_parent("master")).unwrap(),
            vec!["c0", "c1", "c2", "c3"]
        );
        assert_eq!(
            block_on(git.first_parent("feature")).unwrap(),
            vec!["c0", "c1", "f1", "f2"]
        );
        assert!(block_on(git.first_parent("absent")).unwrap().is_empty());
    }

    #[test]
    fn fake_merge_base_is_the_branch_point() {
        let git = fixture();
        assert_eq!(
            block_on(git.merge_base("feature", "master")).unwrap(),
            Some("c1".to_owned())
        );
        // Symmetric, and a ref against itself bases at itself.
        assert_eq!(
            block_on(git.merge_base("master", "feature")).unwrap(),
            Some("c1".to_owned())
        );
        assert_eq!(
            block_on(git.merge_base("master", "master")).unwrap(),
            Some("c3".to_owned())
        );
    }

    #[test]
    fn fake_merge_base_is_none_for_disjoint_histories() {
        let mut git = FakeGitHistory::new();
        git.commit("a0", None)
            .commit("b0", None)
            .branch("a", "a0")
            .branch("b", "b0");
        assert_eq!(block_on(git.merge_base("a", "b")).unwrap(), None);
        assert_eq!(block_on(git.merge_base("a", "absent")).unwrap(), None);
    }
}
