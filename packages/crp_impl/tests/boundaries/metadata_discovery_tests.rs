//! External acquisition for metadata.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;

use crp_impl::ReadFileError;
use crp_impl::manifest::{PathCase, WorkspaceInherit, parse_package_manifest};
use crp_impl::metadata::*;

use crate::git_fixture::Repository;

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
    assert!(error.find_source::<ReadFileError>().is_some());
}
