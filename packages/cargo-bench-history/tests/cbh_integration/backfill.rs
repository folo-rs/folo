use std::fs;
use std::process::Command as ProcessCommand;

use crate::harness::*;

#[test]
#[cfg_attr(
    miri,
    ignore = "uses real git and binary processes with filesystem storage"
)]
fn backfill_rejects_unavailable_git() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    let head_before = workspace.head();
    let worktrees_before = workspace.git(&["worktree", "list", "--porcelain"]).stdout;

    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .args(["backfill", "HEAD", "HEAD", "--ignore-errors"])
        .arg(format!(
            "--local={}",
            workspace.root().join("store").display()
        ))
        .current_dir(workspace.root())
        // Only this child loses Git; fixture setup and other tests keep their environment.
        .env("PATH", "")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    assert!(workspace.stored_objects().is_empty());
    assert_eq!(workspace.head(), head_before);
    assert_eq!(
        workspace.git(&["worktree", "list", "--porcelain"]).stdout,
        worktrees_before
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "uses real git and binary processes with filesystem storage"
)]
fn backfill_rejects_non_git_directory() {
    let workspace = Workspace::new(&storage_only_config());

    for ignore_errors in [false, true] {
        let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_cargo-bench-history"));
        command
            .args(["backfill", "HEAD", "HEAD"])
            .arg(format!(
                "--local={}",
                workspace.root().join("store").display()
            ))
            .current_dir(workspace.root());
        if ignore_errors {
            command.arg("--ignore-errors");
        }
        let output = command.output().unwrap();

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
        assert!(workspace.stored_objects().is_empty());
        assert!(!workspace.root().join(".git").exists());
        assert!(!workspace.root().join("target").exists());
    }
}

/// A backfill stores one clean result per commit in the range, leaves the primary
/// checkout and branch untouched, and the backfilled points then surface through
/// `analyze` in git-topology order.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_stores_one_clean_object_per_commit_and_restores_checkout() {
    let bench = callgrind_arg("grp", CALLGRIND_SINGLE);
    let workspace = Workspace::clean_repo(&storage_only_config())
        .with_bench(&["--callgrind", &bench])
        .with_real_auto_discriminants();
    let c1 = workspace.commit("c1");
    let c2 = workspace.commit("c2");
    let branch_before = workspace.current_branch();
    let head_before = workspace.head();

    let RunOutcome::Completed { message } = workspace.drive(&["backfill", &c1, &c2]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("2 stored"), "{message}");

    // One clean object per commit, keyed by that commit's full ID. `backfill`
    // auto-detects the target triple and machine key, so derive both from a stored
    // object to keep the key assertions correct on every platform CI runs on.
    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 2, "{objects:?}");
    let triple = objects[0].1.context.toolchain.target_triple.clone();
    let machine = objects[0]
        .1
        .context
        .machine
        .as_ref()
        .expect("backfill records host-hardware provenance")
        .fingerprint
        .clone();
    for commit_id in [&c1, &c2] {
        let expected =
            format!("v1/testproj/objects/callgrind/{triple}/{machine}/{commit_id}/clean.json");
        assert!(
            objects.iter().any(|(key, _)| key == &expected),
            "missing {expected} in {objects:?}"
        );
    }

    // The backfill never touches the primary checkout: HEAD and the branch are
    // exactly as they were before it ran.
    assert_eq!(workspace.current_branch(), branch_before);
    assert_eq!(workspace.head(), head_before);

    // The backfilled points are now visible to `analyze` along the master line.
    let report = workspace.drive_json(&["analyze"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 2,
        "analyze should see every backfilled commit: {report}"
    );
}

