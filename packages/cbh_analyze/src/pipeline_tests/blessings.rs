//! Acceptance scopes survive changes in measurement availability and partition discovery.

use std::num::NonZero;
use std::path::PathBuf;

use cbh_command::{
    AnalyzeOptions, BlessOptions, ExamineOptions, ListOptions, ListSubject, PruneOptions,
    UnblessOptions,
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
use crate::examine::examine_with;
use crate::list::list_with;
use crate::pipeline::analyze_with;
use crate::pipeline_tests::harness::{
    auto, config, feature_chain, master_chain, multi_bench, now_anchor, spawner, ts,
};
use crate::prune::prune_with;
use crate::testing::run_points_json;
use crate::{AnalyzeError, BlessingScopeConflictError, InvalidBlessingError, Selection};

/// Both identities are measured together so acceptance cannot hide an unrelated series.
fn seed(storage: &MemoryStorage, set: &DiscriminantSet, commits: &[&str]) {
    // The loader uses key-derived provenance, so the result projection is reusable across commits.
    let run = multi_bench(
        0,
        commits[0],
        &[("selected/case", 100.0), ("unrelated/case", 100.0)],
    );
    let json = run_points_json(&run);
    for commit in commits {
        block_on(storage.put(&set.clean_key("folo", commit), json.as_bytes())).unwrap();
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

/// Checks the complete roster before its boundaries so empty or incomplete results cannot pass.
fn assert_boundaries(
    actual: &[Series],
    expected: &[(&DiscriminantSet, &str, Option<&str>, usize)],
) {
    assert_eq!(actual.len(), expected.len());
    for (set, benchmark, anchor, active_start) in expected {
        let one = actual
            .iter()
            .find(|one| &one.set == *set && one.id.qualified() == *benchmark)
            .unwrap();
        assert_eq!(
            one.blessing
                .as_ref()
                .map(|blessing| blessing.commit.as_str()),
            *anchor
        );
        assert_eq!(one.active_start, *active_start);
    }
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

/// Separates discovery timing cases while retaining an anchor-bearing control partition.
fn assert_scope_without_anchor(discovered_later: bool) {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    let first = partition(Engine::Callgrind, "linux", "m1");
    let no_anchor = partition(Engine::Criterion, "windows", "m2");
    seed(&storage, &first, &["c0", "c1"]);
    if !discovered_later {
        seed(&storage, &no_anchor, &["c0", "c2"]);
    }
    issue(&storage, &git, &options());
    if discovered_later {
        seed(&storage, &no_anchor, &["c0", "c2"]);
    }

    let actual = series(&storage, &git);
    assert_boundaries(
        &actual,
        &[
            (&first, "selected/case", Some("c1"), 1),
            (&first, "unrelated/case", None, 0),
            (&no_anchor, "selected/case", Some("c1"), 1),
            (&no_anchor, "unrelated/case", None, 0),
        ],
    );
    for one in &actual {
        if one.blessing.is_some() {
            assert!(one.points[one.active_start].topo_index >= 1);
        }
    }
}

#[test]
fn logical_scope_covers_missing_anchor_without_widening_identity() {
    assert_scope_without_anchor(false);
}

#[test]
fn logical_scope_covers_later_partitions_without_widening_identity() {
    assert_scope_without_anchor(true);
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
    // The view tag is independent of which context commit the caller selected.
    assert_eq!(audit["scope"], "head");
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
    let set = partition(Engine::Callgrind, "linux", "m1");
    let commits = ["c0", "c2"];
    seed(&storage, &set, &commits);
    revoke(
        &storage,
        &git,
        &UnblessOptions {
            context: Some("c1".to_owned()),
            ..UnblessOptions::default()
        },
    )
    .unwrap();
    let actual = series(&storage, &git);
    assert_boundaries(
        &actual,
        &[
            (&set, "selected/case", None, 0),
            (&set, "unrelated/case", None, 0),
        ],
    );
    assert!(actual.iter().all(|one| one.points.len() == commits.len()));
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
    assert_eq!(accepted.len(), sets.len());
    let storage = MemoryStorage::new();
    issue(&storage, &git, options);
    // Boundary placement and identity narrowing have separate fixtures. This matrix only
    // needs a measured series per partition to check which ones inherit the stored acceptance.
    let json = run_points_json(&multi_bench(0, "c2", &[("selected/case", 100.0)]));
    for set in &sets {
        block_on(storage.put(&set.clean_key("folo", "c2"), json.as_bytes())).unwrap();
    }
    let actual = series(&storage, &git);
    assert_eq!(actual.len(), sets.len());
    for (set, accepted) in sets.iter().zip(accepted) {
        let one = actual
            .iter()
            .find(|one| &one.set == set && one.id.qualified() == "selected/case")
            .unwrap();
        assert_eq!(
            one.blessing
                .as_ref()
                .map(|blessing| blessing.commit.as_str()),
            accepted.then_some("c1")
        );
        assert_eq!(one.active_start, 0);
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
    // Measurements at both anchors keep their acceptance boundaries distinct.
    for set in [&first, &other] {
        seed(&storage, set, &["c1", "c2"]);
    }
    assert_boundaries(
        &series(&storage, &git),
        &[
            (&first, "selected/case", Some("c2"), 1),
            (&first, "unrelated/case", Some("c2"), 1),
            (&other, "selected/case", Some("c1"), 0),
            (&other, "unrelated/case", None, 0),
        ],
    );
}

#[test]
fn revoking_a_legacy_record_preserves_other_anchors() {
    let git = master_chain(3);
    let storage = MemoryStorage::new();
    issue(&storage, &git, &options());
    let first = partition(Engine::Callgrind, "linux", "m1");
    let other = partition(Engine::Callgrind, "linux", "m2");
    let record = BlessingRecord::new("c2".to_owned(), ts(124), Vec::new(), "legacy".to_owned());
    let key = first.bless_key("folo", "c2", 124);
    block_on(storage.put(&key, record.to_json().unwrap().as_bytes())).unwrap();
    // Removing the newer boundary must restore the measurement at the remaining anchor.
    let commits = ["c1", "c2"];
    for set in [&first, &other] {
        seed(&storage, set, &commits);
    }
    let mut expected_keys = block_on(storage.list("v1/folo/")).unwrap();
    expected_keys.retain(|stored| stored != &key);
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
    assert_eq!(block_on(storage.list("v1/folo/")).unwrap(), expected_keys);
    let actual = series(&storage, &git);
    assert_boundaries(
        &actual,
        &[
            (&first, "selected/case", Some("c1"), 0),
            (&first, "unrelated/case", None, 0),
            (&other, "selected/case", Some("c1"), 0),
            (&other, "unrelated/case", None, 0),
        ],
    );
    assert!(actual.iter().all(|one| one.points.len() == commits.len()));
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
    // A clean object and a blessing exercise both deletion sets.
    seed(&storage, &set, &["c0"]);
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

/// Each gate has an eligible control, and the complete object roster verifies preservation.
fn assert_pruning_preserves_scoped_blessing(
    git: &FakeGitHistory,
    options: PruneOptions,
    preserved_commit: &str,
    removed_commit: &str,
) {
    let storage = MemoryStorage::new();
    let keys = [preserved_commit, removed_commit].map(|commit| {
        let record = ScopedBlessingRecord {
            // Issuance time is deliberately unrelated to the commit-date cutoff.
            record: BlessingRecord::new(commit.to_owned(), ts(123), Vec::new(), "test".to_owned()),
            scope: BlessingScope::default(),
        };
        let key = record.key("folo");
        block_on(storage.put(&key, record.to_json().unwrap().as_bytes())).unwrap();
        key
    });
    let set = partition(Engine::Callgrind, &auto().triple, &auto().machine_key);
    // Blessing-only pruning must preserve even a run at the selected anchor.
    seed(&storage, &set, &[removed_commit]);
    let before = block_on(storage.list("v1/folo/")).unwrap();
    let options = PruneOptions {
        include_blessings: true,
        target_triple: vec!["all".to_owned()],
        machine_key: vec!["all".to_owned()],
        no_text: true,
        json: Some(PathBuf::from("unused.json")),
        ..options
    };
    for dry_run in [true, false] {
        let report = block_on(prune_with(
            git,
            &storage,
            "folo",
            &config(),
            &PruneOptions {
                dry_run,
                ..options.clone()
            },
            &auto(),
            now_anchor(),
            &RecordingReporter::quiet(),
        ))
        .unwrap();
        let report: Value = serde_json::from_str(report.json.as_ref().unwrap()).unwrap();
        assert_eq!(report["totals"]["runs"], 0);
        assert_eq!(report["totals"]["blessings"], 1);
        let sets = report["sets"].as_array().unwrap();
        assert_eq!(sets.len(), 1);
        let commits = sets[0]["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0]["commit"], removed_commit);
        assert_eq!(commits[0]["keys"], serde_json::json!([keys[1]]));
        let expected: Vec<_> = before
            .iter()
            .filter(|key| dry_run || **key != keys[1])
            .cloned()
            .collect();
        assert_eq!(block_on(storage.list("v1/folo/")).unwrap(), expected);
    }
}

#[test]
fn pruning_scoped_blessings_preserves_off_history_anchors() {
    assert_pruning_preserves_scoped_blessing(
        &feature_chain(3, 0),
        PruneOptions::default(),
        "c2",
        "f2",
    );
}

#[test]
fn pruning_scoped_blessings_preserves_shared_base_anchors() {
    assert_pruning_preserves_scoped_blessing(
        &feature_chain(3, 0),
        PruneOptions::default(),
        "c0",
        "f2",
    );
}

#[test]
fn pruning_scoped_blessings_preserves_unselected_commits() {
    assert_pruning_preserves_scoped_blessing(
        &master_chain(3),
        PruneOptions {
            prune_base: true,
            commit: vec!["c2".to_owned()],
            ..PruneOptions::default()
        },
        "c1",
        "c2",
    );
}

#[test]
fn pruning_scoped_blessings_preserves_anchors_before_cutoff() {
    assert_pruning_preserves_scoped_blessing(
        &master_chain(3),
        PruneOptions {
            prune_base: true,
            // The retained commit precedes this cutoff; the control is exactly on it.
            since: Some(ts(2).to_string()),
            ..PruneOptions::default()
        },
        "c1",
        "c2",
    );
}

/// Raw query outputs depend on measurements, not acceptance-record payloads.
fn raw_views(storage: &MemoryStorage, git: &FakeGitHistory) -> [Value; 2] {
    let runs = block_on(list_with(
        git,
        storage,
        "folo",
        &config(),
        &ListOptions {
            subject: ListSubject::Runs,
            no_text: true,
            json: Some(PathBuf::from("unused.json")),
            ..ListOptions::default()
        },
        &auto(),
        now_anchor(),
        &RecordingReporter::quiet(),
        &spawner(),
        NonZero::new(1).unwrap(),
    ))
    .unwrap();
    let points = block_on(examine_with(
        git,
        storage,
        "folo",
        &config(),
        &ExamineOptions {
            benchmark: "selected/case".to_owned(),
            metric: "instruction_count".to_owned(),
            no_text: true,
            json: Some(PathBuf::from("unused.json")),
            ..ExamineOptions::default()
        },
        &auto(),
        now_anchor(),
        &RecordingReporter::quiet(),
        &spawner(),
        NonZero::new(1).unwrap(),
    ))
    .unwrap();
    [runs, points].map(|report| serde_json::from_str(report.json.as_ref().unwrap()).unwrap())
}

/// Undecodable acceptance cannot hide measured commits or alter their values.
/// A measured tip and an earlier unmeasured blessing anchor keep the Miri fixture small.
fn assert_raw_views_ignore_invalid_blessing(git: &FakeGitHistory, commits: &[&str], legacy: bool) {
    let storage = MemoryStorage::new();
    let set = partition(Engine::Callgrind, &auto().triple, &auto().machine_key);
    seed(&storage, &set, commits);
    let record = ScopedBlessingRecord {
        record: BlessingRecord::new("c0".to_owned(), ts(123), Vec::new(), "test".to_owned()),
        scope: BlessingScope::default(),
    };
    let key = if legacy {
        set.bless_key("folo", "c0", 123)
    } else {
        record.key("folo")
    };
    block_on(storage.put(&key, b"not JSON")).unwrap();
    let actual = raw_views(&storage, git);
    assert_eq!(actual[0]["totals"]["runs"], commits.len());
    assert_eq!(actual[0]["totals"]["series"], 2);
    assert_eq!(actual[1]["sets"].as_array().unwrap().len(), 1);
    let points = actual[1]["sets"][0]["points"].as_array().unwrap();
    let measured: Vec<_> = points
        .iter()
        .filter(|point| !point["value"].is_null())
        .map(|point| {
            assert_eq!(point["value"], 100.0);
            point["commit"].as_str().unwrap()
        })
        .collect();
    assert_eq!(measured, commits);
}

/// Consumers validate acceptance even when no measurements project its scope.
fn assert_consumers_reject_invalid_blessing(git: &FakeGitHistory) {
    let storage = MemoryStorage::new();
    let record = ScopedBlessingRecord {
        record: BlessingRecord::new("c0".to_owned(), ts(123), Vec::new(), "test".to_owned()),
        scope: BlessingScope::default(),
    };
    block_on(storage.put(&record.key("folo"), b"not JSON")).unwrap();
    let analysis = block_on(analyze_with(
        git,
        &storage,
        "folo",
        &config(),
        &AnalyzeOptions::default(),
        &auto(),
        now_anchor(),
        &RecordingReporter::quiet(),
        false,
        &spawner(),
        NonZero::new(1).unwrap(),
    ))
    .err()
    .unwrap();
    assert!(analysis.find_source::<InvalidBlessingError>().is_some());
    let inspection = block_on(list_with(
        git,
        &storage,
        "folo",
        &config(),
        &ListOptions {
            subject: ListSubject::Blessings,
            all: true,
            ..ListOptions::default()
        },
        &auto(),
        now_anchor(),
        &RecordingReporter::quiet(),
        &spawner(),
        NonZero::new(1).unwrap(),
    ))
    .err()
    .unwrap();
    assert!(inspection.find_source::<InvalidBlessingError>().is_some());
}

#[test]
fn history_consumers_reject_invalid_scoped_blessings() {
    assert_consumers_reject_invalid_blessing(&master_chain(2));
}

#[test]
fn branch_consumers_reject_invalid_scoped_blessings() {
    assert_consumers_reject_invalid_blessing(&feature_chain(2, 0));
}

#[test]
fn history_raw_views_ignore_invalid_scoped_blessings() {
    assert_raw_views_ignore_invalid_blessing(&master_chain(2), &["c1"], false);
}

#[test]
fn history_raw_views_ignore_invalid_legacy_blessings() {
    assert_raw_views_ignore_invalid_blessing(&master_chain(2), &["c1"], true);
}

#[test]
fn branch_raw_views_ignore_invalid_scoped_blessings() {
    assert_raw_views_ignore_invalid_blessing(&feature_chain(2, 0), &["f2"], false);
}

#[test]
fn branch_raw_views_ignore_invalid_legacy_blessings() {
    assert_raw_views_ignore_invalid_blessing(&feature_chain(2, 0), &["f2"], true);
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
