//! Native ownership queries use explicit roots and never change process-global CWD.

use std::fs;
use std::path::Path;

use cargo_detect_package::query_package;

#[test]
#[cfg_attr(
    miri,
    ignore = "Native filesystem ownership and deleted-path resolution."
)]
fn query_handles_existing_deleted_and_standalone_workspace_paths() {
    let workspace = tempfile::tempdir().unwrap();
    fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname='root'",
    )
    .unwrap();
    fs::create_dir_all(workspace.path().join("member")).unwrap();
    fs::write(
        workspace.path().join("member").join("Cargo.toml"),
        "[package]\nname='member'",
    )
    .unwrap();
    assert_eq!(
        query_package(workspace.path(), &Path::new("member").join("deleted.rs")).unwrap(),
        Some("member".to_owned())
    );
    assert_eq!(
        query_package(workspace.path(), Path::new("Cargo.toml")).unwrap(),
        None
    );
    assert_eq!(
        query_package(workspace.path(), Path::new("removed-package")).unwrap(),
        None
    );
    query_package(workspace.path(), workspace.path().parent().unwrap()).unwrap_err();
    fs::write(
        workspace.path().join("member").join("Cargo.toml"),
        "invalid = [",
    )
    .unwrap();
    query_package(workspace.path(), Path::new("member")).unwrap_err();
}

::testing::set_allocator!();
