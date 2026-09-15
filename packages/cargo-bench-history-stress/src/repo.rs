//! Builds the synthetic git repository the analysis resolves topology from.
//!
//! The whole history is created in one `git fast-import` stream: a `main` branch
//! of dated first-parent commits plus a feature branch forked from `main`'s tip.
//! `analyze` shells out to real `git` against this repository, so the commit IDs
//! the seeded storage keys are named by must be the SHAs `git rev-list` reports —
//! hence the import runs first and the SHAs are read back from the export-marks
//! file before any storage object is written.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use jiff::Timestamp;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::error::{Error, fail};
use crate::logging::Logger;
use crate::scenario::{BRANCH_FEATURE, BRANCH_MAIN};

/// A single seeded commit: its commit ID and the committer date it carries.
#[derive(Clone, Debug)]
pub(crate) struct Commit {
    /// Full 40-character commit ID, as `git rev-list` reports it.
    pub(crate) commit_id: String,
    /// Committer date, mirrored into each run's context for `--since` windowing.
    pub(crate) time: Timestamp,
}

/// The seeded repository's commit topology, by branch.
#[derive(Debug)]
pub(crate) struct SeededRepo {
    /// `main` commits, oldest first.
    pub(crate) main: Vec<Commit>,
    /// Feature-branch commits, oldest first (forked from `main`'s tip).
    pub(crate) feature: Vec<Commit>,
}

/// Builds the repository at `dir` from the given committer dates and reads the
/// resulting SHAs back.
///
/// `main_times` and `feature_times` are the committer dates, oldest first. The
/// feature branch forks from `main`'s tip.
///
/// # Errors
///
/// Returns an error if any `git` invocation fails.
pub(crate) async fn build_repo(
    dir: &Path,
    marks_path: &Path,
    main_times: &[Timestamp],
    feature_times: &[Timestamp],
    logger: Logger,
) -> Result<SeededRepo, Error> {
    logger.step(&format!(
        "building synthetic git repository: {} main commits + {} feature commits",
        main_times.len(),
        feature_times.len()
    ));
    logger.detail_with(|| {
        "the whole history is one fast-import stream because analyze resolves topology from real \
         git, so storage keys must be named by the SHAs git assigns"
            .to_owned()
    });

    run_git(dir, &["init", "-q", "-b", BRANCH_MAIN, "."]).await?;

    let stream = build_stream(main_times, feature_times);
    import_stream(dir, marks_path, &stream).await?;

    // Materialize main's working tree so `git status` is clean: fast-import writes
    // refs and objects but leaves the index and work tree empty, which git would
    // otherwise report as a tree full of deletions (a dirty tree). The marks file
    // lives outside the work tree so it never shows up as an untracked file.
    run_git(dir, &["reset", "--hard", BRANCH_MAIN]).await?;
    logger.detail_with(|| {
        "reset the work tree to main so analyze sees a clean (non-dirty) checkout".to_owned()
    });

    let marks = read_marks(marks_path).await?;
    let repo = resolve_commits(&marks, main_times, feature_times)?;
    logger.detail_with(|| {
        format!(
            "read back {} commit IDs from the export-marks file",
            repo.main.len().saturating_add(repo.feature.len())
        )
    });
    Ok(repo)
}

/// Assembles the fast-import stream for the whole history.
fn build_stream(main_times: &[Timestamp], feature_times: &[Timestamp]) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    let main_count = main_times.len();

    for (i, time) in main_times.iter().enumerate() {
        let mark = i.saturating_add(1);
        let global = i;
        // main commits chain automatically: fast-import continues a branch from
        // its previous commit when no `from` is given, so only the very first
        // commit is a root and none need an explicit parent.
        append_commit(&mut buf, BRANCH_MAIN, mark, None, *time, global);
    }

    if !feature_times.is_empty() {
        let branch_point_mark = main_count; // mark of main's tip commit
        for (j, time) in feature_times.iter().enumerate() {
            let mark = main_count.saturating_add(j).saturating_add(1);
            let global = main_count.saturating_add(j);
            // Only the first feature commit needs an explicit parent (the fork
            // point); the rest chain on the feature branch automatically.
            let from = (j == 0).then_some(branch_point_mark);
            append_commit(&mut buf, BRANCH_FEATURE, mark, from, *time, global);
        }
    }

    buf
}

