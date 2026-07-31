use crate::harness::*;

/// The seeded runs' benchmark identity, as `rekey` reports it.
const BENCHMARK: &str = "nm/nm::observe/pull";

/// The measurement level every fixture that must not trip the merge tolerance
/// records on both sides of a merge.
const LEVEL: f64 = 100.0;

/// A dry run reports the plan and writes nothing, and a following `--apply` still
/// finds the objects to copy — the copies are what `--apply` adds, not the plan.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn rekey_dry_run_reports_without_writing() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_rekey_clean(REKEY_SLOW_LEGACY_KEY, REKEY_SLOW_SPEED, "c1", LEVEL);
    let before = workspace.stored_keys();

    let message = workspace.drive_json(&["rekey"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["apply"], false, "{message}");
    assert_eq!(parsed["totals"]["copies"], 1, "{message}");
    assert_eq!(parsed["totals"]["copied"], 0, "{message}");
    assert_eq!(
        workspace.stored_keys(),
        before,
        "a dry run writes nothing: {message}"
    );

    let message = workspace.drive_json(&["rekey", "--apply"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["copied"], 1, "{message}");
}

/// `--apply` copies each object to its current-format key and leaves the source
/// exactly where it was, so the migration is never a move.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn rekey_apply_copies_and_leaves_the_source_in_place() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_rekey_clean(REKEY_SLOW_LEGACY_KEY, REKEY_SLOW_SPEED, "c1", LEVEL);
    let source = workspace.stored_keys().pop().unwrap();

    let message = workspace.drive_json(&["rekey", "--apply"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["copied"], 1, "{message}");

    let keys = workspace.stored_keys();
    assert!(keys.contains(&source), "the source survives: {keys:?}");
    let destination = source.replace(REKEY_SLOW_LEGACY_KEY, REKEY_CURRENT_KEY);
    assert!(
        keys.contains(&destination),
        "the copy landed at {destination}: {keys:?}"
    );
    assert_eq!(keys.len(), 2, "{keys:?}");

    // Both keys now hold the same run, which is what makes the copy a copy.
    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 2);
    assert_eq!(
        objects[0].1.to_json().unwrap(),
        objects[1].1.to_json().unwrap()
    );
}

/// A second `--apply` finds every destination already present and writes nothing
/// further, so the command is safe to re-run.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn rekey_apply_is_idempotent() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_rekey_clean(REKEY_SLOW_LEGACY_KEY, REKEY_SLOW_SPEED, "c1", LEVEL);

    _ = workspace.drive_json(&["rekey", "--apply"]).await;
    let after_first = workspace.stored_keys();

    let message = workspace.drive_json(&["rekey", "--apply"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["copied"], 0, "{message}");
    assert_eq!(parsed["totals"]["already_present"], 1, "{message}");
    assert_eq!(workspace.stored_keys(), after_first);
}

/// Every object kind sharing a machine partition migrates: clean runs, dirty
/// snapshots, and the blessing sidecars that carry no hardware of their own.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn rekey_migrates_clean_dirty_and_bless_objects() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_rekey_clean(REKEY_SLOW_LEGACY_KEY, REKEY_SLOW_SPEED, "c1", LEVEL);
    workspace.seed_rekey_dirty(
        REKEY_SLOW_LEGACY_KEY,
        REKEY_SLOW_SPEED,
        "2024-01-03",
        "c2",
        LEVEL,
    );
    workspace.seed_rekey_bless(REKEY_SLOW_LEGACY_KEY, "2024-01-04", "c2");

    let message = workspace.drive_json(&["rekey", "--apply"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["copied"], 3, "{message}");

    let migrated: Vec<String> = workspace
        .stored_keys()
        .into_iter()
        .filter(|key| key.contains(REKEY_CURRENT_KEY))
        .collect();
    assert_eq!(migrated.len(), 3, "{migrated:?}");
    assert!(
        migrated.iter().any(|key| key.ends_with("clean.json")),
        "{migrated:?}"
    );
    assert!(
        migrated
            .iter()
            .any(|key| key.contains("/dirty-") && key.ends_with(".json")),
        "{migrated:?}"
    );
    assert!(
        migrated
            .iter()
            .any(|key| key.contains("/bless-") && key.ends_with(".json")),
        "{migrated:?}"
    );
}

