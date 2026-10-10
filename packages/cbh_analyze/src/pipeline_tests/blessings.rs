//! Logical acceptance survives changes in measurement availability and partition discovery.

use std::num::NonZero;
use std::path::PathBuf;

use cbh_command::{
    AnalyzeOptions, BlessOptions, ListOptions, ListSubject, PruneOptions, UnblessOptions,
};
use cbh_detect::{AnalysisMode, Series, SeriesFilter, apply_base_blessings, apply_blessings};
use cbh_diag::RecordingReporter;
use cbh_git::FakeGitHistory;
use cbh_model::{
    BenchmarkIdPrefix, BlessingRecord, BlessingScope, DiscriminantSet, Engine, ScopedBlessingRecord,
};
use cbh_storage::{MemoryStorage, Storage};
use futures::executor::block_on;
use ohno::ErrorExt as _;
use serde_json::Value;

use crate::bless::{bless_with, unbless_with};
use crate::dataset::select_dataset;
use crate::list::list_with;
use crate::pipeline_tests::harness::{
    auto, config, feature_chain, master_chain, multi_bench, now_anchor, spawner, ts,
};
use crate::prune::prune_with;
use crate::testing::store_run;
use crate::{AnalyzeError, BlessingScopeConflictError, Selection};

/// Both identities are measured together so acceptance cannot hide an unrelated series.
fn seed(storage: &MemoryStorage, set: &DiscriminantSet, commits: &[&str]) {
    for commit in commits {
        let run = multi_bench(
            0,
            commit,
            &[("selected/case", 100.0), ("unrelated/case", 100.0)],
        );
        store_run(storage, &set.clean_key("folo", commit), &run);
    }
}

fn partition(engine: Engine, target: &str, machine: &str) -> DiscriminantSet {
    DiscriminantSet::new(engine, &target.into(), &machine.into())
}

fn options() -> BlessOptions {
    BlessOptions {
        context: Some("c1".to_owned()),
        prefixes: vec![BenchmarkIdPrefix::new("selected/case").unwrap()],
        ..BlessOptions::default()
    }
}

fn issue(storage: &MemoryStorage, git: &FakeGitHistory, options: &BlessOptions) {
    block_on(bless_with(
        git,
        storage,
        "folo",
        &config(),
        options,
        ts(123),
        "test",
        &RecordingReporter::quiet(),
    ))
    .unwrap();
}

/// Resolves both evidence lines through production selection and applies their acceptance floor.
fn series(storage: &MemoryStorage, git: &FakeGitHistory) -> Vec<Series> {
    let options = AnalyzeOptions {
        target_triple: vec!["all".to_owned()],
        machine_key: vec!["all".to_owned()],
        ..AnalyzeOptions::default()
    };
    let mut dataset = block_on(select_dataset(
        git,
        storage,
        "folo",
        &config(),
        &Selection::from_analyze(&options),
        SeriesFilter::default(),
        true,
        &auto(),
        now_anchor(),
        &RecordingReporter::quiet(),
        &spawner(),
        NonZero::new(1).unwrap(),
    ))
    .unwrap();
    match dataset.mode {
        AnalysisMode::History => apply_blessings(&mut dataset.series, &dataset.blessings),
        AnalysisMode::Branch => apply_base_blessings(&mut dataset.series, &dataset.blessings),
    }
    dataset.series
}

fn listing(storage: &MemoryStorage, git: &FakeGitHistory, options: ListOptions) -> Value {
    let options = ListOptions {
        subject: ListSubject::Blessings,
        no_text: true,
        json: Some(PathBuf::from("unused.json")),
        ..options
    };
    let report = block_on(list_with(
        git,
        storage,
        "folo",
        &config(),
        &options,
        &auto(),
        now_anchor(),
        &RecordingReporter::quiet(),
        &spawner(),
        NonZero::new(1).unwrap(),
    ))
    .unwrap();
    serde_json::from_str(report.json.as_ref().unwrap()).unwrap()
}

fn revoke(
    storage: &MemoryStorage,
    git: &FakeGitHistory,
    options: &UnblessOptions,
) -> Result<String, AnalyzeError> {
    block_on(unbless_with(
        git,
        storage,
        "folo",
        &config(),
        options,
        &RecordingReporter::quiet(),
    ))
}

