//! The `bless` / `unbless` commands: manually accept (or revoke acceptance of) a
//! benchmark's level on the base branch, so history analysis stops re-flagging an
//! intentional change.
//!
//! `bless` writes an append-only [`BlessingRecord`](crate::model::BlessingRecord)
//! sidecar into every facet-selected discriminant set that has a stored result at
//! the context commit (`HEAD` by default, or `--context <ref>`). It is
//! base-branch-only with no escape hatch: a context commit that is not on the base
//! branch, or the absence of a stored result there, are hard errors, because a
//! blessing on anything else would not survive a history analysis (see the
//! `bless` / `unbless` command in `DESIGN.md`). When blessing `HEAD`, a dirty
//! working tree is allowed — the blessing
//! applies to the committed `clean.json` recorded at `HEAD`, which the local edits
//! do not change — but it emits a warning. `unbless` deletes every blessing
//! recorded at the context commit in the selected sets; sidecars are immutable, so
//! narrowing a blessing means unblessing and re-blessing the subset to keep.
//! Blessings issued at later commits are unaffected, so the timeline can stay
//! blessed past the context commit.

use std::path::Path;

use jiff::Timestamp;

use crate::config::{Config, load_config};
use crate::git_history::{GitHistory, SystemGitHistory};
use crate::model::BlessingRecord;
use crate::model::Run;
use crate::report::{Reporter, StderrReporter};
use crate::storage::{Storage, build_storage};
use crate::text::count_noun;
use crate::wiring::{resolve_config_path, resolve_project_id, resolve_repo};
use crate::{BlessOptions, RunError, RunOutcome, UnblessOptions};

use super::StorageKey;
use super::{
    AutoFacets, Selection, detect_auto_facets, facet_filtered_candidates, resolve_base_ref,
    resolve_facets,
};

/// The real `bless`: load configuration, wire the configured storage and git
/// history, and orchestrate.
///
/// `now_override` pins the issue time so end-to-end tests are deterministic;
/// production passes `None` and uses the wall clock.
pub(crate) async fn bless(
    options: &BlessOptions,
    workspace_dir: &Path,
    now_override: Option<Timestamp>,
) -> Result<RunOutcome, RunError> {
    let reporter = StderrReporter::new(options.verbose);

    let config_path = resolve_config_path(workspace_dir, options.config_path.as_deref());
    reporter.note(&format!(
        "loading configuration from {}",
        config_path.display()
    ));
    let config = load_config(&config_path).await?;

    let project_id = resolve_project_id(&config, workspace_dir);
    let storage = build_storage(&config, workspace_dir)?;

    let git = SystemGitHistory::new(resolve_repo(workspace_dir, options.repo.as_deref()));
    let auto = detect_auto_facets().await?;

    let now = now_override.unwrap_or_else(Timestamp::now);
    bless_with(
        &git,
        &storage,
        &project_id,
        &config,
        options,
        &auto,
        now,
        env!("CARGO_PKG_VERSION"),
        &reporter,
    )
    .await
}

/// The real `unbless`: load configuration, wire the configured storage and git
/// history, and orchestrate.
pub(crate) async fn unbless(
    options: &UnblessOptions,
    workspace_dir: &Path,
) -> Result<RunOutcome, RunError> {
    let reporter = StderrReporter::new(options.verbose);

    let config_path = resolve_config_path(workspace_dir, options.config_path.as_deref());
    reporter.note(&format!(
        "loading configuration from {}",
        config_path.display()
    ));
    let config = load_config(&config_path).await?;

    let project_id = resolve_project_id(&config, workspace_dir);
    let storage = build_storage(&config, workspace_dir)?;

    let git = SystemGitHistory::new(resolve_repo(workspace_dir, options.repo.as_deref()));
    let auto = detect_auto_facets().await?;

    unbless_with(
        &git,
        &storage,
        &project_id,
        &config,
        options,
        &auto,
        &reporter,
    )
    .await
}

