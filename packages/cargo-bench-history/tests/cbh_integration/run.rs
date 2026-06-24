use crate::harness::*;

/// End-to-end happy path: a successful no-op engine command, one harvested
/// summary, and a stored set with the expected object key and context.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_callgrind_end_to_end_stores_results() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--summary", "grp=single"]);

    let outcome = workspace.drive(&["run"]).await.unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");

    let (key, set) = workspace.single_object();

    // Synthetic partition (Callgrind is hardware-independent) under the resolved
    // triple. `run` auto-detects the host triple, so derive it from the stored
    // context to keep the assertion host-portable. The temp workspace is outside
    // any git repository, so the commit resolves to the `unknown` fallback and the
    // clean tree yields `clean.json`.
    let triple = &set.context.toolchain.target_triple;
    assert_eq!(
        key,
        format!("v2/testproj/callgrind/{triple}/synthetic/unknown/clean.json")
    );

    assert_eq!(set.schema_version, SCHEMA_VERSION);
    assert_eq!(set.context.tool_version, TOOL_VERSION);
    // Outside a git repository there is no committer date, so the commit time
    // falls back to the observation time.
    assert_eq!(set.context.commit, set.context.observation);

    assert_eq!(set.results.len(), 1);
    let record = &set.results[0];
    // The unparametrized summary carries no parameter segment, so its identity
    // ends at the function name.
    assert_eq!(
        record.id.segments.last().as_str(),
        "timestamp_capture_std_now"
    );
    assert_eq!(ir_of(record), 36.0);
    assert_eq!(metric_of(record, MetricKind::InstructionCount).value, 36.0);
}

/// Regression: a benchmark binary launched by `cargo bench --package X` runs with
/// its working directory set to the package directory, so the harvest must inject
/// an *absolute* `CARGO_TARGET_DIR`. A relative one would be resolved by an engine
/// that honors it (such as Criterion) against that package directory, depositing
/// output where the workspace-rooted harvest never looks — storing nothing, the
/// exact symptom this guards against. Driving without a target-root override
/// exercises the real `resolve_target_root`, and the mock changes into a package
/// subdirectory before writing, standing in for cargo's per-package cwd.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_harvests_output_when_the_engine_runs_in_a_package_directory() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--chdir",
        "subpkg",
        "--summary",
        "grp=single",
    ]);
    std::fs::create_dir_all(workspace.root().join("subpkg")).unwrap();

    let outcome = workspace
        .drive_resolving_target_root(&["run"])
        .await
        .unwrap();

    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");
    assert_eq!(
        workspace.stored_objects().len(),
        1,
        "the summary written from the package directory should be harvested and stored"
    );
}

/// Two summaries under the target tree yield one stored set with one record each.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_stores_a_record_per_summary() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--summary",
        "a=single",
        "--summary",
        "b=parametrized",
    ]);

    let outcome = workspace.drive(&["run"]).await.unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("covering 2"), "{message}");

    let (_, set) = workspace.single_object();
    assert_eq!(set.results.len(), 2);

    let parametrized = set
        .results
        .iter()
        .find(|record| record.id.segments.last().as_str() == "two_instants")
        .unwrap();
    assert_eq!(ir_of(parametrized), 87.0);

    let unparametrized = set
        .results
        .iter()
        .find(|record| record.id.segments.last().as_str() != "two_instants")
        .unwrap();
    assert_eq!(ir_of(unparametrized), 36.0);
}

/// Two bench targets that share a `module_path` but live in different packages
/// stay distinct: their records differ only in `package`, so they never collapse
/// into one series. Without the package component they would silently merge.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_distinguishes_same_module_path_across_packages() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--summary",
        "a=single",
        "--summary",
        "b=single-alt-pkg",
    ]);

    let outcome = workspace.drive(&["run"]).await.unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("covering 2"), "{message}");

    let (_, set) = workspace.single_object();
    assert_eq!(set.results.len(), 2);

    // Both records share module_path, function, and value; only the package (the
    // leading segment) differs.
    assert_eq!(
        set.results[0].id.segments[1], set.results[1].id.segments[1],
        "the colliding module_path is identical"
    );

    let mut packages: Vec<&str> = set
        .results
        .iter()
        .map(|r| r.id.segments[0].as_str())
        .collect();
    packages.sort_unstable();
    assert_eq!(packages, vec!["fast_time", "other_pkg"]);

    // The identities differ, so analyze would build two series rather than one.
    assert_ne!(set.results[0].id, set.results[1].id);
}

