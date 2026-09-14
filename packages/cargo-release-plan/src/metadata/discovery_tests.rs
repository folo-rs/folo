//! Tests of tracked metadata eligibility without invoking Cargo discovery.

#[cfg(unix)]
use std::os::unix::fs::symlink;

use super::*;
use crate::git::testing::Repository;
use crate::manifest::parse_package_manifest;

#[test]
#[cfg_attr(miri, ignore = "uses a real filesystem fixture")]
fn only_tracked_manifests_are_workspace_members() {
    let fixture = Repository::new();
    let git = fixture.repo();
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: fixture.path(),
        paths: vec!["pkg/Cargo.toml".to_string()],
        case: PathCase::Sensitive,
    };
    assert!(tracked.contains_manifest(&fixture.path().join("pkg/Cargo.toml").to_string_lossy()));
    assert!(
        !tracked.contains_manifest(
            &fixture
                .path()
                .join("untracked/Cargo.toml")
                .to_string_lossy()
        )
    );
    assert!(!tracked.contains_manifest("outside/Cargo.toml"));
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
    assert!(error.find_source::<ReadFileError>().is_some());
}