/// Storage- and git-generic `bless`: validate the preconditions, then write a
/// blessing sidecar into every facet-selected set that has a clean result at the
/// current commit.
#[expect(
    clippy::too_many_arguments,
    reason = "blessing wires several injected ports plus the pinned issue time and tool version"
)]
pub(crate) async fn bless_with<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    config: &Config,
    options: &BlessOptions,
    auto: &AutoFacets,
    now: Timestamp,
    tool_version: &str,
    reporter: &dyn Reporter,
) -> Result<RunOutcome, RunError>
where
    G: GitHistory,
    S: Storage,
{
    let prefixes = if options.all {
        // An empty prefix list accepts every benchmark, so `--all` blesses the
        // whole commit.
        Vec::new()
    } else if options.prefixes.is_empty() {
        return Err(RunError::Bless {
            message: "at least one benchmark-id prefix is required (or pass --all); for example \
                      `bless all_the_time/read_cell`"
                .to_owned(),
        });
    } else {
        options.prefixes.clone()
    };

    let context = options.context.as_deref().unwrap_or("HEAD");
    let head = resolve_commit(git, context).await?;
    let short = short_sha(&head);

    // Blessing is base-branch-only: a feature-branch blessing would silently
    // vanish (or duplicate) once the branch is squash-merged, so it is refused
    // outright with no `--force` escape hatch.
    let base = resolve_base_ref(git, config, options.base.as_deref())
        .await?
        .ok_or_else(|| RunError::Bless {
            message: "could not determine the base branch; specify it with --base".to_owned(),
        })?;
    let on_base = git
        .merge_base(&head, &base)
        .await
        .map_err(RunError::Io)?
        .as_deref()
        == Some(head.as_str());
    if !on_base {
        return Err(RunError::Bless {
            message: format!(
                "the context commit {short} is not on the base branch {}; blessings are only \
                 allowed on the base branch, since a feature-branch blessing would not survive \
                 a squash merge",
                short_sha(&base)
            ),
        });
    }

    // A blessing accepts the *committed* level recorded at the context commit
    // (`clean.json`), so a dirty working tree does not change which data point is
    // blessed — the local edits are simply irrelevant. Warn rather than refuse, so
    // an accidental uncommitted edit does not block blessing an already-recorded
    // clean run. The warning is only relevant when blessing the checked-out commit.
    let working_tree_dirty =
        options.context.is_none() && git.is_dirty().await.map_err(RunError::Io)?;

    let selection = Selection::from_bless(options);
    let facets = resolve_facets(&selection, Some(auto))?;
    let candidates = facet_filtered_candidates(storage, project_id, &facets, reporter).await?;
    let clean_at_head: Vec<(String, StorageKey)> = candidates
        .into_iter()
        .filter(|(_, parsed)| parsed.commit == head && parsed.file == "clean.json")
        .collect();
    if clean_at_head.is_empty() {
        return Err(RunError::Bless {
            message: format!(
                "no stored result at the context commit {short}; record a run there before \
                 blessing (a blessing accepts an existing data point)"
            ),
        });
    }

    let issued_unix = now.as_second();
    let mut sets = 0_usize;
    for (clean_key, parsed) in &clean_at_head {
        // The blessed commit's commit time is the committer date already recorded
        // on its run, so the blessing labels and anchors consistently without a
        // separate probe.
        let commit_time = load_commit_time(storage, clean_key).await?;
        let record = BlessingRecord::new(
            head.clone(),
            commit_time,
            now,
            prefixes.clone(),
            tool_version.to_owned(),
        );
        let json = record
            .to_json()
            .expect("a freshly built blessing always serializes to JSON");
        let bless_key = parsed.bless_key(issued_unix);
        storage
            .put_overwrite(&bless_key, json.as_bytes())
            .await
            .map_err(RunError::Storage)?;
        reporter.note(&format!("blessed set {} at {bless_key}", parsed.set));
        sets = sets.saturating_add(1);
    }

    let scope = if options.all {
        "all benchmarks".to_owned()
    } else {
        count_noun(prefixes.len(), "prefix filter")
    };
    let message = format!(
        "{}Blessed {scope} across {} at commit {short}.",
        if working_tree_dirty {
            format!(
                "Warning: uncommitted changes present. Blessing was applied to the existing \
                 commit at HEAD ({short}).\n"
            )
        } else {
            String::new()
        },
        count_noun(sets, "discriminant set"),
    );
    Ok(RunOutcome::Completed { message })
}