/// A nested workspace keeps its project directory in every historical checkout,
/// including its output tree, and resuming never launches already-recorded benches.
#[tokio::test]
#[cfg_attr(
    miri,
    ignore = "uses real git worktrees, processes, and filesystem storage"
)]
async fn backfill_preserves_nested_project_and_resumes() {
    let workspace = Workspace::clean_repo(&storage_only_config());
    let before_project = workspace.head();
    let relative = Path::new(".github").join("fixtures").join("nested project");
    let project = workspace.root().join(&relative);
    fs::create_dir_all(project.join("benches")).unwrap();
    fs::write(
        project.join("Cargo.toml"),
        "[workspace]\n[package]\nname = \"nested_project\"\nversion = \"0.0.0\"\n\
         edition = \"2024\"\n[lib]\npath = \"lib.rs\"\n",
    )
    .unwrap();
    fs::write(project.join("lib.rs"), "pub fn measure() {}\n").unwrap();
    fs::write(project.join("benches").join("marker"), "nested benches\n").unwrap();
    workspace.git(&["add", &relative.to_string_lossy()]);
    let c1 = workspace.commit_with_file("nested workspace", ".gitignore", "target/\n.cargo/\n");
    let c2 = workspace.commit_with_file(
        "update nested workspace",
        &relative.join("lib.rs").to_string_lossy(),
        "pub fn measure() -> bool { true }\n",
    );

    // Configuration belongs to the invocation, not the historical checkouts.
    fs::create_dir_all(project.join(".cargo")).unwrap();
    fs::write(
        project.join(".cargo").join("bench_history.toml"),
        storage_only_config().replace("testproj", "nested-project"),
    )
    .unwrap();
    workspace.make_dirty("UNCOMMITTED");
    // Running in the primary project, or writing to the worktree's repository-root
    // target on the newer commit, makes the faker fail rather than silently pass.
    fs::create_dir_all(workspace.root().join("target")).unwrap();
    let root_target = Path::new("..").join("..").join("..").join("target");
    let status_before = workspace.git(&["status", "--porcelain"]).stdout;
    let worktrees_before = workspace.git(&["worktree", "list", "--porcelain"]).stdout;
    let branch_before = workspace.current_branch();

    let bench = callgrind_arg(
        "nested",
        "nested_bench::measure|measure||nested_project=41/5/2",
    );
    let command = command_from(&[
        "backfill",
        &c1,
        &c2,
        &format!("--local={}", workspace.root().join("store").display()),
    ]);
    let outcome = run_with_overrides(
        &command,
        Overrides {
            workspace_dir: Some(project.clone()),
            bench_command: Some(vec![
                cargo_bench_history_faker::binary_path().to_owned(),
                "--fail-if-exists".to_owned(),
                root_target.to_string_lossy().into_owned(),
                // Only the selected project has this directory.
                "--chdir".to_owned(),
                "benches".to_owned(),
                "--callgrind".to_owned(),
                bench,
            ]),
            ..Overrides::default()
        },
    )
    .await
    .unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("2 stored"), "{message}");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 2);
    for commit in [&c1, &c2] {
        let (key, run) = objects
            .iter()
            .find(|(_, run)| run.context.git.commit.as_ref() == Some(commit))
            .unwrap();
        let triple = &run.context.toolchain.target_triple;
        let machine = &run.context.machine.as_ref().unwrap().fingerprint;
        assert_eq!(
            key,
            &format!("v1/nested-project/objects/callgrind/{triple}/{machine}/{commit}/clean.json")
        );
        assert!(!run.context.git.dirty);
        assert_eq!(run.results.len(), 1);
        assert_eq!(
            run.results[0].id.segments.first().as_str(),
            "nested_project"
        );
        assert_eq!(run.results[0].id.segments.last().as_str(), "measure");
        assert_eq!(ir_of(&run.results[0]), 41.0);
    }

    // Exercise --repo as well as the workspace override. Any benchmark launch
    // fails, so this proves the partition pre-check skips execution, not just writes.
    let workspace = workspace.with_bench(&["--exit-code", "1"]);
    let RunOutcome::Completed { message } = workspace
        .drive(&["backfill", &c1, &c2, "--repo", &relative.to_string_lossy()])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(
        message.contains("0 stored, 2 skipped (existing)"),
        "{message}"
    );

    // Recorded commits launch no benchmarks, so their ignored target output cannot keep
    // the project directory alive when checkout reaches a commit before its introduction.
    let RunOutcome::Completed { message } = workspace
        .drive(&[
            "backfill",
            &before_project,
            &c2,
            "--repo",
            &relative.to_string_lossy(),
            "--ignore-errors",
        ])
        .await
        .unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(
        message.contains("0 stored, 2 skipped (existing)"),
        "{message}"
    );
    assert!(message.contains("1 failed"), "{message}");
    let resumed = workspace.stored_objects();
    assert_eq!(resumed, objects);
    assert_eq!(workspace.head(), c2);
    assert_eq!(workspace.current_branch(), branch_before);
    assert_eq!(
        workspace.git(&["status", "--porcelain"]).stdout,
        status_before
    );
    assert_eq!(
        workspace.git(&["worktree", "list", "--porcelain"]).stdout,
        worktrees_before
    );
}