/// Two bench harnesses that compile to the *same* binary name in different
/// packages (`foo/benches/a.rs` and `bar/benches/a.rs`) write their summaries
/// under the same top-level `gungraun/shared/` directory but in distinct nested
/// ones. Both must be harvested — the recursive walk finds each — and kept
/// distinct by package, so the on-disk name collision never collapses or drops a
/// result.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_harvests_colliding_bench_binary_names_in_distinct_packages() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--summary",
        "shared/foo=single",
        "--summary",
        "shared/bar=single-alt-pkg",
    ]);

    let outcome = workspace.drive(&["run"]).await.unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("covering 2"), "{message}");

    let (_, set) = workspace.single_object();
    assert_eq!(
        set.results.len(),
        2,
        "both colliding-name summaries are harvested from their nested directories"
    );

    // Same module_path/function, distinct package — exactly the cross-package
    // collision shape.
    let mut packages: Vec<&str> = set
        .results
        .iter()
        .map(|r| r.id.segments[0].as_str())
        .collect();
    packages.sort_unstable();
    assert_eq!(packages, vec!["fast_time", "other_pkg"]);
    assert_ne!(set.results[0].id, set.results[1].id);
}

/// `--no-store` still harvests the output but writes nothing to storage.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_no_store_harvests_without_storing() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--summary", "grp=single"]);

    let outcome = workspace.drive(&["run", "--no-store"]).await.unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Harvested 1"), "{message}");
    assert!(message.contains("nothing stored"), "{message}");

    assert!(
        workspace.stored_objects().is_empty(),
        "nothing should be stored"
    );
}

/// A non-zero engine exit aborts the run with the engine name and exit code, and
/// nothing is stored.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_propagates_nonzero_engine_exit() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--exit-code", "7"]);

    let error = workspace.drive(&["run"]).await.unwrap_err();
    let RunError::Engine { engine, code } = error else {
        panic!("expected an engine error, got {error:?}");
    };
    assert_eq!(engine, "cargo bench");
    assert_eq!(code, Some(7));

    assert!(
        workspace.stored_objects().is_empty(),
        "a failed run stores nothing"
    );
}

/// A Criterion run stores a wall-time result set in a machine-fingerprinted
/// partition. With no engine configuration, the criterion output the mock writes
/// is auto-detected and harvested.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_criterion_stores_results() {
    let workspace =
        Workspace::new(&storage_only_config()).with_bench(&["--criterion", "grp|capture|now=26.9"]);

    let outcome = workspace.drive(&["run"]).await.unwrap();
    assert!(matches!(outcome, RunOutcome::Completed { .. }));

    let (key, set) = workspace.single_object();
    // Criterion partitions by the host triple and a machine fingerprint (never the
    // `synthetic` segment Callgrind uses).
    assert!(key.contains("/criterion/"), "{key}");
    assert!(!key.contains("/synthetic/"), "{key}");
    assert_eq!(set.results.len(), 1);
    let metric = &set.results[0].metrics[0];
    assert_eq!(metric.kind, MetricKind::WallTime);
    assert_eq!(metric.value, 26.9);
    assert_eq!(metric.kind.as_unit(), "ns");
    assert!(metric.std_dev.is_some(), "dispersion should be recorded");
}

/// A single benchmark run harvests every engine that produced output: the mock
/// writes a Callgrind summary, a Criterion case, an `alloc_tracker` operation and
/// an `all_the_time` operation, so the run stores one result set per engine in its
/// own partition.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_harvests_every_engine_that_produced_output() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--summary",
        "grp=single",
        "--criterion",
        "grp|capture|now=12.5",
        "--alloc-tracker",
        "allocate_vec=200/2",
        "--all-the-time",
        "read_cell=20",
    ]);

    let outcome = workspace.drive(&["run"]).await.unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 4"), "{message}");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 4, "{objects:?}");
    // Deterministic engines (Callgrind instruction counts, allocation statistics)
    // land in the `synthetic` partition; hardware-dependent engines (Criterion
    // wall time, `all_the_time` processor time) carry a machine fingerprint.
    assert!(
        objects
            .iter()
            .any(|(key, _)| key.contains("/callgrind/") && key.contains("/synthetic/")),
        "{objects:?}"
    );
    assert!(
        objects
            .iter()
            .any(|(key, _)| key.contains("/criterion/") && !key.contains("/synthetic/")),
        "{objects:?}"
    );
    assert!(
        objects
            .iter()
            .any(|(key, _)| key.contains("/alloc_tracker/") && key.contains("/synthetic/")),
        "{objects:?}"
    );
    assert!(
        objects
            .iter()
            .any(|(key, _)| key.contains("/all_the_time/") && !key.contains("/synthetic/")),
        "{objects:?}"
    );
}