/// Storage- and git-generic `unbless`: delete every blessing recorded at the
/// current commit in the facet-selected sets.
pub(crate) async fn unbless_with<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    _config: &Config,
    options: &UnblessOptions,
    auto: &AutoFacets,
    reporter: &dyn Reporter,
) -> Result<RunOutcome, RunError>
where
    G: GitHistory,
    S: Storage,
{
    let context = options.context.as_deref().unwrap_or("HEAD");
    let head = resolve_commit(git, context).await?;
    let short = short_sha(&head);

    let selection = Selection::from_unbless(options);
    let facets = resolve_facets(&selection, Some(auto))?;
    let candidates = facet_filtered_candidates(storage, project_id, &facets, reporter).await?;
    let blessings_at_head: Vec<String> = candidates
        .into_iter()
        .filter(|(_, parsed)| parsed.commit == head && parsed.is_bless())
        .map(|(key, _)| key)
        .collect();

    let mut removed = 0_usize;
    for key in &blessings_at_head {
        storage.delete(key).await.map_err(RunError::Storage)?;
        reporter.note(&format!("removed blessing {key}"));
        removed = removed.saturating_add(1);
    }

    let message = if removed == 0 {
        format!("No blessings recorded at commit {short}.")
    } else {
        format!(
            "Removed {} at commit {short}.",
            count_noun(removed, "blessing")
        )
    };
    Ok(RunOutcome::Completed { message })
}

/// Resolves a context ref (for example `HEAD` or a commit SHA) to a full commit
/// SHA, mapping an unresolvable ref (not a repository, or an unknown ref) to a
/// clear blessing error.
async fn resolve_commit<G: GitHistory>(git: &G, reference: &str) -> Result<String, RunError> {
    git.resolve(reference)
        .await
        .map_err(RunError::Io)?
        .ok_or_else(|| RunError::Bless {
            message: format!(
                "could not resolve {reference}; run this inside a git repository (or pass --repo) \
                 and check the ref exists"
            ),
        })
}

/// Reads the commit time (committer date) recorded on the clean result at `key`.
async fn load_commit_time<S: Storage>(storage: &S, key: &str) -> Result<Timestamp, RunError> {
    let bytes = storage.get(key).await.map_err(RunError::Storage)?;
    let text = String::from_utf8(bytes).map_err(|error| RunError::Bless {
        message: format!("stored object {key} is not valid UTF-8: {error}"),
    })?;
    let result = Run::from_json(&text).map_err(|error| RunError::Bless {
        message: format!("stored object {key} is not a valid result set: {error}"),
    })?;
    Ok(result.context.commit)
}