#[test]
fn logical_scope_covers_missing_anchor_and_later_partitions_without_widening_identity() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    let first = partition(Engine::Callgrind, "linux", "m1");
    let no_anchor = partition(Engine::Callgrind, "linux", "m2");
    let later = partition(Engine::Criterion, "windows", "m3");
    seed(&storage, &first, &["c0", "c1", "c2"]);
    seed(&storage, &no_anchor, &["c0", "c2"]);
    issue(&storage, &git, &options());
    seed(&storage, &later, &["c0", "c2"]);

    let actual = series(&storage, &git);
    assert_eq!(actual.len(), 6);
    for one in &actual {
        assert_eq!(
            one.active_start,
            usize::from(one.id.qualified() == "selected/case")
        );
        if let Some(blessing) = &one.blessing {
            assert_eq!(blessing.commit, "c1");
            assert!(one.points[one.active_start].topo_index >= 1);
        }
    }
}

#[test]
fn context_audit_preserves_persisted_scope_without_measurements() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    issue(&storage, &git, &options());
    let audit = listing(
        &storage,
        &git,
        ListOptions {
            context: Some("c1".to_owned()),
            ..ListOptions::default()
        },
    );
    assert_eq!(audit["blessings"].as_array().unwrap().len(), 1);
    assert_eq!(
        audit["blessings"][0]["discriminant_scope"],
        serde_json::to_value(BlessingScope::default()).unwrap()
    );
    assert_eq!(
        audit["blessings"][0]["prefixes"],
        serde_json::json!(["selected/case"])
    );
    assert_eq!(audit["commit"], "c1");
}

#[test]
fn window_audit_reports_each_measured_partition_accepted_by_a_logical_record() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    issue(&storage, &git, &options());
    for machine in ["m1", "m2"] {
        seed(
            &storage,
            &partition(Engine::Callgrind, "linux", machine),
            &["c0", "c2"],
        );
    }
    // Queries keep their own defaults; explicit all is needed to inspect every effective partition.
    let effective = listing(
        &storage,
        &git,
        ListOptions {
            all: true,
            target_triple: vec!["all".to_owned()],
            machine_key: vec!["all".to_owned()],
            ..ListOptions::default()
        },
    );
    assert_eq!(effective["blessings"].as_array().unwrap().len(), 2);
}

#[test]
fn revocation_restores_the_full_measured_history() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    issue(&storage, &git, &options());
    seed(
        &storage,
        &partition(Engine::Callgrind, "linux", "m1"),
        &["c0", "c2"],
    );
    revoke(
        &storage,
        &git,
        &UnblessOptions {
            context: Some("c1".to_owned()),
            ..UnblessOptions::default()
        },
    )
    .unwrap();
    assert!(
        series(&storage, &git)
            .iter()
            .all(|one| one.blessing.is_none() && one.active_start == 0)
    );
}

/// Each parameter case runs independently to keep interpreter workloads bounded.
fn assert_explicit_scope(options: &BlessOptions, accepted: &[bool]) {
    let git = master_chain(3);
    let sets = [
        partition(Engine::Callgrind, "linux", "m1"),
        partition(Engine::Callgrind, "linux", "m2"),
        partition(Engine::Callgrind, "windows", "m1"),
        partition(Engine::Criterion, "linux", "m1"),
    ];
    let storage = MemoryStorage::new();
    issue(&storage, &git, options);
    for set in &sets {
        seed(&storage, set, &["c0", "c2"]);
    }
    let actual = series(&storage, &git);
    for (set, accepted) in sets.iter().zip(accepted) {
        let one = actual
            .iter()
            .find(|one| &one.set == set && one.id.qualified() == "selected/case")
            .unwrap();
        assert_eq!(one.blessing.is_some(), *accepted);
        assert_eq!(one.active_start, usize::from(*accepted));
    }
}

#[test]
fn explicit_engine_scope_leaves_other_axes_open() {
    assert_explicit_scope(
        &BlessOptions {
            engine: vec!["CALLGRIND".to_owned()],
            ..options()
        },
        &[true, true, true, false],
    );
}

#[test]
fn explicit_target_scope_leaves_other_axes_open() {
    assert_explicit_scope(
        &BlessOptions {
            target_triple: vec!["linux".to_owned()],
            ..options()
        },
        &[true, true, false, true],
    );
}

#[test]
fn explicit_machine_scope_leaves_other_axes_open() {
    assert_explicit_scope(
        &BlessOptions {
            machine_key: vec!["m1".to_owned()],
            ..options()
        },
        &[true, false, true, true],
    );
}