/// An `alloc_tracker` run stores allocation statistics in the `synthetic`
/// partition (allocation counts are a deterministic property of the code, not the
/// hardware), carrying both the byte and the count metric.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_alloc_tracker_stores_results() {
    let workspace = Workspace::new(&storage_only_config())
        .with_bench(&["--alloc-tracker", "allocate_vec=200/2"]);

    let outcome = workspace.drive(&["run"]).await.unwrap();
    assert!(matches!(outcome, RunOutcome::Completed { .. }));

    let (key, set) = workspace.single_object();
    assert!(key.contains("/alloc_tracker/"), "{key}");
    assert!(key.contains("/synthetic/"), "{key}");
    assert_eq!(set.results.len(), 1);
    let record = &set.results[0];
    assert_eq!(record.id.qualified(), "allocate_vec");
    assert_eq!(record.metrics.len(), 2, "{:?}", record.metrics);

    let bytes = metric_of(record, MetricKind::AllocatedBytes);
    assert_eq!(bytes.value, 200.0);
    assert_eq!(bytes.kind.as_unit(), "bytes");

    let count = metric_of(record, MetricKind::AllocationCount);
    assert_eq!(count.value, 2.0);
    assert_eq!(count.kind.as_unit(), "count");
}

/// An `all_the_time` run stores processor time in a machine-fingerprinted
/// partition (processor time is hardware-dependent), and `--machine-key` overrides
/// the fingerprint.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_all_the_time_is_partitioned_by_machine_key() {
    let workspace =
        Workspace::new(&storage_only_config()).with_bench(&["--all-the-time", "read_cell=20"]);

    workspace
        .drive(&["run", "--machine-key", "ci-pool-b"])
        .await
        .unwrap();

    let (key, set) = workspace.single_object();
    let triple = &set.context.toolchain.target_triple;
    assert!(
        key.contains(&format!("/all_the_time/{triple}/ci-pool-b/")),
        "{key}"
    );
    assert!(!key.contains("/synthetic/"), "{key}");
    assert_eq!(set.results.len(), 1);
    let record = &set.results[0];
    assert_eq!(record.id.qualified(), "read_cell");
    let processor_time = metric_of(record, MetricKind::ProcessorTime);
    assert_eq!(processor_time.value, 20.0);
    assert_eq!(processor_time.kind.as_unit(), "ns");
}

/// An `all_the_time` run whose emitted output carries a bootstrap confidence
/// interval stores that dispersion on the metric, so the noise detector can
/// later apply its interval-overlap gate to processor time. This proves the
/// dispersion fields flow from the engine's JSON through the harvest and adapter
/// into the stored result set, end to end through the real adapter.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_all_the_time_records_dispersion() {
    let workspace = Workspace::new(&storage_only_config())
        .with_bench(&["--all-the-time", "read_cell=20@19:21"]);

    workspace.drive(&["run"]).await.unwrap();

    let (_key, set) = workspace.single_object();
    let processor_time = metric_of(&set.results[0], MetricKind::ProcessorTime);
    assert_eq!(processor_time.value, 20.0);
    assert_eq!(processor_time.interval_low, Some(19.0));
    assert_eq!(processor_time.interval_high, Some(21.0));
    assert_eq!(processor_time.std_dev, Some(1.0));
}

/// `--machine-key` overrides the machine fingerprint in a Criterion partition.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_criterion_honors_machine_key_override() {
    let workspace =
        Workspace::new(&storage_only_config()).with_bench(&["--criterion", "grp|capture|now=9"]);

    // `run` auto-detects the triple; this test asserts the machine-key override
    // segment, so derive the triple from the stored context for a portable key.
    workspace
        .drive(&["run", "--machine-key", "ci-pool-a"])
        .await
        .unwrap();

    let (key, set) = workspace.single_object();
    let triple = &set.context.toolchain.target_triple;
    assert!(
        key.contains(&format!("/criterion/{triple}/ci-pool-a/")),
        "{key}"
    );
}

/// A Criterion run collects every harvested case into one result set, keeping
/// distinct group/function/value identities as separate records.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_criterion_collects_distinct_cases_as_records() {
    // Same function name in two different groups, plus a parametrized case: all
    // three identities are distinct and must survive as separate records.
    let workspace = Workspace::new(&storage_only_config()).with_bench(&[
        "--criterion",
        "timestamp/capture|std_instant|now=27",
        "--criterion",
        "timestamp/elapsed|std_instant|now=41",
        "--criterion",
        "timestamp/capture|fast_clock|=13",
    ]);

    workspace.drive(&["run"]).await.unwrap();

    let (_, set) = workspace.single_object();
    assert_eq!(set.results.len(), 3, "{:?}", set.results);
    // Criterion identities are `group/function[/value]`; the three cases stay
    // distinct. Criterion output carries no package attribution, so no package
    // segment appears.
    let mut ids: Vec<String> = set
        .results
        .iter()
        .map(|record| record.id.qualified())
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "timestamp/capture/fast_clock".to_owned(),
            "timestamp/capture/std_instant/now".to_owned(),
            "timestamp/elapsed/std_instant/now".to_owned(),
        ]
    );
}

