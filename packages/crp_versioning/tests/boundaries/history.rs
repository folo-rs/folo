//! Actual release history and final unmerged parent snapshots remain separate assessment inputs.

use std::fs;
use std::path::{Path, PathBuf};

use crp_diag::{Discard, Stderr, Verbose};
use crp_versioning::apply::run_apply;
use crp_versioning::classify::{Classification, PackageClass, PackageStatus, classify_with_target};
use crp_versioning::groups::GroupState;
use crp_versioning::history::resolve_merge_target;
use crp_versioning::plan::SCHEMA_VERSION;
use crp_versioning::preview::{run_prepare_with_target, run_preview};
use crp_versioning::propose::{DECISION_SCHEMA_VERSION, run_propose};
use crp_versioning::report::{read_report, run_report_with_target};
use crp_versioning::resolved::{Inputs, ResolvedState, run_verify_preview};
use crp_versioning::{CheckFormat, CheckRequest, check_with_target};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::git_fixture::Repository;

/// A release line with catch-up work and a parent whose final edit follows its version commit.
struct HistoryFixture {
    repository: Repository,
    original_release: String,
    release_history: String,
    parent_final: String,
}

impl HistoryFixture {
    fn new() -> Self {
        let repository = Repository::new();
        repository.write(
            "Cargo.toml",
            b"[workspace]\nmembers=['api','consumer','catchup']\nresolver='3'\n",
        );
        package(&repository, "api", "1.0.0", "");
        package(&repository, "consumer", "1.0.0", &dependency("1.0.0"));
        package(&repository, "catchup", "1.0.0", "");
        repository.command(&["add", "."]);
        repository.command(&["commit", "-qm", "released packages"]);
        let original_release = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();

        // Manual unversioned content in real history must remain a release obligation.
        repository.write(
            "catchup/src/lib.rs",
            b"pub fn unreleased_history_change() {}\n",
        );
        repository.command(&["add", "."]);
        repository.command(&["commit", "-qm", "manual unversioned change"]);
        let release_history = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
        repository.command(&["update-ref", "refs/heads/release-history", &release_history]);

        package(&repository, "api", "1.1.0", "");
        package(&repository, "consumer", "1.1.0", &dependency("1.1.0"));
        repository.command(&["add", "."]);
        repository.command(&["commit", "-qm", "parent version decision"]);
        repository.write("api/src/lib.rs", b"pub fn final_parent_content() {}\n");
        repository.write(
            "Cargo.toml",
            b"[workspace]\nmembers=['api','consumer','catchup','parent-new']\nresolver='3'\n",
        );
        package(&repository, "parent-new", "1.0.0", "");
        repository.command(&["add", "."]);
        repository.command(&["commit", "-qm", "final parent content"]);
        let parent_final = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
        repository.command(&["update-ref", "refs/heads/merge-target", &parent_final]);
        Self {
            repository,
            original_release,
            release_history,
            parent_final,
        }
    }

    fn manifest(&self) -> PathBuf {
        self.repository.path().join("Cargo.toml")
    }

    fn classify(&self, target: Option<&str>) -> Classification {
        classify_with_target(&self.manifest(), Some("release-history"), target, quiet()).unwrap()
    }
}

fn quiet() -> Verbose<'static> {
    Verbose::new(false, &Discard)
}

fn dependency(version: &str) -> String {
    format!(
        "[dependencies]\nalias={{package='api',path='../api',version='={version}'}}\n\
        [package.metadata.cargo_check_external_types]\nallowed_external_types=['api::*']\n"
    )
}

fn package(repository: &Repository, name: &str, version: &str, extra: &str) {
    repository.write(
        &format!("{name}/Cargo.toml"),
        format!("[package]\nname='{name}'\nversion='{version}'\nedition='2024'\n{extra}")
            .as_bytes(),
    );
    repository.write(
        &format!("{name}/src/lib.rs"),
        b"pub fn ordinary_fixture() {}\n",
    );
}