/// History stored under an explicit `--machine-key` override is left exactly where
/// it is: its partitioning was an operator's decision, and its own recorded
/// fingerprint proves the segment is not a hardware hash.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn rekey_leaves_override_keyed_objects_alone() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_rekey_clean("github", REKEY_SLOW_SPEED, "c1", LEVEL);
    let before = workspace.stored_keys();

    let message = workspace.drive_json(&["rekey", "--apply"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["copies"], 0, "{message}");
    assert_eq!(parsed["totals"]["key_override"], 1, "{message}");
    assert_eq!(parsed["key_overrides"][0]["machine_key"], "github");
    assert_eq!(workspace.stored_keys(), before);
}

/// Two speed buckets of one machine measured alternately across history merge
/// without complaint: the groups already share a timeline and agree on level.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn rekey_merges_interleaved_partitions_that_agree_on_level() {
    let workspace = Workspace::repo(&storage_only_config());
    for (index, label) in ["c1", "c2", "c3", "c4"].iter().enumerate() {
        workspace.commit_dated(&format!("2024-01-0{}", index + 1), label);
    }
    workspace.seed_rekey_clean(REKEY_SLOW_LEGACY_KEY, REKEY_SLOW_SPEED, "c1", LEVEL);
    workspace.seed_rekey_clean(REKEY_FAST_LEGACY_KEY, REKEY_FAST_SPEED, "c2", LEVEL);
    workspace.seed_rekey_clean(REKEY_SLOW_LEGACY_KEY, REKEY_SLOW_SPEED, "c3", LEVEL);
    workspace.seed_rekey_clean(REKEY_FAST_LEGACY_KEY, REKEY_FAST_SPEED, "c4", LEVEL);

    let message = workspace.drive_json(&["rekey", "--apply"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["copied"], 4, "{message}");

    let merge = &parsed["merges"][0];
    assert_eq!(merge["machine_key"], REKEY_CURRENT_KEY, "{message}");
    let pair = &merge["pairs"][0];
    assert_eq!(pair["interleaving"], "interleaved", "{message}");
    let offset = &pair["offsets"][0];
    assert_eq!(offset["benchmark"], BENCHMARK, "{message}");
    assert_eq!(offset["absolute"], 0.0, "{message}");
    assert_eq!(offset["exceeds_tolerance"], false, "{message}");

    // The merged partition now carries one continuous four-point series, which is
    // the whole point of the migration.
    let message = workspace
        .drive_json(&["list", "runs", "--machine-key", REKEY_CURRENT_KEY])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["runs"], 4, "{message}");
}

/// Two partitions occupying disjoint stretches of history at systematically
/// different levels would splice a step change into the merged series, so the pass
/// refuses and writes nothing.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn rekey_refuses_a_merge_that_would_manufacture_a_step_change() {
    /// The level the later partition sits at: far enough above [`LEVEL`] to clear
    /// both the relative and the absolute merge tolerance.
    const SHIFTED_LEVEL: f64 = 200.0;

    let workspace = Workspace::repo(&storage_only_config());
    for (index, label) in ["c1", "c2", "c3", "c4"].iter().enumerate() {
        workspace.commit_dated(&format!("2024-01-0{}", index + 1), label);
    }
    workspace.seed_rekey_clean(REKEY_SLOW_LEGACY_KEY, REKEY_SLOW_SPEED, "c1", LEVEL);
    workspace.seed_rekey_clean(REKEY_SLOW_LEGACY_KEY, REKEY_SLOW_SPEED, "c2", LEVEL);
    workspace.seed_rekey_clean(REKEY_FAST_LEGACY_KEY, REKEY_FAST_SPEED, "c3", SHIFTED_LEVEL);
    workspace.seed_rekey_clean(REKEY_FAST_LEGACY_KEY, REKEY_FAST_SPEED, "c4", SHIFTED_LEVEL);
    let before = workspace.stored_keys();

    let error = workspace
        .drive(&["rekey", "--apply"])
        .await
        .expect_err("the merge is refused");
    let message = error.to_string();
    assert!(message.contains("beyond the merge tolerance"), "{message}");
    assert!(message.contains("time-blocked"), "{message}");
    assert!(message.contains(BENCHMARK), "{message}");
    assert!(message.contains("--allow-level-shift"), "{message}");
    assert_eq!(workspace.stored_keys(), before, "nothing is written");

    // The dry run refuses on exactly the same grounds, so the preview cannot
    // disagree with what `--apply` would do.
    let error = workspace
        .drive(&["rekey"])
        .await
        .expect_err("the dry run refuses too");
    assert!(
        error.to_string().contains("beyond the merge tolerance"),
        "{error}"
    );

    // The override is what lets an operator proceed once the difference is
    // understood.
    let message = workspace
        .drive_json(&["rekey", "--apply", "--allow-level-shift"])
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    assert_eq!(parsed["totals"]["copied"], 4, "{message}");
}

