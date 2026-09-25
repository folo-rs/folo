//! In-process application dispatch, manifest transformation and write ordering.

use std::cell::{Cell, RefCell};

use fixture::*;

use super::*;
use crate::{ParseTomlError, UnknownPlanTargetError};

#[test]
fn expanded_or_captured_plans_dispatch_without_manifest_only_operations() {
    for (stage, captured) in [
        (PlanStage::Proposed, true),
        (PlanStage::Expanded, false),
        (PlanStage::Expanded, true),
    ] {
        let plan = plan(stage, captured);
        for dry_run in [false, true] {
            let output = apply_plan(
                &plan,
                dry_run,
                Verbose::new(false),
                |received, received_dry_run| {
                    assert_eq!(received, &plan);
                    assert_eq!(received_dry_run, dry_run);
                    Ok("captured result".to_owned())
                },
                || panic!(),
                |_| panic!(),
                |_| panic!(),
            )
            .unwrap();
            assert_eq!(output, "captured result");
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
fn root_edit_rewrites_dependencies_without_changing_shared_package_version() {
    let original = concat!(
        "# workspace root\n[package]\nname = 'root'\nversion = '0.1.0' # local\n",
        "[workspace.package]\nversion = '0.1.0'\n",
        "[workspace.dependencies]\nalias = { package = 'api', path = 'api', version = '=0.1.0' }\n",
        "[dependencies]\napi = { path = 'api', version = '^0.1.0' }\n",
    );
    let expected = concat!(
        "# workspace root\n[package]\nname = 'root'\nversion = \"0.2.0\" # local\n",
        "[workspace.package]\nversion = '0.1.0'\n",
        "[workspace.dependencies]\nalias = { package = 'api', path = 'api', version = \"=0.2.0\" }\n",
        "[dependencies]\napi = { path = 'api', version = \"0.2.0\" }\n",
    );
    let mut tree = work_tree();
    tree.member_manifests.clear();
    let edits = compute_edits_with(&tree, &versions("0.2.0"), Verbose::new(false), |_| {
        Ok(original.to_owned())
    })
    .unwrap();
    assert_eq!(edits.len(), 1);
    let edit = edits.first().unwrap();
    assert_eq!(edit.original, original);
    assert_eq!(edit.updated, expected);
}

#[test]
fn member_edit_rewrites_each_dependency_kind_and_preserves_versionless_entries() {
    let original = concat!(
        "[package]\nname = 'helper'\nversion = '0.1.0'\npublish = false\n",
        "[dependencies]\nalias = { workspace = true }\n",
        "[build-dependencies]\napi = { path = '../api', version = '=0.1.0' }\n",
        "[dev-dependencies]\napi = { path = '../api' }\n",
        "[target.'cfg(unix)'.dependencies.api]\npath = '../api'\nversion = '^0.1.0'\n",
    );
    let expected = concat!(
        "[package]\nname = 'helper'\nversion = \"0.2.0\"\npublish = false\n",
        "[dependencies]\nalias = { workspace = true }\n",
        "[build-dependencies]\napi = { path = '../api', version = \"=0.2.0\" }\n",
        "[dev-dependencies]\napi = { path = '../api' }\n",
        "[target.'cfg(unix)'.dependencies.api]\npath = '../api'\nversion = \"0.2.0\"\n",
    );
    let mut tree = work_tree();
    let path = tree.workspace_root.join("helper").join("Cargo.toml");
    tree.member_manifests = vec![path.clone()];
    let edits = compute_edits_with(
        &tree,
        &versions("0.2.0"),
        Verbose::new(false),
        |read_path| {
            // This scenario only transforms the member; the virtual root has no declarations.
            Ok(if read_path == path { original } else { "" }.to_owned())
        },
    )
    .unwrap();
    assert_eq!(edits.len(), 2);
    let edit = edits.last().unwrap();
    assert_eq!(edit.path, path);
    assert_eq!(edit.original, original);
    assert_eq!(edit.updated, expected);
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
    let originals = manifests();
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
        |path| Ok(originals.get(path).unwrap().clone()),
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
fn read_failure_discards_earlier_computed_edits_without_writes() {
    let error = failed_manifest_application(Err(ApplicationFailure::new().into()));
    assert!(error.find_source::<ApplicationFailure>().is_some());
}

#[test]
fn parse_failure_discards_earlier_computed_edits_without_writes() {
    let error = failed_manifest_application(Ok("[package".to_owned()));
    assert!(error.find_source::<ParseTomlError>().is_some());
}

#[test]
fn write_failure_stops_subsequent_writes_and_is_not_reported_as_success() {
    let originals = manifests();
    let mut writes = Vec::new();
    let error = apply_plan(
        &plan(PlanStage::Proposed, false),
        false,
        Verbose::new(false),
        |_, _| panic!(),
        || Ok(work_tree()),
        |path| Ok(originals.get(path).unwrap().clone()),
        |edit| {
            writes.push(edit.path.clone());
            Err(ApplicationFailure::new().into())
        },
    )
    .unwrap_err();
    assert!(error.find_source::<ApplicationFailure>().is_some());
    assert_eq!(writes, [PathBuf::from("workspace").join("Cargo.toml")]);
}

mod fixture;