/// The first twelve characters of a SHA (all of it when shorter), for messages.
fn short_sha(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
    use futures::executor::block_on;

    use crate::git_history::FakeGitHistory;
    use crate::model::{BenchmarkId, BenchmarkIdPrefix, BenchmarkResult, Metric, MetricKind, Run};
    use crate::model::{EnvironmentInfo, GitInfo, RunContext, ToolchainInfo};
    use crate::report::RecordingReporter;
    use crate::storage::MemoryStorage;

    use nonempty::nonempty;

    use super::*;

    fn config() -> Config {
        crate::config::parse_config("[storage.local]\npath = \"./data\"\n").unwrap()
    }

    /// The auto-detected facets for the default synthetic partition the tests seed.
    fn auto() -> AutoFacets {
        AutoFacets {
            triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine_key: "synthetic".to_owned(),
        }
    }

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    /// A serialized clean result set at `commit`, ready to seed storage.
    fn clean_run_json(commit: &str, effective: i64) -> String {
        let time = ts(effective);
        let context = RunContext::new(
            time,
            time,
            GitInfo {
                commit: Some(commit.to_owned()),
                short_commit: Some(commit.to_owned()),
                branch: Some("master".to_owned()),
                dirty: false,
            },
            EnvironmentInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        let record = BenchmarkResult::new(
            BenchmarkId::new(nonempty!["all_the_time".to_owned(), "read_cell".to_owned()]),
            vec![Metric::new(MetricKind::InstructionCount, 100.0)],
        );
        Run::new(context, vec![record]).to_json().unwrap()
    }

    fn clean_key(commit: &str) -> String {
        format!("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/{commit}/clean.json")
    }

    /// A linear master history `c0 - c1 - c2`, HEAD at the tip `c2`.
    fn master_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .branch("master", "c2")
            .head("master")
            .mark_default("master");
        git
    }

    fn bless_options(prefixes: &[&str]) -> BlessOptions {
        BlessOptions {
            prefixes: prefixes
                .iter()
                .map(|prefix| BenchmarkIdPrefix::new(*prefix).unwrap())
                .collect(),
            ..BlessOptions::default()
        }
    }

    /// All blessing sidecar keys stored under the project partition.
    fn stored_blessings(storage: &MemoryStorage) -> Vec<String> {
        let mut keys = block_on(storage.list("v2/folo/")).unwrap();
        keys.retain(|key| {
            key.rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with("bless-"))
        });
        keys.sort();
        keys
    }

    fn drive_bless(
        storage: &MemoryStorage,
        git: &FakeGitHistory,
        options: &BlessOptions,
    ) -> Result<String, RunError> {
        block_on(bless_with(
            git,
            storage,
            "folo",
            &config(),
            options,
            &auto(),
            ts(1_700_000_000),
            "0.0.1",
            &RecordingReporter::new(),
        ))
        .map(|outcome| match outcome {
            RunOutcome::Completed { message } => message,
            RunOutcome::Analyzed { .. } => panic!("bless returns a Completed outcome"),
        })
    }

    #[test]
    fn bless_writes_a_sidecar_into_the_set_with_a_clean_run_at_head() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c2"), clean_run_json("c2", 1000).as_bytes())).unwrap();
        let git = master_git();

        let message =
            drive_bless(&storage, &git, &bless_options(&["all_the_time/read_cell"])).unwrap();
        assert!(message.contains("Blessed"), "{message}");

        let blessings = stored_blessings(&storage);
        assert_eq!(blessings.len(), 1, "one sidecar written: {blessings:?}");
        // The sidecar lands in the same commit directory as the run it accepts.
        assert!(
            blessings[0].contains("/c2/bless-"),
            "sidecar in the commit dir: {}",
            blessings[0]
        );

        // The record carries the requested prefix, blessed commit, and commit time
        // read from the run it accepts.
        let bytes = block_on(storage.get(&blessings[0])).unwrap();
        let record = BlessingRecord::from_json(&String::from_utf8(bytes).unwrap()).unwrap();
        assert_eq!(
            record.prefixes,
            vec![BenchmarkIdPrefix::new("all_the_time/read_cell").unwrap()]
        );
        assert_eq!(record.commit, "c2");
        assert_eq!(record.commit_time, ts(1000));
    }

    #[test]
    fn bless_requires_at_least_one_prefix() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c2"), clean_run_json("c2", 1000).as_bytes())).unwrap();
        let error = drive_bless(&storage, &master_git(), &bless_options(&[])).unwrap_err();
        assert!(matches!(error, RunError::Bless { .. }), "{error:?}");
        assert!(error.to_string().contains("prefix"), "{error}");
    }

    #[test]
    fn bless_off_the_base_branch_is_an_error() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("f1"), clean_run_json("f1", 1000).as_bytes())).unwrap();
        // A feature commit on top of master: HEAD is not on the base branch.
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("f1", Some("c2"))
            .branch("master", "c2")
            .branch("feature", "f1")
            .head("feature")
            .mark_default("master");

        let error = drive_bless(&storage, &git, &bless_options(&["all_the_time"])).unwrap_err();
        assert!(matches!(error, RunError::Bless { .. }), "{error:?}");
        assert!(error.to_string().contains("base branch"), "{error}");
        // The message names both the current commit and the base ref via
        // `short_sha`, so both must appear verbatim.
        assert!(error.to_string().contains("f1"), "names HEAD: {error}");
        assert!(error.to_string().contains("c2"), "names base: {error}");
        assert!(stored_blessings(&storage).is_empty(), "nothing written");
    }

    #[test]
    fn bless_a_dirty_tree_warns_but_still_blesses() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c2"), clean_run_json("c2", 1000).as_bytes())).unwrap();
        let mut git = master_git();
        git.mark_dirty();

        let message = drive_bless(&storage, &git, &bless_options(&["all_the_time"])).unwrap();
        assert!(
            message.contains("Warning: uncommitted changes present"),
            "{message}"
        );
        assert!(message.contains("Blessed"), "{message}");
        assert_eq!(
            stored_blessings(&storage).len(),
            1,
            "the committed clean run at HEAD is still blessed"
        );
    }

    #[test]
    fn bless_with_a_context_targets_an_earlier_commit() {
        let storage = MemoryStorage::new();
        // A clean run exists at c1, an earlier commit than HEAD (c2).
        block_on(storage.put(&clean_key("c1"), clean_run_json("c1", 1000).as_bytes())).unwrap();
        let options = BlessOptions {
            context: Some("c1".to_owned()),
            ..bless_options(&["all_the_time/read_cell"])
        };

        let message = drive_bless(&storage, &master_git(), &options).unwrap();
        assert!(message.contains("at commit c1"), "{message}");

        let blessings = stored_blessings(&storage);
        assert_eq!(blessings.len(), 1, "one sidecar written: {blessings:?}");
        assert!(
            blessings[0].contains("/c1/bless-"),
            "sidecar in the c1 commit dir: {}",
            blessings[0]
        );
    }

    #[test]
    fn bless_with_an_explicit_context_does_not_warn_about_a_dirty_tree() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c1"), clean_run_json("c1", 1000).as_bytes())).unwrap();
        let mut git = master_git();
        git.mark_dirty();
        let options = BlessOptions {
            context: Some("c1".to_owned()),
            ..bless_options(&["all_the_time/read_cell"])
        };

        let message = drive_bless(&storage, &git, &options).unwrap();
        assert!(
            !message.contains("Warning"),
            "an explicit context ignores the working tree: {message}"
        );
    }

    #[test]
    fn bless_all_writes_an_empty_prefix_list_accepting_every_benchmark() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c2"), clean_run_json("c2", 1000).as_bytes())).unwrap();
        let options = BlessOptions {
            all: true,
            ..BlessOptions::default()
        };

        let message = drive_bless(&storage, &master_git(), &options).unwrap();
        assert!(message.contains("all benchmarks"), "{message}");

        let blessings = stored_blessings(&storage);
        assert_eq!(blessings.len(), 1, "one sidecar written: {blessings:?}");
        let bytes = block_on(storage.get(&blessings[0])).unwrap();
        let record = BlessingRecord::from_json(&String::from_utf8(bytes).unwrap()).unwrap();
        // An empty prefix list accepts every benchmark.
        assert!(record.prefixes.is_empty());
    }

    #[test]
    fn unbless_with_a_context_removes_blessings_at_an_earlier_commit() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c1"), clean_run_json("c1", 1000).as_bytes())).unwrap();
        let git = master_git();
        let bless = BlessOptions {
            context: Some("c1".to_owned()),
            ..bless_options(&["all_the_time/read_cell"])
        };
        drive_bless(&storage, &git, &bless).unwrap();
        assert_eq!(stored_blessings(&storage).len(), 1, "blessed once");

        let unbless = UnblessOptions {
            context: Some("c1".to_owned()),
            ..UnblessOptions::default()
        };
        let outcome = block_on(unbless_with(
            &git,
            &storage,
            "folo",
            &config(),
            &unbless,
            &auto(),
            &RecordingReporter::new(),
        ))
        .unwrap();
        let message = match outcome {
            RunOutcome::Completed { message } => message,
            RunOutcome::Analyzed { .. } => panic!("unbless returns a Completed outcome"),
        };
        assert!(message.contains("at commit c1"), "{message}");
        assert!(stored_blessings(&storage).is_empty(), "sidecar deleted");
    }

    #[test]
    fn bless_without_a_run_at_head_is_an_error() {
        let storage = MemoryStorage::new();
        // A clean run exists, but on an earlier commit, not HEAD.
        block_on(storage.put(&clean_key("c1"), clean_run_json("c1", 1000).as_bytes())).unwrap();
        let error =
            drive_bless(&storage, &master_git(), &bless_options(&["all_the_time"])).unwrap_err();
        assert!(matches!(error, RunError::Bless { .. }), "{error:?}");
        assert!(error.to_string().contains("no stored result"), "{error}");
    }

    #[test]
    fn unbless_removes_every_blessing_at_head() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c2"), clean_run_json("c2", 1000).as_bytes())).unwrap();
        let git = master_git();
        drive_bless(&storage, &git, &bless_options(&["all_the_time/read_cell"])).unwrap();
        assert_eq!(stored_blessings(&storage).len(), 1, "blessed once");

        let outcome = block_on(unbless_with(
            &git,
            &storage,
            "folo",
            &config(),
            &UnblessOptions::default(),
            &auto(),
            &RecordingReporter::new(),
        ))
        .unwrap();
        let message = match outcome {
            RunOutcome::Completed { message } => message,
            RunOutcome::Analyzed { .. } => panic!("unbless returns a Completed outcome"),
        };
        assert!(message.contains("Removed"), "{message}");
        assert!(stored_blessings(&storage).is_empty(), "sidecar deleted");
    }

    #[test]
    fn unbless_reports_when_there_is_nothing_to_remove() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c2"), clean_run_json("c2", 1000).as_bytes())).unwrap();
        let outcome = block_on(unbless_with(
            &master_git(),
            &storage,
            "folo",
            &config(),
            &UnblessOptions::default(),
            &auto(),
            &RecordingReporter::new(),
        ))
        .unwrap();
        let message = match outcome {
            RunOutcome::Completed { message } => message,
            RunOutcome::Analyzed { .. } => panic!("unbless returns a Completed outcome"),
        };
        assert!(message.contains("No blessings"), "{message}");
    }

    #[test]
    fn bless_without_a_repository_is_an_error() {
        let storage = MemoryStorage::new();
        // No commits: HEAD does not resolve.
        let git = FakeGitHistory::new();
        let error =
            drive_bless(&storage, &git, &bless_options(&["all_the_time/read_cell"])).unwrap_err();
        match error {
            RunError::Bless { message } => {
                assert!(message.contains("could not resolve HEAD"), "{message}");
            }
            other => panic!("expected a bless error, got {other:?}"),
        }
    }

    #[test]
    fn bless_without_a_resolvable_base_branch_is_an_error() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c2"), clean_run_json("c2", 1000).as_bytes())).unwrap();
        // HEAD resolves, but no advertised default branch and no --base / config
        // default, so the base branch cannot be determined.
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .branch("master", "c2")
            .head("master"); // No `.mark_default(...)`.
        let error =
            drive_bless(&storage, &git, &bless_options(&["all_the_time/read_cell"])).unwrap_err();
        match error {
            RunError::Bless { message } => {
                assert!(
                    message.contains("could not determine the base branch"),
                    "{message}"
                );
                assert!(message.contains("--base"), "{message}");
            }
            other => panic!("expected a bless error, got {other:?}"),
        }
    }

    #[test]
    fn load_commit_time_rejects_a_non_utf8_object() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c2"), &[0xff, 0xfe, 0x00])).unwrap();
        let error = block_on(load_commit_time(&storage, &clean_key("c2"))).unwrap_err();
        match error {
            RunError::Bless { message } => {
                assert!(message.contains("is not valid UTF-8"), "{message}");
            }
            other => panic!("expected a bless error, got {other:?}"),
        }
    }

    #[test]
    fn load_commit_time_rejects_an_invalid_result_set() {
        let storage = MemoryStorage::new();
        block_on(storage.put(&clean_key("c2"), b"{ not a valid result set")).unwrap();
        let error = block_on(load_commit_time(&storage, &clean_key("c2"))).unwrap_err();
        match error {
            RunError::Bless { message } => {
                assert!(message.contains("is not a valid result set"), "{message}");
            }
            other => panic!("expected a bless error, got {other:?}"),
        }
    }
}
