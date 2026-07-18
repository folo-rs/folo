//! End-to-end tests for the hidden `import` command against the published faker.
//!
//! These exercise the seams `import` newly makes reachable without a real
//! benchmark engine: the faker's **library** value core and file writers feeding a
//! real filesystem harvest, the faker **binary** an external repository would run,
//! and the metadata overrides (`--commit`, `--target-triple`) flowing through the
//! real CLI into the storage key. They complement — never replace — the real
//! `cargo bench` end-to-end test in `cbh_real_engine.rs`.
//!
//! The freshness gate's `None` (admit-all) and `Some` (mtime-gated) branches are
//! covered directly at the `FsBenchOutputSource` unit level in `cbh_engines`, so
//! they are not re-proven here; `import` simply passes `None`, which the format
//! canary in `commands::import`'s own tests confirms harvests and stores.

use cargo_bench_history_faker::{callgrind_summary, write_callgrind_summary};
use cbh_engines::parse_callgrind_summary;

use crate::harness::*;

/// Runs the published faker **binary** — the exact path an external repository
/// takes — writing its engine output into `target_dir` via `CARGO_TARGET_DIR`,
/// exactly as a harvest resolves it.
fn run_faker_binary(target_dir: &Path, args: &[&str]) {
    let status = std::process::Command::new(cargo_bench_history_faker::binary_path())
        .args(args)
        .env("CARGO_TARGET_DIR", target_dir)
        .status()
        .expect("the faker binary should run");
    assert!(
        status.success(),
        "faker binary exited unsuccessfully: {status}"
    );
}

/// Imports a curated tree for one commit and asserts the run stored, returning the
/// human-readable completion message. Drives the real hidden `import` command
/// through the production entry, keying the run to `commit` via `--commit` so no
/// checkout is needed to attribute the imported data to a historical commit.
async fn import_for_commit(workspace: &Workspace, target_dir: &Path, commit: &str) -> String {
    let outcome = workspace
        .drive(&[
            "import",
            "--target-dir",
            target_dir.to_str().unwrap(),
            "--commit",
            commit,
        ])
        .await
        .unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected an import completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");
    message
}