/// The reported offset and interleaving describe the constructed fixture exactly:
/// a `+5%` step between two disjoint stretches, under the absolute floor so it does
/// not block.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn rekey_reports_the_offset_and_interleaving_of_a_merge() {
    /// A level 1% above [`LEVEL`]: below the relative merge tolerance, so the
    /// merge is reported but permitted.
    const SLIGHTLY_HIGHER: f64 = 101.0;

    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.commit_dated("2024-01-02", "c2");
    workspace.seed_rekey_clean(REKEY_SLOW_LEGACY_KEY, REKEY_SLOW_SPEED, "c1", LEVEL);
    workspace.seed_rekey_clean(
        REKEY_FAST_LEGACY_KEY,
        REKEY_FAST_SPEED,
        "c2",
        SLIGHTLY_HIGHER,
    );

    let message = workspace.drive_json(&["rekey"]).await;
    let parsed: serde_json::Value = serde_json::from_str(&message).unwrap();
    let pair = &parsed["merges"][0]["pairs"][0];
    assert_eq!(pair["baseline_machine_key"], REKEY_SLOW_LEGACY_KEY);
    assert_eq!(pair["incoming_machine_key"], REKEY_FAST_LEGACY_KEY);
    assert_eq!(pair["interleaving"], "time-blocked", "{message}");
    assert_eq!(pair["blocks"], 2, "{message}");
    let offset = &pair["offsets"][0];
    assert_eq!(offset["metric"], "instruction_count", "{message}");
    assert_eq!(offset["baseline_level"], LEVEL, "{message}");
    assert_eq!(offset["incoming_level"], SLIGHTLY_HIGHER, "{message}");
    assert_eq!(offset["absolute"], 1.0, "{message}");
    assert_eq!(offset["exceeds_tolerance"], false, "{message}");

    let text = workspace.drive(&["rekey"]).await.unwrap();
    let report = text.stdout_text().unwrap();
    assert!(report.contains("time-blocked over 2 blocks"), "{report}");
    assert!(report.contains("100 -> 101 (+1, +1%)"), "{report}");
}

/// A stored run whose recorded fingerprint matches neither the retired nor the
/// current hash of the hardware recorded beside it stops the whole pass: the
/// migration's only proof that a key segment is a hardware hash is that
/// recomputation, so a single disagreement invalidates every decision it would make.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn rekey_aborts_when_a_stored_fingerprint_cannot_be_reproduced() {
    let workspace = Workspace::repo(&storage_only_config());
    workspace.commit_dated("2024-01-01", "c1");
    workspace.seed_rekey_clean(REKEY_SLOW_LEGACY_KEY, REKEY_SLOW_SPEED, "c1", LEVEL);

    // Rewrite the seeded object's fingerprint so it no longer matches its own
    // recorded hardware.
    let (key, mut set) = workspace.single_object();
    set.context.machine.as_mut().unwrap().fingerprint = "0000000000000000".to_owned();
    workspace.seed(&key, &set);

    let error = workspace
        .drive(&["rekey"])
        .await
        .expect_err("the pass is abandoned");
    let message = error.to_string();
    assert!(
        message.contains("do not reproduce the fingerprint"),
        "{message}"
    );
    assert!(message.contains("0000000000000000"), "{message}");
}
