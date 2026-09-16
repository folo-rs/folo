//! In-process application dispatch, manifest transformation and write ordering.

use std::cell::{Cell, RefCell};

use serde_json::json;

use super::*;
use crate::groups::Groups;
use crate::lockfile::InstallationGraph;
use crate::metadata::VersionTarget;
use crate::plan::PlanIncrement;
use crate::{ParseTomlError, UnknownPlanTargetError};

#[test]
fn expanded_or_captured_plans_dispatch_without_manifest_only_operations() {
    for stage in [PlanStage::Proposed, PlanStage::Expanded] {
        for captured in [false, true] {
            for dry_run in [false, true] {
                let plan = plan(stage, captured);
                let called = Cell::new(false);
                let output = apply_plan(
                    &plan,
                    dry_run,
                    Verbose::new(false),
                    |received, received_dry_run| {
                        assert_eq!(received, &plan);
                        assert_eq!(received_dry_run, dry_run);
                        called.set(true);
                        Ok("captured result".to_owned())
                    },
                    || {
                        assert_eq!(stage, PlanStage::Proposed);
                        assert!(!captured);
                        Ok(work_tree())
                    },
                    |path| Ok(manifests().get(path).unwrap().clone()),
                    |_| Ok(()),
                )
                .unwrap();
                match (stage, captured) {
                    (PlanStage::Proposed, false) => {
                        assert!(!called.get());
                        assert_eq!(output, manifest_only_summary(dry_run));
                    }
                    _ => {
                        assert!(called.get());
                        assert_eq!(output, "captured result");
                    }
                }
            }
        }
    }
}

#[test]
fn captured_application_errors_do_not_fall_back_to_manifest_only_edits() {
    let error = apply_plan(
        &plan(PlanStage::Expanded, false),
        false,
        Verbose::new(false),
        |_, _| Err(ApplicationFailure::new().into()),
        || panic!(),
        |_| panic!(),
        |_| panic!(),
    )
    .unwrap_err();
    assert!(error.find_source::<ApplicationFailure>().is_some());
}

#[test]
fn edits_include_root_and_unique_members_with_complete_rewritten_contents() {
    let tree = work_tree();
    let originals = manifests();
    let reads = RefCell::new(Vec::new());
    let edits = compute_edits_with(&tree, &versions("0.2.0"), Verbose::new(false), |path| {
        reads.borrow_mut().push(path.to_path_buf());
        Ok(originals.get(path).unwrap().clone())
    })
    .unwrap();

    let paths = unique_paths();
    assert_eq!(*reads.borrow(), paths);
    assert_eq!(
        edits.iter().map(|edit| &edit.path).collect::<Vec<_>>(),
        paths.iter().collect::<Vec<_>>()
    );
    let expected = updated_manifests();
    for edit in edits {
        assert_eq!(&edit.original, originals.get(&edit.path).unwrap());
        assert_eq!(&edit.updated, expected.get(&edit.path).unwrap());
    }
}

#[test]
fn matching_versions_and_unselected_content_survive_byte_for_byte() {
    let originals = manifests();
    let edits = compute_edits_with(
        &work_tree(),
        &versions("0.1.0"),
        Verbose::new(false),
        |path| Ok(originals.get(path).unwrap().clone()),
    )
    .unwrap();

    assert_eq!(edits.len(), originals.len());
    for edit in &edits {
        assert_eq!(&edit.original, originals.get(&edit.path).unwrap());
        assert_eq!(&edit.updated, originals.get(&edit.path).unwrap());
    }
    assert_eq!(changed_edit_count(&edits), 0);
}