/// Appends one commit (with an inline file change) to the fast-import stream.
fn append_commit(
    buf: &mut Vec<u8>,
    branch: &str,
    mark: usize,
    from: Option<usize>,
    time: Timestamp,
    global_index: usize,
) {
    let message = format!("commit {global_index}\n");
    let content = format!("state {global_index}\n");

    buf.extend_from_slice(format!("commit refs/heads/{branch}\n").as_bytes());
    buf.extend_from_slice(format!("mark :{mark}\n").as_bytes());
    buf.extend_from_slice(
        format!(
            "committer Stress Bot <stress@example.invalid> {} +0000\n",
            time.as_second()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(format!("data {}\n", message.len()).as_bytes());
    buf.extend_from_slice(message.as_bytes());
    if let Some(parent) = from {
        buf.extend_from_slice(format!("from :{parent}\n").as_bytes());
    }
    buf.extend_from_slice(b"M 100644 inline history/state.txt\n");
    buf.extend_from_slice(format!("data {}\n", content.len()).as_bytes());
    buf.extend_from_slice(content.as_bytes());
    buf.extend_from_slice(b"\n");
}

/// Feeds the stream to `git fast-import`, writing SHAs to the marks file.
async fn import_stream(dir: &Path, marks_path: &Path, stream: &[u8]) -> Result<(), Error> {
    let marks_arg = format!("--export-marks={}", marks_path.display());
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["fast-import", "--quiet", "--force"])
        .arg(&marks_arg)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| fail(format!("failed to spawn git fast-import: {error}")))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| fail("git fast-import stdin was not captured"))?;
        stdin
            .write_all(stream)
            .await
            .map_err(|error| fail(format!("failed to write the fast-import stream: {error}")))?;
        stdin
            .shutdown()
            .await
            .map_err(|error| fail(format!("failed to close the fast-import stream: {error}")))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|error| fail(format!("git fast-import did not complete: {error}")))?;
    if !output.status.success() {
        return Err(fail(format!(
            "git fast-import failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// Parses the export-marks file into a `mark -> commit ID` map.
async fn read_marks(marks_path: &Path) -> Result<HashMap<usize, String>, Error> {
    let text = tokio::fs::read_to_string(marks_path)
        .await
        .map_err(|error| fail(format!("failed to read the fast-import marks: {error}")))?;
    let mut marks = HashMap::new();
    for line in text.lines() {
        // Each line is ":<mark> <commit_id>".
        let Some((mark, commit_id)) = line.split_once(' ') else {
            continue;
        };
        let mark = mark.strip_prefix(':').unwrap_or(mark);
        if let Ok(mark) = mark.parse::<usize>() {
            marks.insert(mark, commit_id.to_owned());
        }
    }
    Ok(marks)
}

/// Resolves the `mark -> commit ID` map back into per-branch commit lists.
fn resolve_commits(
    marks: &HashMap<usize, String>,
    main_times: &[Timestamp],
    feature_times: &[Timestamp],
) -> Result<SeededRepo, Error> {
    let main_count = main_times.len();
    let lookup = |mark: usize| -> Result<String, Error> {
        marks.get(&mark).cloned().ok_or_else(|| {
            fail(format!(
                "fast-import did not report a commit ID for mark :{mark}"
            ))
        })
    };

    let mut main = Vec::with_capacity(main_count);
    for (i, time) in main_times.iter().enumerate() {
        main.push(Commit {
            commit_id: lookup(i.saturating_add(1))?,
            time: *time,
        });
    }

    let mut feature = Vec::with_capacity(feature_times.len());
    for (j, time) in feature_times.iter().enumerate() {
        feature.push(Commit {
            commit_id: lookup(main_count.saturating_add(j).saturating_add(1))?,
            time: *time,
        });
    }

    Ok(SeededRepo { main, feature })
}

/// Runs a `git` subcommand in `dir`, failing on a non-zero exit.
async fn run_git(dir: &Path, args: &[&str]) -> Result<(), Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map_err(|error| fail(format!("failed to run git {}: {error}", args.join(" "))))?;
    if !output.status.success() {
        return Err(fail(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::future::Future;

    use testing::with_watchdog;
    use tokio::runtime::Builder;

    use super::*;

    // These helpers own real Git and filesystem I/O. Tiny native library tests keep that
    // boundary observable during mutation testing without running the stress pipeline.
    // Ref: ../docs/implementation.md, "Repository construction".
    fn with_runtime(test: impl Future<Output = ()> + Send + 'static) {
        with_watchdog(|| {
            Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(test);
        });
    }

    #[test]
    #[cfg_attr(miri, ignore = "creates a repository through a real Git subprocess")]
    fn run_git_creates_a_repository() {
        with_runtime(async {
            let dir = tempfile::tempdir().unwrap();

            run_git(dir.path(), &["init", "-q", "-b", BRANCH_MAIN, "."])
                .await
                .unwrap();

            assert!(dir.path().join(".git").is_dir());
        });
    }

    #[test]
    #[cfg_attr(miri, ignore = "observes a real Git subprocess exit status")]
    fn run_git_rejects_a_failed_command() {
        with_runtime(async {
            let dir = tempfile::tempdir().unwrap();

            // A missing -C directory fails inside Git, after the process has spawned.
            assert!(
                run_git(&dir.path().join("missing"), &["status"])
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "imports and queries history through real Git subprocesses"
    )]
    fn import_stream_exports_the_commits_git_created() {
        with_runtime(async {
            let dir = tempfile::tempdir().unwrap();
            // Spaces exercise the export-marks argument as one path, not shell words.
            let marks_path = dir.path().join("exported marks");
            run_git(dir.path(), &["init", "-q", "-b", BRANCH_MAIN, "."])
                .await
                .unwrap();
            // One commit per branch is enough to distinguish their exported identities.
            let stream = build_stream(&[ts(1_000)], &[ts(2_000)]);

            import_stream(dir.path(), &marks_path, &stream)
                .await
                .unwrap();

            let output = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(["rev-parse", BRANCH_MAIN, BRANCH_FEATURE])
                .output()
                .await
                .unwrap();
            assert!(output.status.success());
            let text = String::from_utf8(output.stdout).unwrap();
            let commits: Vec<_> = text.lines().collect();
            let [main, feature] = commits.as_slice() else {
                panic!();
            };
            assert_ne!(main, feature);
            assert_eq!(
                read_marks(&marks_path).await.unwrap(),
                HashMap::from([(1, (*main).to_owned()), (2, (*feature).to_owned())])
            );
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "observes a real Git fast-import subprocess exit status"
    )]
    fn import_stream_rejects_a_failed_exit() {
        with_runtime(async {
            let dir = tempfile::tempdir().unwrap();

            // An empty stream avoids a broken-pipe write racing with Git's rejection of the
            // missing -C directory. The error must come from the completed subprocess.
            assert!(
                import_stream(&dir.path().join("missing"), &dir.path().join("marks"), b"")
                    .await
                    .is_err()
            );
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "reads an exported-marks file through Tokio filesystem I/O"
    )]
    fn read_marks_preserves_all_mark_identities() {
        with_runtime(async {
            let dir = tempfile::tempdir().unwrap();
            let marks_path = dir.path().join("marks");
            // Distinct full-length IDs and out-of-order marks expose dropped or substituted
            // entries without relying on the order in which Git exports them.
            let first = "0123456789abcdef0123456789abcdef01234567";
            let second = "abcdef0123456789abcdef0123456789abcdef01";
            tokio::fs::write(&marks_path, format!(":2 {second}\n:1 {first}\n"))
                .await
                .unwrap();

            assert_eq!(
                read_marks(&marks_path).await.unwrap(),
                HashMap::from([(1, first.to_owned()), (2, second.to_owned())])
            );
        });
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "reads exported-marks files through Tokio filesystem I/O"
    )]
    fn read_marks_distinguishes_an_empty_file_from_a_missing_file() {
        with_runtime(async {
            let dir = tempfile::tempdir().unwrap();
            let marks_path = dir.path().join("marks");

            assert!(read_marks(&marks_path).await.is_err());
            tokio::fs::write(&marks_path, "").await.unwrap();
            assert!(read_marks(&marks_path).await.unwrap().is_empty());
        });
    }

    fn ts(second: i64) -> Timestamp {
        Timestamp::from_second(second).unwrap()
    }

    #[test]
    fn only_the_first_feature_commit_forks_from_main() {
        // Three feature commits make the fork-point logic observable: only the
        // first carries an explicit `from`, pinning it to main's tip (mark :2),
        // while the rest chain implicitly and no main commit names a parent.
        let main = vec![ts(1_000), ts(2_000)];
        let feature = vec![ts(3_000), ts(4_000), ts(5_000)];

        let stream = build_stream(&main, &feature);
        let text = String::from_utf8(stream).unwrap();

        let from_lines: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("from :"))
            .collect();
        assert_eq!(from_lines, vec!["from :2"]);
    }
}
