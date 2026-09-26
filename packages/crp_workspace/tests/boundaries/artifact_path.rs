//! External acquisition for `artifact_path`.

use std::fs;
#[cfg(unix)]
use std::io::{Error as IoError, ErrorKind};
#[cfg(unix)]
use std::os::unix::fs::symlink;

use crp_workspace::artifact_path::*;
use tempfile::tempdir;
#[cfg(unix)]
use tempfile::tempdir_in;

#[test]
#[cfg_attr(miri, ignore = "resolves filesystem paths with missing ancestors")]
fn missing_parent_components_cannot_hide_an_input_alias() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("report.json");
    fs::write(&input, "evidence").unwrap();
    let alias = directory
        .path()
        .join("missing")
        .join("..")
        .join("report.json");
    assert!(same_path(&input, &alias).unwrap());
    assert!(!same_path(&input, &directory.path().join("new").join("plan.json")).unwrap());
    assert!(!directory.path().join("missing").exists());
}

#[test]
#[cfg_attr(miri, ignore = "checks filesystem errors for a non-directory ancestor")]
fn an_unusable_ancestor_is_an_error_not_a_different_path() {
    let directory = tempdir().unwrap();
    let file = directory.path().join("file");
    fs::write(&file, "not a directory").unwrap();
    assert!(
        resolve_path(&file.join("plan.json"))
            .unwrap_err()
            .find_source::<std::io::Error>()
            .is_some()
    );
    _ = resolve_path(
        &directory
            .path()
            .join("new")
            .join("..")
            .join("file")
            .join("plan.json"),
    )
    .unwrap_err();
}

#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "checks parent traversal through a regular file")]
fn parent_traversal_cannot_escape_a_non_directory() {
    let directory = tempdir().unwrap();
    let file = directory.path().join("file");
    fs::write(&file, "not a directory").unwrap();
    let error = resolve_path(
        &directory
            .path()
            .join("missing")
            .join("..")
            .join("file")
            .join("..")
            .join("plan.json"),
    )
    .unwrap_err();
    assert_eq!(
        error.find_source::<IoError>().unwrap().kind(),
        ErrorKind::NotADirectory
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "not a directory");
    assert!(!directory.path().join("missing").exists());
}

#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "creates a directory symlink")]
fn existing_symlink_ancestors_are_resolved_before_missing_components() {
    // Exercise a noncanonical temporary root regardless of the host's temporary directory.
    let root = tempdir().unwrap();
    let root_alias = root.path().join("root-alias");
    symlink(root.path(), &root_alias).unwrap();
    let directory = tempdir_in(&root_alias).unwrap();
    assert_ne!(
        directory.path(),
        fs::canonicalize(directory.path()).unwrap()
    );
    let real = directory.path().join("nested").join("real");
    fs::create_dir_all(&real).unwrap();
    let alias = directory.path().join("alias");
    symlink(&real, &alias).unwrap();
    let input = real.join("report.json");
    fs::write(&input, "evidence").unwrap();
    assert!(same_path(&input, &alias.join("report.json")).unwrap());
    assert!(same_path(&input, &alias.join("new").join("..").join("report.json")).unwrap());
    assert!(
        same_path(
            &input,
            &directory
                .path()
                .join("missing")
                .join("..")
                .join("alias")
                .join("report.json")
        )
        .unwrap()
    );
    let parent_input = real.parent().unwrap().join("parent.json");
    fs::write(&parent_input, "parent evidence").unwrap();
    assert!(
        same_path(
            &parent_input,
            &directory
                .path()
                .join("missing")
                .join("..")
                .join("alias")
                .join("..")
                .join("parent.json")
        )
        .unwrap()
    );
    assert_eq!(
        resolve_path(&alias.join("new").join("plan.json")).unwrap(),
        fs::canonicalize(&real)
            .unwrap()
            .join("new")
            .join("plan.json")
    );
}