fn classified<'a>(classification: &'a Classification, name: &str) -> &'a PackageClass {
    classification
        .packages
        .iter()
        .find(|package| package.name == name)
        .unwrap()
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Inspects real Git parent snapshots and Cargo workspace observations"
)]
fn final_parent_anchors_new_and_incremented_packages_without_hiding_history_catchup() {
    let fixture = HistoryFixture::new();
    let initial = fixture.classify(Some("merge-target"));
    for name in ["api", "consumer", "parent-new"] {
        let package = classified(&initial, name);
        assert_eq!(package.status(), PackageStatus::Unchanged);
        assert_eq!(package.anchor().unwrap().commit, fixture.parent_final);
    }
    let catchup = classified(&initial, "catchup");
    assert_eq!(catchup.status(), PackageStatus::NeedsIncrement);
    assert_eq!(catchup.anchor().unwrap().commit, fixture.original_release);
    assert!(!catchup.changed().is_empty());
    let consumer = classified(&initial, "consumer");
    assert_eq!(consumer.dependencies.len(), 1);
    let edge = consumer.dependencies.first().unwrap();
    assert_eq!(edge.name, "api");
    assert!(edge.public);
    assert_eq!(classified(&initial, "api").dependents, ["consumer"]);

    fixture
        .repository
        .write("api/src/lib.rs", b"pub fn extra_child_content() {}\n");
    let child = fixture.classify(Some("merge-target"));
    assert_eq!(
        classified(&child, "api").status(),
        PackageStatus::NeedsIncrement
    );
    assert_eq!(
        classified(&child, "api").anchor().unwrap().commit,
        fixture.parent_final
    );
    assert_eq!(
        classified(&child, "parent-new").status(),
        PackageStatus::Unchanged
    );
    fixture
        .repository
        .write("parent-new/src/lib.rs", b"pub fn child_extension() {}\n");
    assert_eq!(
        classified(&fixture.classify(Some("merge-target")), "parent-new").status(),
        PackageStatus::NeedsIncrement
    );

    package(&fixture.repository, "api", "1.1.1", "");
    package(
        &fixture.repository,
        "consumer",
        "1.1.1",
        &dependency("1.1.1"),
    );
    let incremented = fixture.classify(Some("merge-target"));
    assert_eq!(
        classified(&incremented, "api").status(),
        PackageStatus::PendingRelease
    );
    assert_eq!(
        classified(&incremented, "api").declared_version.to_string(),
        "1.1.1"
    );
    assert_eq!(
        classified(&incremented, "consumer").status(),
        PackageStatus::PendingRelease
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Compares real commit trees without rewriting repository history"
)]
fn anticipated_assessment_matches_a_commit_of_the_same_final_parent_tree() {
    let fixture = HistoryFixture::new();
    fixture
        .repository
        .write("api/src/lib.rs", b"pub fn child_after_parent() {}\n");
    let anticipated = fixture.classify(Some("merge-target"));
    let tree = fixture
        .repository
        .command(&["rev-parse", &format!("{}^{{tree}}", fixture.parent_final)]);
    // Create an additional commit object with the expected squash parent; no existing history changes.
    let squashed = fixture.repository.command(&[
        "commit-tree",
        tree.trim(),
        "-p",
        &fixture.release_history,
        "-m",
        "anticipated squash",
    ]);
    let actual =
        classify_with_target(&fixture.manifest(), Some(squashed.trim()), None, quiet()).unwrap();
    for expected in &anticipated.packages {
        let observed = classified(&actual, &expected.name);
        assert_eq!(observed.status(), expected.status());
        assert_eq!(observed.declared_version, expected.declared_version);
        assert_eq!(
            observed.anchor().unwrap().version,
            expected.anchor().unwrap().version
        );
        assert_eq!(
            serde_json::to_value(observed.changed()).unwrap(),
            serde_json::to_value(expected.changed()).unwrap()
        );
        assert_eq!(observed.dependencies, expected.dependencies);
        assert_eq!(observed.dependents, expected.dependents);
    }
    assert_eq!(
        fixture.repository.command(&["rev-parse", "HEAD"]).trim(),
        fixture.parent_final
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Resolves actual refs and validates captured context after ref movement"
)]
fn history_target_identity_and_ancestry_are_checked_and_equal_boundaries_are_not_duplicated() {
    let fixture = HistoryFixture::new();
    let ordinary = fixture.classify(None);
    let equal = fixture.classify(Some("release-history"));
    assert!(equal.merge_target.is_none());
    for expected in &ordinary.packages {
        let observed = classified(&equal, &expected.name);
        assert_eq!(observed.status(), expected.status());
        assert_eq!(observed.anchor(), expected.anchor());
    }
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/equal-target",
        &fixture.release_history,
    ]);
    let equal_inputs = Inputs::capture_with_target(
        &fixture.manifest(),
        Some("release-history"),
        Some("equal-target"),
    )
    .unwrap();
    assert!(equal_inputs.merge_target.is_none());
    assert_eq!(
        equal_inputs.merge_target_revision.as_deref(),
        Some("equal-target")
    );
    equal_inputs.verify(&fixture.manifest(), None).unwrap();
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/equal-target",
        &fixture.parent_final,
    ]);
    _ = equal_inputs.verify(&fixture.manifest(), None).unwrap_err();
    for (history, target) in [
        ("missing-history", "merge-target"),
        ("release-history", "missing-target"),
    ] {
        _ = classify_with_target(&fixture.manifest(), Some(history), Some(target), quiet())
            .unwrap_err();
    }
    let tree = fixture
        .repository
        .command(&["rev-parse", &format!("{}^{{tree}}", fixture.parent_final)]);
    let unrelated =
        fixture
            .repository
            .command(&["commit-tree", tree.trim(), "-m", "unrelated root"]);
    _ = classify_with_target(
        &fixture.manifest(),
        Some("release-history"),
        Some(unrelated.trim()),
        quiet(),
    )
    .unwrap_err();
    _ = classify_with_target(
        &fixture.manifest(),
        Some(tree.trim()),
        Some("merge-target"),
        quiet(),
    )
    .unwrap_err();
    let inputs = Inputs::capture_with_target(
        &fixture.manifest(),
        Some("release-history"),
        Some("merge-target"),
    )
    .unwrap();
    assert_eq!(inputs.release_history, fixture.release_history);
    assert_eq!(
        inputs.merge_target.as_deref(),
        Some(fixture.parent_final.as_str())
    );
    inputs.verify(&fixture.manifest(), None).unwrap();

    fixture.repository.command(&[
        "update-ref",
        "refs/heads/merge-target",
        &fixture.release_history,
    ]);
    _ = inputs.verify(&fixture.manifest(), None).unwrap_err();
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/merge-target",
        &fixture.parent_final,
    ]);
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/release-history",
        &fixture.parent_final,
    ]);
    _ = inputs.verify(&fixture.manifest(), None).unwrap_err();
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Checks actual event-target ancestry without constructing merge history"
)]
fn already_integrated_targets_are_ordinary_and_divergent_targets_require_rebase() {
    let fixture = HistoryFixture::new();
    let git = fixture.repository.repo();
    assert_eq!(
        resolve_merge_target(&git, &fixture.release_history, Some("merge-target")).unwrap(),
        Some(fixture.parent_final.clone())
    );
    assert!(
        resolve_merge_target(&git, &fixture.parent_final, Some("release-history"))
            .unwrap()
            .is_none()
    );
    assert!(
        resolve_merge_target(&git, &fixture.release_history, None)
            .unwrap()
            .is_none()
    );
    let ordinary =
        classify_with_target(&fixture.manifest(), Some("merge-target"), None, quiet()).unwrap();
    let integrated = classify_with_target(
        &fixture.manifest(),
        Some("merge-target"),
        Some("release-history"),
        quiet(),
    )
    .unwrap();
    assert!(integrated.merge_target.is_none());
    for expected in &ordinary.packages {
        let observed = classified(&integrated, &expected.name);
        assert_eq!(observed.status(), expected.status());
        assert_eq!(observed.anchor(), expected.anchor());
    }
    let captured = Inputs::capture_with_target(
        &fixture.manifest(),
        Some("merge-target"),
        Some("release-history"),
    )
    .unwrap();
    assert!(captured.merge_target.is_none());
    captured.verify(&fixture.manifest(), None).unwrap();

    let tree = fixture
        .repository
        .command(&["rev-parse", &format!("{}^{{tree}}", fixture.parent_final)]);
    let divergent = fixture.repository.command(&[
        "commit-tree",
        tree.trim(),
        "-p",
        &fixture.original_release,
        "-m",
        "stale divergent target",
    ]);
    let error = classify_with_target(
        &fixture.manifest(),
        Some("release-history"),
        Some(divergent.trim()),
        quiet(),
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("diverges from release history"));
    assert!(message.contains("refresh"));
    assert!(message.contains("rebase"));
}