#[test]
fn application_reads_every_manifest_before_writing_only_changed_contents() {
    let originals = manifests();
    let expected = updated_manifests();
    let reads = Cell::new(0);
    let mut writes = BTreeMap::new();
    let output = apply_plan(
        &plan(PlanStage::Proposed, false),
        false,
        Verbose::new(false),
        |_, _| panic!(),
        || Ok(work_tree()),
        |path| {
            assert!(reads.get() < originals.len());
            reads.set(reads.get() + 1);
            Ok(originals.get(path).unwrap().clone())
        },
        |edit| {
            assert_eq!(reads.get(), originals.len());
            assert_eq!(&edit.original, originals.get(&edit.path).unwrap());
            assert_ne!(edit.updated, edit.original);
            assert!(
                writes
                    .insert(edit.path.clone(), edit.updated.clone())
                    .is_none()
            );
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(output, manifest_only_summary(false));
    assert_eq!(
        writes,
        expected
            .into_iter()
            .filter(|(path, updated)| originals.get(path).unwrap() != updated)
            .collect()
    );
}

#[test]
fn dry_run_computes_the_full_edit_set_without_writing() {
    let originals = manifests();
    let mut reads = Vec::new();
    let output = apply_plan(
        &plan(PlanStage::Proposed, false),
        true,
        Verbose::new(false),
        |_, _| panic!(),
        || Ok(work_tree()),
        |path| {
            reads.push(path.to_path_buf());
            Ok(originals.get(path).unwrap().clone())
        },
        |_| panic!(),
    )
    .unwrap();
    assert_eq!(reads, unique_paths());
    assert_eq!(output, manifest_only_summary(true));
}

#[test]
fn unchanged_application_does_not_write() {
    let mut plan = plan(PlanStage::Proposed, false);
    for increment in &mut plan.increments {
        increment.version = Some("0.1.0".to_owned());
    }
    let output = apply_plan(
        &plan,
        false,
        Verbose::new(false),
        |_, _| panic!(),
        || Ok(work_tree()),
        |path| Ok(manifests().get(path).unwrap().clone()),
        |_| panic!(),
    )
    .unwrap();
    assert_eq!(
        output,
        "Updated 0 manifests and left the workspace lockfile untouched; use prepare and preview for a resolved release plan"
    );
}

#[test]
fn workspace_and_plan_failures_precede_manifest_acquisition() {
    let plan = plan(PlanStage::Proposed, false);
    let error = apply_plan(
        &plan,
        false,
        Verbose::new(false),
        |_, _| panic!(),
        || Err(ApplicationFailure::new().into()),
        |_| panic!(),
        |_| panic!(),
    )
    .unwrap_err();
    assert!(error.find_source::<ApplicationFailure>().is_some());

    let mut plan = plan;
    plan.increments.first_mut().unwrap().name = "absent".to_owned();
    let error = apply_plan(
        &plan,
        false,
        Verbose::new(false),
        |_, _| panic!(),
        || Ok(work_tree()),
        |_| panic!(),
        |_| panic!(),
    )
    .unwrap_err();
    assert!(error.find_source::<UnknownPlanTargetError>().is_some());
}

#[test]
fn read_and_parse_failures_discard_earlier_computed_edits_without_writes() {
    for parse_failure in [false, true] {
        let originals = manifests();
        let mut reads = Vec::new();
        let paths = unique_paths();
        let last = paths.last().unwrap();
        let error = apply_plan(
            &plan(PlanStage::Proposed, false),
            false,
            Verbose::new(false),
            |_, _| panic!(),
            || Ok(work_tree()),
            |path| {
                reads.push(path.to_path_buf());
                if path == last {
                    if parse_failure {
                        Ok("[package".to_owned())
                    } else {
                        Err(ApplicationFailure::new().into())
                    }
                } else {
                    Ok(originals.get(path).unwrap().clone())
                }
            },
            |_| panic!(),
        )
        .unwrap_err();
        assert_eq!(reads, paths);
        if parse_failure {
            assert!(error.find_source::<ParseTomlError>().is_some());
        } else {
            assert!(error.find_source::<ApplicationFailure>().is_some());
        }
    }
}

#[test]
fn write_failure_stops_subsequent_writes_and_is_not_reported_as_success() {
    let mut writes = Vec::new();
    let error = apply_plan(
        &plan(PlanStage::Proposed, false),
        false,
        Verbose::new(false),
        |_, _| panic!(),
        || Ok(work_tree()),
        |path| Ok(manifests().get(path).unwrap().clone()),
        |edit| {
            writes.push(edit.path.clone());
            Err(ApplicationFailure::new().into())
        },
    )
    .unwrap_err();
    assert!(error.find_source::<ApplicationFailure>().is_some());
    assert_eq!(writes, [PathBuf::from("workspace").join("Cargo.toml")]);
}

fn plan(stage: PlanStage, captured: bool) -> PlanFile {
    let mut plan = PlanFile::new(
        stage,
        ["root", "api"]
            .into_iter()
            .map(|name| PlanIncrement {
                name: name.to_owned(),
                level: None,
                version: Some("0.2.0".to_owned()),
            })
            .collect(),
    );
    if captured {
        // Dispatch forwards capture contents unchanged; the captured-state validator owns them.
        plan.resolved = Some(
            serde_json::from_value(json!({
                "inputs": {
                    "root": "workspace", "manifest": "Cargo.toml",
                    "head": "head", "base": "base", "base_revision": "main",
                    "index": "", "paths": [], "digest": "inputs"
                },
                "files": [], "final_digest": "candidate", "versions": {},
                "evidence_manifest_path": "candidate/Cargo.toml"
            }))
            .unwrap(),
        );
    }
    plan
}

fn versions(version: &str) -> ResolvedVersions {
    ResolvedVersions {
        packages: ["root", "api", "helper"]
            .into_iter()
            .map(|name| (name.to_owned(), version.parse().unwrap()))
            .collect(),
    }
}

fn unique_paths() -> [PathBuf; 4] {
    ["", "api", "helper", "untouched"]
        .map(|member| PathBuf::from("workspace").join(member).join("Cargo.toml"))
}

fn work_tree() -> WorkTree {
    let paths = unique_paths();
    WorkTree {
        workspace_root: PathBuf::from("workspace"),
        packages: Vec::new(),
        version_targets: ["root", "api", "helper", "untouched"]
            .into_iter()
            .zip(&paths)
            .map(|(name, path)| VersionTarget {
                name: name.to_owned(),
                version: Version::new(0, 1, 0),
                manifest_path: path.clone(),
                publishable: name != "helper",
            })
            .collect(),
        exact_dependencies: Vec::new(),
        member_manifests: [
            &paths[0], &paths[1], &paths[2], &paths[1], &paths[0], &paths[3],
        ]
        .into_iter()
        .cloned()
        .collect(),
        members_by_dir: ["root", "api", "helper", "untouched"]
            .into_iter()
            .zip(&paths)
            .map(|(name, path)| (path.parent().unwrap().to_path_buf(), name.to_owned()))
            .collect(),
        groups: Groups::from_edges(
            ["root", "api", "helper", "untouched"].map(str::to_owned),
            [("helper".to_owned(), "api".to_owned())],
        ),
        installation: InstallationGraph::default(),
    }
}

fn manifests() -> BTreeMap<PathBuf, String> {
    unique_paths()
        .into_iter()
        .zip([
            concat!(
                "# workspace root\n[package]\nname = 'root'\nversion = '0.1.0' # local\n",
                "[workspace]\nmembers = ['api', 'helper', 'untouched']\n",
                "[workspace.package]\nversion = '0.1.0'\n",
                "[workspace.dependencies]\nalias = { package = 'api', path = 'api', version = '=0.1.0' }\n",
                "[dependencies]\napi = { path = 'api', version = '^0.1.0' }\n",
            ),
            "[package]\nname = 'api'\nversion = '0.1.0'\n",
            concat!(
                "[package]\nname = 'helper'\nversion = '0.1.0'\npublish = false\n",
                "[dependencies]\nalias = { workspace = true }\n",
                "[build-dependencies]\napi = { path = '../api', version = '=0.1.0' }\n",
                "[dev-dependencies]\napi = { path = '../api' }\n",
                "[target.'cfg(unix)'.dependencies.api]\npath = '../api'\nversion = '^0.1.0'\n",
            ),
            "# preserve spacing and quoting\n[package]\nname = 'untouched'\nversion  =  '0.1.0'\n",
        ])
        .map(|(path, text)| (path, text.to_owned()))
        .collect()
}

fn updated_manifests() -> BTreeMap<PathBuf, String> {
    let mut updated = manifests();
    let paths = unique_paths();
    updated.insert(
        paths[0].clone(),
        concat!(
            "# workspace root\n[package]\nname = 'root'\nversion = \"0.2.0\" # local\n",
            "[workspace]\nmembers = ['api', 'helper', 'untouched']\n",
            "[workspace.package]\nversion = '0.1.0'\n",
            "[workspace.dependencies]\nalias = { package = 'api', path = 'api', version = \"=0.2.0\" }\n",
            "[dependencies]\napi = { path = 'api', version = \"0.2.0\" }\n",
        )
        .to_owned(),
    );
    updated.insert(
        paths[1].clone(),
        "[package]\nname = 'api'\nversion = \"0.2.0\"\n".to_owned(),
    );
    updated.insert(
        paths[2].clone(),
        concat!(
            "[package]\nname = 'helper'\nversion = \"0.2.0\"\npublish = false\n",
            "[dependencies]\nalias = { workspace = true }\n",
            "[build-dependencies]\napi = { path = '../api', version = \"=0.2.0\" }\n",
            "[dev-dependencies]\napi = { path = '../api' }\n",
            "[target.'cfg(unix)'.dependencies.api]\npath = '../api'\nversion = \"0.2.0\"\n",
        )
        .to_owned(),
    );
    updated
}

fn manifest_only_summary(dry_run: bool) -> String {
    if dry_run {
        let mut summary = "Dry run: 3 manifests would change".to_owned();
        for path in &unique_paths()[..3] {
            write!(summary, "\n  {}", quote_path(&path.to_string_lossy())).unwrap();
        }
        summary.push_str("; the workspace lockfile would be left untouched");
        summary
    } else {
        "Updated 3 manifests and left the workspace lockfile untouched; use prepare and preview for a resolved release plan".to_owned()
    }
}

/// Distinguishes injected acquisition/application failures from successful empty work.
#[ohno::error]
struct ApplicationFailure;