/// A project id containing characters that require sanitizing (a space and a `/`)
/// round-trips: `run` stores under the sanitized partition and `analyze` finds the
/// very same history. Both sides must derive the identical storage segment, so this
/// guards against writer/reader sanitization drift through the real pipeline.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_then_analyze_round_trips_a_sanitizing_project_id() {
    let workspace = Workspace::clean_repo(&storage_only_config_with_id("my proj/sub"))
        .with_bench(&["--summary", "grp=single"]);

    workspace.drive(&["run"]).await.unwrap();

    // The writer sanitizes `my proj/sub` to `my_proj_sub` for the partition.
    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 1, "{objects:?}");
    assert!(
        objects
            .iter()
            .all(|(key, _)| key.starts_with("v2/my_proj_sub/callgrind/")),
        "{objects:?}"
    );

    // The reader must sanitize the same way; otherwise it lists an empty history.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 1,
        "analyze must find the run the sanitized partition stored: {report}"
    );
    // The single Callgrind record carries several metrics, each its own series; the
    // exact count is incidental, but the history must be non-empty.
    assert!(parsed["series"].as_u64().unwrap() >= 1, "{report}");
}

/// Unusual characters in a benchmark identity (spaces, a dot, and non-ASCII
/// letters) belong to the object's JSON body, never to the storage partition key.
/// They survive `run` -> store -> `analyze` verbatim while the key stays sanitized
/// and the reader keeps both runs in one series.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_then_analyze_preserves_unusual_identity_characters() {
    let workspace = Workspace::clean_repo(&storage_only_config())
        .with_bench(&["--criterion", "time.capture|mide tiempo|tamaño 4=18.5"]);

    workspace
        .drive(&["run", "--machine-key", "pool"])
        .await
        .unwrap();

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 1, "{objects:?}");
    let (key, set) = &objects[0];
    // The partition key is identity-free and fully sanitized: none of the
    // identity's spaces or non-ASCII letters leak into it. `run` auto-detects the
    // triple, so derive it from the stored context for a portable prefix.
    let triple = &set.context.toolchain.target_triple;
    assert!(
        key.starts_with(&format!("v2/testproj/criterion/{triple}/pool/")),
        "{key}"
    );
    assert!(
        !key.contains(' ') && !key.contains("tamaño") && !key.contains("mide"),
        "{key}"
    );

    // The identity survives verbatim in the stored result set.
    assert_eq!(set.results.len(), 1, "{:?}", set.results);
    let id = &set.results[0].id;
    assert_eq!(id.qualified(), "time.capture/mide tiempo/tamaño 4");

    // The reader reconstructs the series, proving the unusual identity is a stable
    // series key end to end.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--machine-key", "pool", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["runs"], 1, "{report}");
    assert_eq!(parsed["series"], 1, "{report}");
}

/// `--config` loads the configuration from a non-default path.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_uses_explicit_config_path() {
    // Only the custom path holds a configuration; the default discovery path is
    // absent, so a successful run proves `--config` was honored.
    let workspace = Workspace::with_config_at("config/bench.toml", &storage_only_config())
        .with_bench(&["--summary", "grp=single"]);

    let outcome = workspace
        .drive(&["run", "--config", "config/bench.toml"])
        .await
        .unwrap();
    assert!(matches!(outcome, RunOutcome::Completed { .. }));
    assert_eq!(workspace.stored_objects().len(), 1);
}

/// A clean re-run of the same commit collides with the deterministic clean key,
/// so the second run is refused unless an overwrite is requested.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn re_running_the_same_commit_is_refused_as_a_duplicate() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--summary", "grp=single"]);

    workspace.drive(&["run"]).await.unwrap();

    let error = workspace.drive(&["run"]).await.unwrap_err();
    let RunError::Duplicate { key } = error else {
        panic!("expected a duplicate error, got {error:?}");
    };
    assert!(key.ends_with("/clean.json"), "{key}");

    // The refused run left the single stored object in place.
    assert_eq!(workspace.stored_objects().len(), 1);
}

/// `--overwrite` replaces an already-stored clean result rather than refusing it.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn overwrite_replaces_the_stored_result() {
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--summary", "grp=single"]);

    workspace.drive(&["run"]).await.unwrap();

    workspace.drive(&["run", "--overwrite"]).await.unwrap();

    let objects = workspace.stored_objects();
    assert_eq!(
        objects.len(),
        1,
        "overwrite must not create a second object"
    );
    assert!(objects[0].0.ends_with("/clean.json"), "{:?}", objects[0].0);
}