#[test]
#[cfg_attr(miri, ignore = "captures Git refs and rechecks their native movement")]
fn direct_history_verification_retains_original_refs_and_normalizes_integrated_movement() {
    let fixture = HistoryFixture::new();
    let inputs = Inputs::capture_with_target(
        &fixture.manifest(),
        Some("release-history"),
        Some("merge-target"),
    )
    .unwrap();
    inputs.verify_history().unwrap();
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/merge-target",
        &fixture.release_history,
    ]);
    inputs.verify_history().unwrap_err();
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/merge-target",
        &fixture.parent_final,
    ]);
    inputs.verify_history().unwrap();
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/release-history",
        &fixture.original_release,
    ]);
    inputs.verify_history().unwrap_err();
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/release-history",
        &fixture.release_history,
    ]);

    let integrated = Inputs::capture_with_target(
        &fixture.manifest(),
        Some("merge-target"),
        Some("release-history"),
    )
    .unwrap();
    assert!(integrated.merge_target.is_none());
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/release-history",
        &fixture.original_release,
    ]);
    integrated.verify_history().unwrap();
    fixture
        .repository
        .command(&["update-ref", "-d", "refs/heads/release-history"]);
    integrated.verify_history().unwrap_err();
}

#[test]
#[cfg_attr(
    miri,
    ignore = "classifies group members from real history and parent snapshots"
)]
fn a_group_member_introduced_by_the_parent_is_not_exempt_from_child_version_matching() {
    let repository = Repository::new();
    repository.write(
        "Cargo.toml",
        b"[workspace]\nmembers=['api']\nresolver='3'\n",
    );
    package(&repository, "api", "1.0.0", "");
    repository.command(&["add", "."]);
    repository.command(&["commit", "-qm", "released api"]);
    let history = repository.repo().rev_parse("HEAD").unwrap();
    repository.write(
        "Cargo.toml",
        b"[workspace]\nmembers=['api','consumer']\nresolver='3'\n",
    );
    package(&repository, "consumer", "1.0.0", &dependency("1.0.0"));
    repository.command(&["add", "."]);
    repository.command(&["commit", "-qm", "parent adds group member"]);
    let target = repository.repo().rev_parse("HEAD").unwrap();
    package(&repository, "api", "1.1.0", "");
    package(&repository, "consumer", "1.0.0", &dependency("1.1.0"));
    let classification = classify_with_target(
        &repository.path().join("Cargo.toml"),
        Some(&history),
        Some(&target),
        quiet(),
    )
    .unwrap();
    let group = classification.groups.values().next().unwrap();
    assert_eq!(group.members(), ["api", "consumer"]);
    assert!(matches!(group.state, GroupState::Inconsistent { .. }));
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Prepares real offline evidence from the recorded default history ref"
)]
fn default_history_preparation_retains_target_and_detects_default_ref_movement() {
    let fixture = HistoryFixture::new();
    fixture.repository.command(&[
        "update-ref",
        "refs/remotes/origin/stable",
        &fixture.release_history,
    ]);
    fixture.repository.command(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/stable",
    ]);
    let output = TempDir::new().unwrap();
    let prepared = output.path().join("prepared");
    run_prepare_with_target(
        &prepared,
        None,
        Some("merge-target"),
        &fixture.manifest(),
        Verbose::new(true, &Stderr),
    )
    .unwrap();
    let captured = read(&prepared.join("prepared.json"));
    let inputs: Inputs = serde_json::from_value(captured.get("inputs").unwrap().clone()).unwrap();
    assert_eq!(inputs.release_history, fixture.release_history);
    assert_eq!(inputs.release_history_revision, "origin/stable");
    assert_eq!(
        inputs.merge_target.as_deref(),
        Some(fixture.parent_final.as_str())
    );
    let report = read_report(&prepared.join("report.json")).unwrap();
    assert_eq!(
        report.anticipated_parent_anchor("api").unwrap().commit,
        fixture.parent_final
    );
    let report = read(&prepared.join("report.json"));
    assert_eq!(
        report.get("release_history").unwrap(),
        &fixture.release_history
    );
    assert_eq!(report.get("merge_target").unwrap(), &fixture.parent_final);
    inputs.verify(&fixture.manifest(), None).unwrap();

    // Freezing the default's SHA must not forget the actual ref used to acquire it.
    fixture.repository.command(&[
        "update-ref",
        "refs/remotes/origin/stable",
        &fixture.parent_final,
    ]);
    _ = inputs.verify(&fixture.manifest(), None).unwrap_err();
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Prepares, proposes, previews and applies an offline target-aware workspace"
)]
fn prepared_proposal_preview_and_apply_preserve_history_target_and_graph_provenance() {
    let fixture = HistoryFixture::new();
    fixture
        .repository
        .write("api/src/lib.rs", b"pub fn extra_child_change() {}\n");
    let output = TempDir::new().unwrap();
    let prepared = output.path().join("prepared");
    run_prepare_with_target(
        &prepared,
        Some("release-history"),
        Some("merge-target"),
        &fixture.manifest(),
        quiet(),
    )
    .unwrap();
    let captured = read(&prepared.join("prepared.json"));
    assert_eq!(
        captured.pointer("/inputs/release_history").unwrap(),
        &fixture.release_history
    );
    assert_eq!(
        captured.pointer("/inputs/merge_target").unwrap(),
        &fixture.parent_final
    );
    assert!(captured.pointer("/inputs/base").is_none());

    let report = read(&prepared.join("report.json"));
    let typed_report = read_report(&prepared.join("report.json")).unwrap();
    assert_eq!(
        typed_report
            .anticipated_parent_anchor("api")
            .unwrap()
            .commit,
        fixture.parent_final
    );
    assert_eq!(
        typed_report
            .anticipated_parent_anchor("api")
            .unwrap()
            .version,
        "1.1.0"
    );
    assert_eq!(
        typed_report
            .anticipated_parent_anchor("parent-new")
            .unwrap()
            .commit,
        fixture.parent_final
    );
    assert!(typed_report.anticipated_parent_anchor("catchup").is_none());
    assert_eq!(
        report.get("release_history").unwrap(),
        &fixture.release_history
    );
    assert_eq!(report.get("merge_target").unwrap(), &fixture.parent_final);
    let decisions = output.path().join("decisions.json");
    fs::write(
        &decisions,
        serde_json::to_vec(&json!({
            "schema_version": DECISION_SCHEMA_VERSION,
            "changes":[{"name":"api","impact":"patch"},{"name":"catchup","impact":"patch"}]
        }))
        .unwrap(),
    )
    .unwrap();
    let proposal = output.path().join("proposal.json");
    run_propose(
        &prepared.join("report.json"),
        &decisions,
        &proposal,
        quiet(),
    )
    .unwrap();
    let plan = read(&proposal);
    assert_eq!(plan.get("release_history"), report.get("release_history"));
    assert_eq!(plan.get("merge_target"), report.get("merge_target"));

    let mut wrong = plan;
    *wrong.get_mut("merge_target").unwrap() = json!(fixture.release_history);
    let wrong_path = output.path().join("wrong-plan.json");
    fs::write(&wrong_path, serde_json::to_vec(&wrong).unwrap()).unwrap();
    let error = run_preview(
        &wrong_path,
        &prepared.join("prepared.json"),
        &output.path().join("wrong-preview"),
        &fixture.manifest(),
        quiet(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("differs from prepared evidence"));

    let preview = output.path().join("preview");
    run_preview(
        &proposal,
        &prepared.join("prepared.json"),
        &preview,
        &fixture.manifest(),
        quiet(),
    )
    .unwrap();
    let resolved = read(&preview.join("plan.json"));
    assert_eq!(resolved.get("schema_version").unwrap(), SCHEMA_VERSION);
    assert_eq!(
        resolved
            .pointer("/resolved/inputs/release_history")
            .unwrap(),
        &fixture.release_history
    );
    assert_eq!(
        resolved.pointer("/resolved/inputs/merge_target").unwrap(),
        &fixture.parent_final
    );
    assert_eq!(resolved.pointer("/resolved/versions/api").unwrap(), "1.1.1");
    assert_eq!(
        resolved.pointer("/resolved/versions/consumer").unwrap(),
        "1.1.1"
    );
    assert_eq!(
        resolved.pointer("/resolved/versions/catchup").unwrap(),
        "1.0.1"
    );
    let candidate = PathBuf::from(
        resolved
            .pointer("/resolved/evidence_manifest_path")
            .unwrap()
            .as_str()
            .unwrap(),
    );
    assert_captured_dry_run_preserves_live_files(&fixture, &preview.join("plan.json"), &resolved);
    run_verify_preview(&preview.join("plan.json"), &candidate, quiet()).unwrap();
    verify_retained_target_binding(&fixture, &resolved, &candidate);

    run_apply(
        &preview.join("plan.json"),
        false,
        &fixture.manifest(),
        quiet(),
    )
    .unwrap();
    let fresh = output.path().join("fresh");
    run_report_with_target(
        &fresh,
        Some("release-history"),
        Some("merge-target"),
        &fixture.manifest(),
        quiet(),
    )
    .unwrap();
    assert_eq!(
        read(&fresh.join("report.json")),
        read(&preview.join("report.json"))
    );
    let check = check_with_target(
        &CheckRequest {
            release_history: Some("release-history"),
            manifest_path: &fixture.manifest(),
            format: CheckFormat::Text,
            verify_packaging: false,
        },
        Some("merge-target"),
        quiet(),
    )
    .unwrap();
    assert!(check.passed, "{}", check.message);
    // A report-derived follow-up proposal must retain adequate pending increments.
    let retained = output.path().join("retained-proposal.json");
    run_propose(&fresh.join("report.json"), &decisions, &retained, quiet()).unwrap();
    let retained = read(&retained);
    assert_retained_increments(&retained, &resolved);

    assert_eq!(retained.get("merge_target").unwrap(), &fixture.parent_final);
    assert_eq!(
        retained.get("release_history").unwrap(),
        &fixture.release_history
    );
    let mut malformed = resolved;
    malformed.as_object_mut().unwrap().remove("release_history");
    let malformed_path = output.path().join("unbound-target.json");
    fs::write(&malformed_path, serde_json::to_vec(&malformed).unwrap()).unwrap();
    let error = run_apply(&malformed_path, true, &fixture.manifest(), quiet()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("merge_target requires a bound release_history")
    );
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/merge-target",
        &fixture.release_history,
    ]);
    _ = run_apply(
        &preview.join("plan.json"),
        true,
        &fixture.manifest(),
        quiet(),
    )
    .unwrap_err();
}

