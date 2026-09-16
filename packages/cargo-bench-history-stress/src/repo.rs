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
use std::process::{Output, Stdio};

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
// Process and pipe I/O is covered by stress_smoke; exit decisions remain unit-tested.
// Ref: docs/implementation.md, "Repository construction".
#[cfg_attr(test, mutants::skip)]
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
    check_git_output("fast-import", &output)
}

/// Reads Git's exported marks and delegates their interpretation.
// File acquisition is an integration boundary; parsing remains a mutation target.
#[cfg_attr(test, mutants::skip)]
async fn read_marks(marks_path: &Path) -> Result<HashMap<usize, String>, Error> {
    let text = tokio::fs::read_to_string(marks_path)
        .await
        .map_err(|error| fail(format!("failed to read the fast-import marks: {error}")))?;
    Ok(parse_marks(&text))
}

/// Parses the export-marks contents into a `mark -> commit ID` map.
fn parse_marks(text: &str) -> HashMap<usize, String> {
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
    marks
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
// Subprocess acquisition is covered by stress_smoke; check_git_output owns exit decisions.
#[cfg_attr(test, mutants::skip)]
async fn run_git(dir: &Path, args: &[&str]) -> Result<(), Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map_err(|error| fail(format!("failed to run git {}: {error}", args.join(" "))))?;
    check_git_output(&args.join(" "), &output)
}

/// Applies Git's completion status independently of subprocess acquisition.
fn check_git_output(subcommand: &str, output: &Output) -> Result<(), Error> {
    if !output.status.success() {
        return Err(fail(format!(
            "git {subcommand} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt as _;
    use std::process::ExitStatus;

    use super::*;

    #[test]
    fn successful_git_exit_accepts_diagnostic_output() {
        let output = Output {
            status: ExitStatus::from_raw(0),
            stdout: Vec::new(),
            stderr: b"progress on stderr".to_vec(),
        };
        check_git_output("fast-import", &output).unwrap();
    }

    #[test]
    fn unsuccessful_git_exit_rejects_empty_diagnostics() {
        // These represent nonzero Windows exits, or Unix signal and exit failures.
        for raw in [1, 256] {
            let output = Output {
                status: ExitStatus::from_raw(raw),
                stdout: Vec::new(),
                stderr: Vec::new(),
            };
            _ = check_git_output("fast-import", &output).unwrap_err();
        }
    }

    #[test]
    fn marks_preserve_identities_and_resolve_branch_order() {
        // Distinct full IDs and out-of-order marks distinguish missing/substituted
        // entries without depending on Git's export order.
        let first = "0123456789abcdef0123456789abcdef01234567";
        let second = "abcdef0123456789abcdef0123456789abcdef01";
        let marks = parse_marks(&format!(":2 {second}\n:1 {first}\n"));
        assert_eq!(
            marks,
            HashMap::from([(1, first.to_owned()), (2, second.to_owned())])
        );
        let repo = resolve_commits(&marks, &[ts(1_000)], &[ts(2_000)]).unwrap();
        assert_eq!(repo.main.len(), 1);
        assert_eq!(repo.feature.len(), 1);
        assert_eq!(repo.main[0].commit_id, first);
        assert_eq!(repo.main[0].time, ts(1_000));
        assert_eq!(repo.feature[0].commit_id, second);
        assert_eq!(repo.feature[0].time, ts(2_000));
    }

    #[test]
    fn missing_marks_fail_commit_resolution() {
        let marks = parse_marks(":1 main-id\n");
        _ = resolve_commits(&HashMap::new(), &[ts(1_000)], &[]).unwrap_err();
        _ = resolve_commits(&marks, &[ts(1_000)], &[ts(2_000)]).unwrap_err();
    }

    #[test]
    fn marks_parse_empty_and_unrecognized_lines() {
        assert!(parse_marks("").is_empty());
        assert!(parse_marks("\nnot-a-mark\n:invalid ignored\n").is_empty());
        assert_eq!(
            parse_marks("3 commit-id\n"),
            HashMap::from([(3, "commit-id".to_owned())])
        );
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
