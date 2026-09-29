//! External acquisition for `artifact_path`.

use std::fs;
use std::io::Write as _;
#[cfg(unix)]
use std::io::{Error as IoError, ErrorKind};

use crp_workspace::artifact_path::*;
use tempfile::tempdir;

#[test]
#[cfg_attr(miri, ignore = "Creates and promotes owned temporary artifact files")]
fn artifact_promotion_preserves_existing_files_and_discards_failed_writes() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("nested").join("report.json");
    write_new(&output, |file| Ok(file.write_all(b"original")?)).unwrap();
    let error = write_new(&output, |file| Ok(file.write_all(b"replacement")?)).unwrap_err();
    assert!(error.find_source::<tempfile::PersistError>().is_some());
    drop(error);
    assert_eq!(fs::read(&output).unwrap(), b"original");

    let failed = output.with_file_name("failed.json");
    let error = write_new(&failed, |file| {
        file.write_all(b"partial")?;
        Err(std::io::Error::other("serialization canary").into())
    })
    .unwrap_err();
    assert!(error.find_source::<std::io::Error>().is_some());
    assert!(!failed.exists());
    assert_eq!(fs::read_dir(output.parent().unwrap()).unwrap().count(), 1);

    let error = write_new(&output.join("child.json"), |_| {
        panic!("an invalid parent must fail before invoking the writer")
    })
    .unwrap_err();
    assert!(error.find_source::<std::io::Error>().is_some());
    assert_eq!(fs::read(&output).unwrap(), b"original");
}

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