fn assert_captured_dry_run_preserves_live_files(
    fixture: &HistoryFixture,
    plan_path: &Path,
    plan: &Value,
) {
    let live_files: Vec<_> = plan
        .pointer("/resolved/files")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            let path = fixture
                .repository
                .path()
                .join(file.get("path").unwrap().as_str().unwrap());
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    run_apply(plan_path, true, &fixture.manifest(), quiet()).unwrap();
    for (path, bytes) in live_files {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

fn verify_retained_target_binding(fixture: &HistoryFixture, plan: &Value, candidate: &Path) {
    let retained_state: ResolvedState =
        serde_json::from_value(plan.get("resolved").unwrap().clone()).unwrap();
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/merge-target",
        &fixture.release_history,
    ]);
    _ = retained_state.verify_candidate(candidate).unwrap_err();
    fixture.repository.command(&[
        "update-ref",
        "refs/heads/merge-target",
        &fixture.parent_final,
    ]);
    retained_state.verify_candidate(candidate).unwrap();
}

fn assert_retained_increments(proposal: &Value, resolved: &Value) {
    for increment in proposal.get("increments").unwrap().as_array().unwrap() {
        let name = increment.get("name").unwrap().as_str().unwrap();
        assert_eq!(
            increment.get("version").unwrap(),
            resolved
                .pointer(&format!("/resolved/versions/{name}"))
                .unwrap()
        );
    }
}