#[test]
fn full_scope_requires_every_axis_and_unions_repeated_values() {
    assert_explicit_scope(
        &BlessOptions {
            engine: vec!["callgrind".to_owned()],
            target_triple: vec!["linux".to_owned()],
            machine_key: vec!["m1".to_owned(), "m2".to_owned()],
            ..options()
        },
        &[true, true, false, false],
    );
}

/// Off-context base acceptance and an ignored branch acceptance exercise distinct topologies.
fn branch_fixture() -> (FakeGitHistory, MemoryStorage) {
    // c0-c1-c2 is the base; f1-f2 fork at c0. The anchor is not on the context line.
    let git = feature_chain(3, 0);
    let storage = MemoryStorage::new();
    let set = partition(Engine::Callgrind, "linux", "later-machine");
    issue(&storage, &git, &options());
    issue(
        &storage,
        &git,
        &BlessOptions {
            context: Some("f2".to_owned()),
            all: true,
            prefixes: Vec::new(),
            ..options()
        },
    );
    seed(&storage, &set, &["c0", "c2", "f2"]);
    (git, storage)
}

#[test]
fn branch_base_floor_applies_without_anchor_measurement_and_ignores_branch_blessings() {
    let (git, storage) = branch_fixture();
    let actual = series(&storage, &git);
    let selected = actual
        .iter()
        .find(|one| one.id.qualified() == "selected/case")
        .unwrap();
    assert_eq!(selected.blessing.as_ref().unwrap().commit, "c1");
    assert_eq!(selected.base_window.len(), 1);
    assert_eq!(selected.base_window[0].topo_index, 2);
    let unrelated = actual
        .iter()
        .find(|one| one.id.qualified() == "unrelated/case")
        .unwrap();
    assert!(unrelated.blessing.is_none());
    assert_eq!(unrelated.base_window.len(), 2);
}

#[test]
fn branch_window_audit_uses_the_base_ref_evidence_line() {
    let (git, storage) = branch_fixture();
    let audit = listing(
        &storage,
        &git,
        ListOptions {
            all: true,
            target_triple: vec!["all".to_owned()],
            machine_key: vec!["all".to_owned()],
            ..ListOptions::default()
        },
    );
    assert_eq!(audit["blessings"].as_array().unwrap().len(), 1);
    assert_eq!(audit["blessings"][0]["commit"], "c1");
}

#[test]
fn legacy_partition_records_coexist_with_scopes_and_keep_their_original_restrictions() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    let first = partition(Engine::Callgrind, "linux", "m1");
    let other = partition(Engine::Callgrind, "linux", "m2");
    let record = BlessingRecord::new("c2".to_owned(), ts(124), Vec::new(), "legacy".to_owned());
    block_on(storage.put(
        &first.bless_key("folo", "c2", 124),
        record.to_json().unwrap().as_bytes(),
    ))
    .unwrap();
    issue(&storage, &git, &options());
    for set in [&first, &other] {
        seed(&storage, set, &["c0", "c1", "c2"]);
    }
}

#[test]
fn revoking_a_legacy_record_preserves_other_anchors() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    issue(&storage, &git, &options());
    let first = partition(Engine::Callgrind, "linux", "m1");
    let record = BlessingRecord::new("c2".to_owned(), ts(124), Vec::new(), "legacy".to_owned());
    block_on(storage.put(
        &first.bless_key("folo", "c2", 124),
        record.to_json().unwrap().as_bytes(),
    ))
    .unwrap();
    for one in series(&storage, &git) {
        let expected = if one.set == first {
            Some("c2")
        } else if one.id.qualified() == "selected/case" {
            Some("c1")
        } else {
            None
        };
        assert_eq!(
            one.blessing
                .as_ref()
                .map(|blessing| blessing.commit.as_str()),
            expected
        );
    }
    revoke(
        &storage,
        &git,
        &UnblessOptions {
            context: Some("c2".to_owned()),
            machine_key: vec!["m1".to_owned()],
            ..UnblessOptions::default()
        },
    )
    .unwrap();
    assert!(
        listing(
            &storage,
            &git,
            ListOptions {
                context: Some("c2".to_owned()),
                target_triple: vec!["all".to_owned()],
                machine_key: vec!["all".to_owned()],
                ..ListOptions::default()
            }
        )["blessings"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(block_on(storage.list("v1/folo/")).unwrap().len(), 1);
}

#[test]
fn narrowed_revocation_rejects_broader_acceptance_before_any_deletion() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    issue(&storage, &git, &options());
    let set = partition(Engine::Callgrind, "linux", "m1");
    let legacy = BlessingRecord::new("c1".to_owned(), ts(1), Vec::new(), "legacy".to_owned());
    block_on(storage.put(
        &set.bless_key("folo", "c1", 1),
        legacy.to_json().unwrap().as_bytes(),
    ))
    .unwrap();
    let before = block_on(storage.list("v1/folo/")).unwrap();
    let error = revoke(
        &storage,
        &git,
        &UnblessOptions {
            context: Some("c1".to_owned()),
            machine_key: vec!["m1".to_owned()],
            ..UnblessOptions::default()
        },
    )
    .unwrap_err();
    assert!(error.find_source::<BlessingScopeConflictError>().is_some());
    assert_eq!(block_on(storage.list("v1/folo/")).unwrap(), before);
}