/// The faker's **library** writers produce a curated tree that a real harvest,
/// `import`, and `examine` round-trip back into exactly the written instruction
/// counts — the writer↔harvester↔store↔read seam an external repository depends
/// on. The whole history is committed up front, then each ancestor's run is
/// imported via `--commit` while HEAD stays at the tip, so a synthetic series is
/// attributed across history without a single checkout.
#[tokio::test]
#[cfg_attr(miri, ignore)] // Real filesystem harvest + git; Miri runs no such IO.
async fn importing_faker_library_output_per_commit_round_trips_through_examine() {
    let workspace = Workspace::repo(&storage_only_config());

    // A three-commit rising history for the shared `nm/nm::observe/pull` benchmark:
    // the package (`nm`), module (`nm::observe`), and function (`pull`) segments are
    // exactly what the parser derives from this identity, so the series is
    // addressable by that qualified name. Committing all three before importing
    // proves `--commit` attributes data to an ancestor without leaving the tip.
    let history: [(&str, &str, u64); 3] = [
        ("2024-01-01", "c1", 100),
        ("2024-01-02", "c2", 100),
        ("2024-01-03", "c3", 130),
    ];
    let commits: Vec<(String, u64)> = history
        .iter()
        .map(|&(date, label, ir)| (workspace.commit_dated(date, label), ir))
        .collect();
    for (commit, ir) in &commits {
        // Faker output goes under `target/`, which the harness excludes from git, so
        // the working tree stays clean and the imported run is recorded as clean.
        let tree = workspace
            .root()
            .join("target")
            .join(format!("import-{commit}"));
        let summary = callgrind_summary("nm::observe", "pull", None, Some("/pkg/nm"), *ir, 4, 2);
        write_callgrind_summary(&tree, "nm", &summary);
        import_for_commit(&workspace, &tree, commit).await;
    }

    let report = workspace
        .drive_json(&[
            "examine",
            "--benchmark",
            "nm/nm::observe/pull",
            "--metric",
            "instruction_count",
        ])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    let points = parsed["sets"][0]["points"].as_array().unwrap();

    let values: Vec<f64> = points
        .iter()
        .map(|point| point["value"].as_f64().unwrap())
        .collect();
    assert_eq!(values, vec![100.0, 100.0, 130.0], "{report}");
    assert_eq!(
        points
            .iter()
            .map(|point| point["title"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["c1", "c2", "c3"],
        "{report}"
    );
    assert!(
        points.iter().all(|point| point["dirty"] == false),
        "every imported run is clean: {report}"
    );
    assert!(
        points
            .iter()
            .all(|point| point["commit"].as_str().unwrap().len() == 40),
        "full commit ids: {report}"
    );
}

/// The public path an external repository takes — run the faker **binary**, then
/// `import` its output — surfaces the synthetic series in `list`, and a
/// `--target-triple` override flows through the real CLI into the partition key.
/// This is the write-path e2e `import` newly makes possible with only the two
/// published binaries.
#[tokio::test]
#[cfg_attr(miri, ignore)] // Spawns the real faker binary and harvests real files.
async fn importing_real_faker_binary_output_surfaces_in_list_under_the_overridden_triple() {
    let workspace = Workspace::repo(&storage_only_config());
    let commits = [
        workspace.commit_dated("2024-01-01", "c1"),
        workspace.commit_dated("2024-01-02", "c2"),
    ];

    // Import an overridden triple so the discriminant is deterministic regardless of
    // the host this test runs on, proving `--target-triple` reaches the storage key
    // through the real command line.
    for (index, commit) in commits.iter().enumerate() {
        let tree = workspace
            .root()
            .join("target")
            .join(format!("faker-{index}"));
        run_faker_binary(&tree, &["--summary", "grp=single"]);
        let outcome = workspace
            .drive(&[
                "import",
                "--target-dir",
                tree.to_str().unwrap(),
                "--target-triple",
                "x86_64-unknown-linux-gnu",
                "--commit",
                commit,
            ])
            .await
            .unwrap();
        let RunOutcome::Completed { message } = outcome else {
            panic!("expected an import completion, got {outcome:?}");
        };
        assert!(message.contains("Stored 1"), "{message}");
    }

    let runs = workspace.drive_json(&["list", "runs"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&runs).unwrap();
    assert_eq!(parsed["totals"]["runs"], 2, "{runs}");
    // The single callgrind benchmark contributes three metric series (Ir, Bc, Bi).
    assert_eq!(parsed["totals"]["series"], 3, "{runs}");
    assert_eq!(parsed["totals"]["discriminant_sets"], 1, "{runs}");
    assert_eq!(parsed["sets"][0]["engine"], "callgrind", "{runs}");

    let discriminants = workspace.drive_json(&["list", "discriminants"]).await;
    let sets: serde_json::Value = serde_json::from_str(&discriminants).unwrap();
    let sets = sets.as_array().unwrap();
    assert_eq!(sets.len(), 1, "{discriminants}");
    assert_eq!(
        sets[0]["target_triple"], "x86_64-unknown-linux-gnu",
        "the --target-triple override should key the partition: {discriminants}"
    );
    // Callgrind is hardware-independent, so it stays in the `synthetic` machine
    // partition even though the real host was probed for provenance.
    assert_eq!(sets[0]["machine_key"], "synthetic", "{discriminants}");
}

/// The faker's callgrind value core produces output the *real* parser reads back
/// into the requested identity and instruction/branch counts, for arbitrary
/// parameters (not just the fixed presets). This is the self-contained
/// faker→parser guarantee that replaced the old shared-fixture coupling: if the
/// faker's document shape drifts from what the parser accepts, this fails. Pure
/// value core, no filesystem — so it runs under Miri too.
#[test]
fn faker_callgrind_value_core_round_trips_through_the_real_parser() {
    let summary = callgrind_summary(
        "nm::observe",
        "pull",
        Some("wide"),
        Some("/work/packages/nm"),
        123,
        45,
        6,
    );
    let record = parse_callgrind_summary(&summary).expect("faker output must parse");

    assert_eq!(record.id.qualified(), "nm/nm::observe/pull/wide");
    assert_eq!(
        metric_of(&record, MetricKind::InstructionCount).value,
        123.0
    );
    assert_eq!(
        metric_of(&record, MetricKind::ConditionalBranches).value,
        45.0
    );
    assert_eq!(metric_of(&record, MetricKind::IndirectBranches).value, 6.0);
}
