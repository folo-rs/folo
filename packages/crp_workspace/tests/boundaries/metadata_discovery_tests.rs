//! External acquisition for metadata.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;

use crp_workspace::manifest::{PathCase, WorkspaceInherit, parse_package_manifest};
use crp_workspace::metadata::*;

use crate::git_fixture::Repository;

#[test]
#[cfg_attr(miri, ignore = "Reads Cargo metadata from tracked package fixtures")]
fn tracked_packages_reject_reserved_metadata_typos_before_projection() {
    for (source, publish) in [("lib.rs", true), ("main.rs", true), ("lib.rs", false)] {
        let fixture = Repository::new();
        fixture.write(
            "Cargo.toml",
            format!(
                "[package]\nname='package'\nversion='1.0.0'\npublish={publish}\n\
                 [package.metadata.release-plan]\nrelease-targtes=['x86_64-pc-windows-msvc']\n"
            )
            .as_bytes(),
        );
        fixture.write(&format!("src/{source}"), b"fn main() {}\n");
        fixture.command(&["add", "."]);
        let error = load_tracked_work_tree(&fixture.path().join("Cargo.toml")).unwrap_err();
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("unknown metadata.release-plan key"));
        assert!(diagnostic.contains("release-targtes"));
    }
}

#[test]
#[cfg_attr(miri, ignore = "changes real filesystem target presence and type")]
fn automatic_installable_targets_require_tracked_present_regular_files() {
    let fixture = Repository::new();
    let git = fixture.repo();
    let manifest = parse_package_manifest(
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n",
        "pkg/Cargo.toml",
        &WorkspaceInherit::default(),
    )
    .unwrap()
    .unwrap();
    let mut tracked = TrackedMetadata {
        git: &git,
        workspace_root: fixture.path(),
        paths: vec![
            "pkg/src/lib.rs".to_string(),
            "pkg/examples/demo.rs".to_string(),
            "other/src/main.rs".to_string(),
        ],
        case: PathCase::Sensitive,
    };
    for path in &tracked.paths {
        fixture.write(path, b"source");
    }
    assert!(!tracked.has_lockfile_target(&manifest).unwrap());

    fixture.write("pkg/src/main.rs", b"binary");
    assert!(!tracked.has_lockfile_target(&manifest).unwrap());
    tracked.paths.push("pkg/src/main.rs".to_string());
    assert!(tracked.has_lockfile_target(&manifest).unwrap());

    fs::remove_file(fixture.path().join("pkg/src/main.rs")).unwrap();
    assert!(!tracked.has_lockfile_target(&manifest).unwrap());
    fs::create_dir_all(fixture.path().join("pkg/src/main.rs")).unwrap();
    assert!(!tracked.has_lockfile_target(&manifest).unwrap());
    fs::remove_dir(fixture.path().join("pkg/src/main.rs")).unwrap();

    #[cfg(unix)]
    {
        symlink("lib.rs", fixture.path().join("pkg/src/main.rs")).unwrap();
        assert!(!tracked.has_lockfile_target(&manifest).unwrap());
    }

    // Invalid input produces an operational error independently of user privileges.
    tracked.paths.push("pkg/invalid\0path".to_string());
    let error = tracked.has_lockfile_target(&manifest).unwrap_err();
    assert!(error.find_source::<std::io::Error>().is_some());
}
