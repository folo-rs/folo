//! In-process manifest transformation before prospective resolution.

use std::cell::RefCell;

use fixture::*;
use toml_edit::TomlError;

use super::*;

#[test]
fn edits_include_root_and_unique_members_with_complete_rewritten_contents() {
    let tree = work_tree();
    let originals = manifests();
    let reads = RefCell::new(Vec::new());
    let edits = compute_edits_with(
        &tree,
        &versions("0.2.0"),
        Verbose::new(false, &crp_diag::Discard),
        |path| {
            reads.borrow_mut().push(path.to_path_buf());
            Ok(originals.get(path).unwrap().clone())
        },
    )
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
    let edits = compute_edits_with(
        &tree,
        &versions("0.2.0"),
        Verbose::new(false, &crp_diag::Discard),
        |_| Ok(original.to_owned()),
    )
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
        Verbose::new(false, &crp_diag::Discard),
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
        Verbose::new(false, &crp_diag::Discard),
        |path| Ok(originals.get(path).unwrap().clone()),
    )
    .unwrap();

    assert_eq!(edits.len(), originals.len());
    for edit in &edits {
        assert_eq!(&edit.original, originals.get(&edit.path).unwrap());
        assert_eq!(&edit.updated, originals.get(&edit.path).unwrap());
    }
}

#[test]
fn read_failure_discards_earlier_computed_edits() {
    let error = failed_edit_computation(Err(ApplicationFailure::new().into()));
    assert!(error.find_source::<ApplicationFailure>().is_some());
}

#[test]
fn parse_failure_discards_earlier_computed_edits() {
    let error = failed_edit_computation(Ok("[package".to_owned()));
    assert!(error.find_source::<TomlError>().is_some());
}

mod fixture;