/// Backfill walks the first-parent line across a merge commit: a range spanning a
/// pull-request merge stores one clean object on each first-parent commit
/// (including the merge commit itself) and never on the merged-in side branch's
/// own commits.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_spans_a_merge_commit_along_first_parent() {
    let bench = callgrind_arg("grp", CALLGRIND_SINGLE);
    let workspace =
        Workspace::clean_repo(&storage_only_config()).with_bench(&["--callgrind", &bench]);
    // master:  root - c1 - M - c3   (M merges the side branch into master)
    //                  \   /
    //  side:            sf1 - sf2
    let c1 = workspace.commit("c1");
    workspace.checkout_new_branch("side");
    let sf1 = workspace.commit("sf1");
    let sf2 = workspace.commit("sf2");
    workspace.checkout("master");
    let m = workspace.merge("side", "M");
    let c3 = workspace.commit("c3");

    let RunOutcome::Completed { message } = workspace.drive(&["backfill", &c1, &c3]).await.unwrap()
    else {
        panic!("expected a completed outcome");
    };
    assert!(message.contains("3 stored"), "{message}");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 3, "{objects:?}");
    let triple = objects[0].1.context.toolchain.target_triple.clone();
    let machine = objects[0]
        .1
        .context
        .machine
        .as_ref()
        .expect("backfill records host-hardware provenance")
        .fingerprint
        .clone();
    for commit_id in [&c1, &m, &c3] {
        let expected =
            format!("v1/testproj/objects/callgrind/{triple}/{machine}/{commit_id}/clean.json");
        assert!(
            objects.iter().any(|(key, _)| key == &expected),
            "missing {expected} in {objects:?}"
        );
    }
    // The merged-in side-branch commits are off the first-parent line: nothing is
    // stored for them.
    for commit_id in [&sf1, &sf2] {
        assert!(
            !objects
                .iter()
                .any(|(key, _)| key.contains(commit_id.as_str())),
            "side-branch commit {commit_id} must not be backfilled: {objects:?}"
        );
    }
}

/// A commit that fails to benchmark stops the backfill by default: the newer
/// commits are stored, but the loop halts at the failure and reports a non-zero
/// exit.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_stops_on_a_failing_commit_by_default() {
    let bench = callgrind_arg("grp", CALLGRIND_SINGLE);
    let workspace = Workspace::clean_repo(&storage_only_config()).with_bench(&[
        "--callgrind",
        &bench,
        "--fail-if-exists",
        "BROKEN",
    ]);
    let c1 = workspace.commit("c1");
    workspace.commit_with_file("c2 introduces a broken build", "BROKEN", "boom\n");
    let c3 = workspace.commit_removing_file("c3 fixes the build", "BROKEN");

    workspace.drive(&["backfill", &c1, &c3]).await.unwrap_err();

    // The walk is newest-first, so only c3 (healthy) was stored before the stop at
    // c2; c1 never ran.
    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 1, "{objects:?}");
    assert!(objects[0].0.contains(&c3), "{:?}", objects[0].0);
}