#[test]
fn pruning_scoped_blessings_requires_complete_scope_and_respects_dry_run() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    issue(&storage, &git, &options());
    let set = partition(Engine::Callgrind, &auto().triple, &auto().machine_key);
    seed(&storage, &set, &["c0", "c1", "c2"]);
    let before = block_on(storage.list("v1/folo/")).unwrap();
    let drive = |options: &PruneOptions| {
        block_on(prune_with(
            &git,
            &storage,
            "folo",
            &config(),
            options,
            &auto(),
            now_anchor(),
            &RecordingReporter::quiet(),
        ))
    };
    let options = PruneOptions {
        clean: true,
        include_blessings: true,
        prune_base: true,
        ..PruneOptions::default()
    };
    let error = drive(&options).err().unwrap();
    assert!(error.find_source::<BlessingScopeConflictError>().is_some());
    assert_eq!(block_on(storage.list("v1/folo/")).unwrap(), before);
    let options = PruneOptions {
        target_triple: vec!["all".to_owned()],
        machine_key: vec!["all".to_owned()],
        dry_run: true,
        ..options
    };
    let report = drive(&options).unwrap();
    assert!(report.text.as_ref().unwrap().contains("all/all/all"));
    assert_eq!(block_on(storage.list("v1/folo/")).unwrap(), before);
    drive(&PruneOptions {
        dry_run: false,
        ..options
    })
    .unwrap();
    assert!(block_on(storage.list("v1/folo/")).unwrap().is_empty());
}

#[test]
fn a_logical_record_never_defaults_missing_scope_on_read() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    let record = ScopedBlessingRecord {
        record: BlessingRecord::new("c1".to_owned(), ts(1), Vec::new(), "test".to_owned()),
        scope: BlessingScope::default(),
    };
    block_on(storage.put(
        &record.key("folo"),
        record.record.to_json().unwrap().as_bytes(),
    ))
    .unwrap();
    revoke(
        &storage,
        &git,
        &UnblessOptions {
            context: Some("c1".to_owned()),
            ..UnblessOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(block_on(storage.list("v1/folo/")).unwrap().len(), 1);
}

#[test]
fn narrowed_revocation_removes_matching_records_but_not_disjoint_scopes() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    issue(
        &storage,
        &git,
        &BlessOptions {
            machine_key: vec!["m1".to_owned()],
            ..options()
        },
    );
    let unbless = UnblessOptions {
        context: Some("c1".to_owned()),
        machine_key: vec!["m2".to_owned()],
        ..UnblessOptions::default()
    };
    revoke(&storage, &git, &unbless).unwrap();
    assert_eq!(block_on(storage.list("v1/folo/")).unwrap().len(), 1);
    revoke(
        &storage,
        &git,
        &UnblessOptions {
            machine_key: vec!["M1".to_owned()],
            ..unbless
        },
    )
    .unwrap();
    assert!(block_on(storage.list("v1/folo/")).unwrap().is_empty());
}

#[test]
fn mismatched_anchor_is_an_error_instead_of_misleading_provenance() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    let mut record = ScopedBlessingRecord {
        record: BlessingRecord::new("c1".to_owned(), ts(1), Vec::new(), "test".to_owned()),
        scope: BlessingScope::default(),
    };
    let key = record.key("folo");
    record.record.commit = "c2".to_owned();
    block_on(storage.put(&key, record.to_json().unwrap().as_bytes())).unwrap();
    revoke(
        &storage,
        &git,
        &UnblessOptions {
            context: Some("c1".to_owned()),
            ..UnblessOptions::default()
        },
    )
    .unwrap_err();
    assert_eq!(block_on(storage.list("v1/folo/")).unwrap(), [key]);
}
