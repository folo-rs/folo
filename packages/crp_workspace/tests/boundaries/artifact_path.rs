//! External acquisition for `artifact_path`.

use std::fs;
use std::io::Write as _;
#[cfg(unix)]
use std::io::{Error as IoError, ErrorKind};
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;
#[cfg(windows)]
use std::process::Command;

use crp_workspace::artifact_path::*;
use tempfile::tempdir;

#[test]
#[cfg_attr(
    miri,
    ignore = "resolves real filesystem aliases and nonexistent output suffixes"
)]
fn output_admission_resolves_aliases_without_creating_destinations() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("lib.rs"), "source").unwrap();
    let alias = directory.path().join("alias");
    directory_alias(&source, &alias);
    for output in [
        alias.join("missing/evidence"),
        directory.path().join("new/../source/evidence"),
    ] {
        admit_output(&output, [source.clone()], ["diffs"]).unwrap_err();
        assert!(!output.exists());
    }
    assert!(!directory.path().join("new").exists());
    let other = directory.path().join("excluded");
    fs::create_dir_all(&other).unwrap();
    let other_alias = directory.path().join("excluded-alias");
    directory_alias(&other, &other_alias);
    admit_output(&other_alias.join("evidence"), [source.clone()], ["diffs"]).unwrap();
    assert!(!other.join("evidence").exists());
    directory_alias(&source, &other.join("unrelated"));
    admit_output(&other, [source.clone()], ["diffs"]).unwrap();
    // Output subtree replacement must not operate through an existing redirected child.
    directory_alias(&source, &other.join("diffs"));
    admit_output(&other, [source.clone()], ["diffs"]).unwrap_err();
    assert_eq!(fs::read_to_string(source.join("lib.rs")).unwrap(), "source");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "probes native directory case before checking missing source aliases"
)]
fn output_admission_uses_observed_case_for_absent_reserved_directories() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("Cargo.toml"), "probe").unwrap();
    let insensitive = directory.path().join("cARGO.TOML").try_exists().unwrap();
    let output = directory.path().join("SRC/evidence");
    let result = admit_output(&output, [directory.path().join("src")], []);
    assert_eq!(result.is_err(), insensitive);
    assert!(!output.exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[cfg(unix)]
fn directory_alias(source: &Path, alias: &Path) {
    symlink(source, alias).unwrap();
}

#[cfg(windows)]
fn directory_alias(source: &Path, alias: &Path) {
    // Junctions exercise Windows aliases without requiring symbolic-link privileges.
    let output = Command::new("pwsh")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:CRP_TEST_ALIAS -Target $env:CRP_TEST_SOURCE | Out-Null",
        ])
        .env("CRP_TEST_SOURCE", source)
        .env("CRP_TEST_ALIAS", alias)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

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
