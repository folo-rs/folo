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

use jiff::Timestamp;

use crate::process::capture;

/// A commit on a first-parent ancestry, paired with its committer timestamp and
/// its subject line.
///
/// `analyze` orders a series by first-parent topology and filters it by the
/// `--since`/`--until` window. The window is a per-commit property — a commit's
/// committer date — that topology alone decides, letting out-of-window objects be
/// skipped before any stored object is fetched. `committer_time` is `None` only
/// when `git` emitted an unparseable date, which a real commit never does; such a
/// commit is treated as in-window (never excluded by the window).
///
/// `subject` is the commit's title (`git`'s `%s`), harvested on the same walk so
/// `examine` can label each data point with what its commit changed. It is empty
/// when `git` reported no subject; commands other than `examine` ignore it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FirstParentCommit {
    /// The commit's full SHA.
    pub(crate) sha: String,
    /// The commit's committer timestamp (`git`'s `%cI`), or `None` when absent or
    /// unparseable.
    pub(crate) committer_time: Option<Timestamp>,
    /// The commit's subject line (`git`'s `%s`), or empty when absent.
    pub(crate) subject: String,
}

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
    /// series by. Each commit carries its committer timestamp, which `analyze`
    /// uses to apply the `--since`/`--until` window before fetching any object,
    /// and its subject line, which `examine` uses to label each data point.
    /// An unresolvable ref yields an empty list.
    fn first_parent(
        &self,
        reference: &str,
    ) -> impl Future<Output = io::Result<Vec<FirstParentCommit>>>;

    /// Returns the committer timestamp of the commit `reference` resolves to, read
    /// from that one commit alone — no first-parent history walk.
    ///
    /// `analyze`/`list` use this to date a single commit (for example `HEAD`, when
    /// reporting the blessings recorded there) without paying for the whole
    /// ancestry. `Ok(None)` when the ref does not resolve or carries no parseable
    /// committer date.
    fn committer_time(
        &self,
        reference: &str,
    ) -> impl Future<Output = io::Result<Option<Timestamp>>>;

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

    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; parsing delegated to `parse_first_parent_log`.
    async fn first_parent(&self, reference: &str) -> io::Result<Vec<FirstParentCommit>> {
        // `%H`, `%cI`, `%s` pair each first-parent commit with its committer date
        // (strict ISO 8601) and subject, so the window can be decided from
        // topology before any fetch and `examine` can label each point. `-z`
        // NUL-terminates each commit's record and the fields are NUL-delimited
        // (`%x00`): NUL cannot occur in git object content, so no field — the
        // subject, which may contain any printable character, included — can be
        // confused with a delimiter.
        let output = self
            .run(&[
                "log",
                "--first-parent",
                "--reverse",
                "-z",
                "--format=%H%x00%cI%x00%s",
                reference,
            ])
            .await?;
        Ok(output
            .map(|stdout| parse_first_parent_log(&stdout))
            .unwrap_or_default())
    }

    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; parsing delegated to `parse_committer_time`.
    async fn committer_time(&self, reference: &str) -> io::Result<Option<Timestamp>> {
        // `-1` reads a single commit, so the date is fetched without walking history.
        let output = self.run(&["log", "-1", "--format=%cI", reference]).await?;
        Ok(output.and_then(|stdout| parse_committer_time(&stdout)))
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

/// Parses `git log --first-parent --reverse -z --format=%H%x00%cI%x00%s` output
/// into [`FirstParentCommit`]s in stream order. `-z` NUL-terminates each commit's
/// record and the three fields are NUL-delimited, so the stream is a flat run of
/// `<sha>\0<committer-date>\0<subject>` triples with a trailing empty token from
/// the final terminator. A field carrying an unparseable date yields
/// `committer_time: None` and an empty subject field yields an empty string. With
/// `--reverse` the input is oldest-first.
fn parse_first_parent_log(stdout: &str) -> Vec<FirstParentCommit> {
    let mut fields = stdout.split('\0');
    let mut commits = Vec::new();
    while let Some(sha) = fields.next() {
        let sha = sha.trim();
        // A real commit's SHA is never empty; the only empty leading token is the
        // one following the final record's terminator, which ends the stream.
        if sha.is_empty() {
            break;
        }
        let committer_time = fields
            .next()
            .and_then(|field| field.trim().parse::<Timestamp>().ok());
        let subject = fields.next().unwrap_or_default().to_owned();
        commits.push(FirstParentCommit {
            sha: sha.to_owned(),
            committer_time,
            subject,
        });
    }
    commits
}

/// Parses `git log -1 --format=%cI` output into a single committer [`Timestamp`]:
/// the first non-empty trimmed line parsed as strict ISO 8601, or `None` when
/// there is none (an unresolved ref) or it does not parse.
fn parse_committer_time(stdout: &str) -> Option<Timestamp> {
    stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(|line| line.parse::<Timestamp>().ok())
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

    use jiff::Timestamp;

    use super::{FirstParentCommit, GitHistory};

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
        /// Commit SHA -> its committer timestamp, for commits seeded with one.
        times: HashMap<String, Timestamp>,
        /// Commit SHA -> its subject line, for commits seeded with one.
        subjects: HashMap<String, String>,
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
                times: HashMap::new(),
                subjects: HashMap::new(),
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

        /// Records a commit `sha` (first `parent`, `None` = root) carrying a
        /// committer timestamp, so the topology window can be exercised.
        pub(crate) fn commit_at(
            &mut self,
            sha: &str,
            parent: Option<&str>,
            time: Timestamp,
        ) -> &mut Self {
            self.commit(sha, parent);
            self.times.insert(sha.to_owned(), time);
            self
        }

        /// Records the subject line of a commit, so `examine`'s point labeling can
        /// be exercised. The commit itself must be recorded separately (via
        /// [`commit`](Self::commit) or [`commit_at`](Self::commit_at)).
        pub(crate) fn subject(&mut self, sha: &str, subject: &str) -> &mut Self {
            self.subjects.insert(sha.to_owned(), subject.to_owned());
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

        fn first_parent(
            &self,
            reference: &str,
        ) -> impl Future<Output = io::Result<Vec<FirstParentCommit>>> {
            let Some(sha) = self.resolve_sync(reference) else {
                return ready(Ok(Vec::new()));
            };
            let mut chain = self.chain(&sha);
            chain.reverse(); // oldest-first, matching `git log --reverse`.
            let commits = chain
                .into_iter()
                .map(|sha| FirstParentCommit {
                    committer_time: self.times.get(&sha).copied(),
                    subject: self.subjects.get(&sha).cloned().unwrap_or_default(),
                    sha,
                })
                .collect();
            ready(Ok(commits))
        }

        fn committer_time(
            &self,
            reference: &str,
        ) -> impl Future<Output = io::Result<Option<Timestamp>>> {
            let time = self
                .resolve_sync(reference)
                .and_then(|sha| self.times.get(&sha).copied());
            ready(Ok(time))
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
    fn parse_first_parent_log_keeps_order_times_subjects_and_drops_blanks() {
        // `-z` NUL-terminates each `<sha>\0<date>\0<subject>` record, so the input
        // is a flat NUL-separated run of triples ending in a terminator.
        let parsed = parse_first_parent_log(
            "c0\x002024-01-01T00:00:00+00:00\x00First commit\x00c1\x002024-02-01T00:00:00+00:00\x00Fix the thing\x00c2\x002024-03-01T00:00:00+00:00\x00Subject with spaces  \x00",
        );
        assert_eq!(
            parsed,
            vec![
                FirstParentCommit {
                    sha: "c0".to_owned(),
                    committer_time: Some("2024-01-01T00:00:00+00:00".parse().unwrap()),
                    subject: "First commit".to_owned(),
                },
                FirstParentCommit {
                    sha: "c1".to_owned(),
                    committer_time: Some("2024-02-01T00:00:00+00:00".parse().unwrap()),
                    subject: "Fix the thing".to_owned(),
                },
                FirstParentCommit {
                    sha: "c2".to_owned(),
                    committer_time: Some("2024-03-01T00:00:00+00:00".parse().unwrap()),
                    subject: "Subject with spaces  ".to_owned(),
                },
            ]
        );
        // An unparseable date is timeless; an empty subject field yields an empty
        // subject.
        assert_eq!(
            parse_first_parent_log(
                "c0\x002024-01-01T00:00:00+00:00\x00\x00c1\x00not-a-date\x00has subject\x00"
            ),
            vec![
                FirstParentCommit {
                    sha: "c0".to_owned(),
                    committer_time: Some("2024-01-01T00:00:00+00:00".parse().unwrap()),
                    subject: String::new(),
                },
                FirstParentCommit {
                    sha: "c1".to_owned(),
                    committer_time: None,
                    subject: "has subject".to_owned(),
                },
            ]
        );
        assert!(parse_first_parent_log("").is_empty());
        assert!(parse_first_parent_log("\x00").is_empty());
    }

    #[test]
    fn parse_first_parent_log_preserves_delimiter_like_bytes_in_subjects() {
        // NUL-delimiting (rather than the unit separator `\x1f`) keeps a subject
        // that happens to contain the old delimiter intact: only NUL, which git
        // object content can never hold, separates fields.
        assert_eq!(
            parse_first_parent_log("c0\x002024-01-01T00:00:00+00:00\x00Weird\u{1f}subject\x00"),
            vec![FirstParentCommit {
                sha: "c0".to_owned(),
                committer_time: Some("2024-01-01T00:00:00+00:00".parse().unwrap()),
                subject: "Weird\u{1f}subject".to_owned(),
            }]
        );
    }

    #[test]
    fn parse_committer_time_takes_first_non_empty_line() {
        assert_eq!(
            parse_committer_time("  \n 2024-03-01T00:00:00+00:00 \n ignored \n"),
            Some("2024-03-01T00:00:00+00:00".parse().unwrap())
        );
        assert_eq!(parse_committer_time("not-a-date\n"), None);
        assert_eq!(parse_committer_time("   \n  \n"), None);
        assert_eq!(parse_committer_time(""), None);
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
        let shas = |reference: &str| {
            block_on(git.first_parent(reference))
                .unwrap()
                .into_iter()
                .map(|commit| commit.sha)
                .collect::<Vec<_>>()
        };
        assert_eq!(shas("master"), vec!["c0", "c1", "c2", "c3"]);
        assert_eq!(shas("feature"), vec!["c0", "c1", "f1", "f2"]);
        assert!(block_on(git.first_parent("absent")).unwrap().is_empty());
    }

    #[test]
    fn fake_first_parent_carries_committer_times_and_subjects() {
        let mut git = FakeGitHistory::new();
        let t0: Timestamp = "2024-01-01T00:00:00+00:00".parse().unwrap();
        let t1: Timestamp = "2024-02-01T00:00:00+00:00".parse().unwrap();
        git.commit_at("c0", None, t0)
            .commit_at("c1", Some("c0"), t1)
            .commit("c2", Some("c1")) // A commit seeded without a time stays None.
            .subject("c0", "Initial commit")
            .subject("c1", "Fix the thing")
            .branch("master", "c2")
            .head("master");
        assert_eq!(
            block_on(git.first_parent("master")).unwrap(),
            vec![
                FirstParentCommit {
                    sha: "c0".to_owned(),
                    committer_time: Some(t0),
                    subject: "Initial commit".to_owned(),
                },
                FirstParentCommit {
                    sha: "c1".to_owned(),
                    committer_time: Some(t1),
                    subject: "Fix the thing".to_owned(),
                },
                FirstParentCommit {
                    // A commit seeded without a subject carries an empty one.
                    sha: "c2".to_owned(),
                    committer_time: None,
                    subject: String::new(),
                },
            ]
        );
    }

    #[test]
    fn fake_committer_time_reads_a_single_commit() {
        let mut git = FakeGitHistory::new();
        let t0: Timestamp = "2024-01-01T00:00:00+00:00".parse().unwrap();
        let t1: Timestamp = "2024-02-01T00:00:00+00:00".parse().unwrap();
        git.commit_at("c0", None, t0)
            .commit_at("c1", Some("c0"), t1)
            .commit("c2", Some("c1")) // Seeded without a time.
            .branch("master", "c1")
            .head("master");
        // Resolves a ref to its commit's time, and a raw SHA likewise.
        assert_eq!(block_on(git.committer_time("HEAD")).unwrap(), Some(t1));
        assert_eq!(block_on(git.committer_time("c0")).unwrap(), Some(t0));
        // A commit without a recorded time, and an unresolved ref, are `None`.
        assert_eq!(block_on(git.committer_time("c2")).unwrap(), None);
        assert_eq!(block_on(git.committer_time("absent")).unwrap(), None);
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
