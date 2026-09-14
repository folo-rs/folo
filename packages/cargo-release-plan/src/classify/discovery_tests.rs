//! Released-content acquisition tests without full Cargo workspace classification.

#[cfg(unix)]
use std::os::unix::fs::symlink;

use super::*;
use crate::git::testing::{Repository, tree_entry};

#[test]
fn declared_resources_keep_the_first_archive_path_claim() {
    let mut released = HashMap::from([("README.md".to_string(), "pkg/README.md".to_string())]);
    let resources = BTreeMap::from([
        ("README.md".to_string(), "shared/README.md".to_string()),
        ("LICENSE".to_string(), "shared/LICENSE".to_string()),
    ]);
    add_resources(&mut released, resources.iter());
    assert_eq!(
        released,
        HashMap::from([
            ("README.md".to_string(), "pkg/README.md".to_string()),
            ("LICENSE".to_string(), "shared/LICENSE".to_string()),
        ])
    );
}

#[test]
fn only_released_anchor_symlinks_are_rejected() {
    let entries = [
        tree_entry("pkg/link", "120000"),
        tree_entry("pkg/plain", tree_mode(false)),
    ];
    let plain = HashMap::from([("plain".to_string(), "pkg/plain".to_string())]);
    reject_anchor_symlinks("pkg", &entries, &plain).unwrap();
    let released = HashMap::from([("link".to_string(), "pkg/link".to_string())]);
    let error = reject_anchor_symlinks("pkg", &entries, &released).unwrap_err();
    assert!(error.find_source::<SymlinkReleasedError>().is_some());
}

#[test]
fn optional_reads_separate_absence_from_failures_at_each_acquisition() {
    let path = Path::new("Cargo.lock");
    for kind in [io::ErrorKind::NotFound, io::ErrorKind::PermissionDenied] {
        let result = read_optional_bytes_with(path, "pkg", "Cargo.lock", Err(kind.into()), || {
            panic!("metadata rejection must precede reading")
        });
        if kind == io::ErrorKind::NotFound {
            assert_eq!(result.unwrap(), None);
        } else {
            assert!(result.unwrap_err().find_source::<ReadFileError>().is_some());
        }
        let result =
            read_optional_bytes_with(path, "pkg", "Cargo.lock", Ok(false), || Err(kind.into()));
        if kind == io::ErrorKind::NotFound {
            assert_eq!(result.unwrap(), None);
        } else {
            assert!(result.unwrap_err().find_source::<ReadFileError>().is_some());
        }
    }
    let error = read_optional_bytes_with(path, "pkg", "Cargo.lock", Ok(true), || {
        panic!("a symbolic link must not be read")
    })
    .unwrap_err();
    assert!(error.find_source::<SymlinkReleasedError>().is_some());
    assert_eq!(
        read_optional_bytes_with(path, "pkg", "Cargo.lock", Ok(false), || Ok(vec![])).unwrap(),
        Some(vec![])
    );
    assert_eq!(
        read_optional_bytes_with(
            path,
            "pkg",
            "Cargo.lock",
            Ok(false),
            || Ok(b"lock".to_vec())
        )
        .unwrap(),
        Some(b"lock".to_vec())
    );
}

#[test]
#[cfg_attr(miri, ignore = "uses real filesystem metadata and a Git index")]
fn work_tree_presence_and_hash_inputs_distinguish_missing_and_invalid_paths() {
    let fixture = Repository::new();
    let git = fixture.repo();
    fixture.write("pkg/present", b"contents");
    let paths = vec!["pkg/present".to_string(), "pkg/missing".to_string()];
    assert_eq!(present_in_work_tree(&git, &paths).unwrap(), ["pkg/present"]);
    let released = HashMap::from([
        ("present".to_string(), "pkg/present".to_string()),
        ("missing".to_string(), "pkg/missing".to_string()),
    ]);
    assert_eq!(
        validated_work_tree_files(&git, "pkg", &released, &WorkTreeModes::default()).unwrap(),
        [("present", "pkg/present")]
    );

    // An interior NUL is rejected by filesystem APIs on both supported hosts.
    // Unlike permissions, this failure does not depend on the test user's privileges.
    let invalid = "pkg/invalid\0path".to_string();
    let error = present_in_work_tree(&git, std::slice::from_ref(&invalid)).unwrap_err();
    assert!(error.find_source::<ReadFileError>().is_some());
    let error = validated_work_tree_files(
        &git,
        "pkg",
        &HashMap::from([("invalid".to_string(), invalid)]),
        &WorkTreeModes::default(),
    )
    .unwrap_err();
    assert!(error.find_source::<ReadFileError>().is_some());

    fixture.command(&["add", "pkg/present"]);
    let id = git.hash_objects(&["pkg/present"]).unwrap().remove(0);
    fixture.command(&[
        "update-index",
        "--cacheinfo",
        &format!("120000,{id},pkg/present"),
    ]);
    // core.symlinks=false can leave an indexed link as a regular file on disk.
    // The index must reject that path before the regular-file metadata can accept it.
    fixture.command(&["config", "core.symlinks", "false"]);
    let modes = git.work_tree_modes(&["pkg"]).unwrap();
    let error = validated_work_tree_files(&git, "pkg", &released, &modes).unwrap_err();
    assert!(error.find_source::<SymlinkReleasedError>().is_some());
}

#[test]
fn filesystem_link_metadata_stops_hash_input_selection() {
    let root = Path::new("repository");
    let released = HashMap::from([("link".to_string(), "pkg/link".to_string())]);
    let error =
        validated_work_tree_files_with(root, "pkg", &released, &WorkTreeModes::default(), |path| {
            assert_eq!(path, root.join("pkg/link"));
            Ok(true)
        })
        .unwrap_err();
    assert!(error.find_source::<SymlinkReleasedError>().is_some());
}

#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "creates real filesystem symbolic links")]
fn filesystem_links_are_present_but_never_hashable_even_when_dangling() {
    let fixture = Repository::new();
    fixture.write("pkg/target", b"contents");
    symlink("target", fixture.path().join("pkg/link")).unwrap();
    symlink("missing", fixture.path().join("pkg/dangling")).unwrap();
    let git = fixture.repo();
    for path in ["pkg/link", "pkg/dangling"] {
        assert_eq!(
            present_in_work_tree(&git, &[path.to_string()]).unwrap(),
            [path]
        );
        let released = HashMap::from([("link".to_string(), path.to_string())]);
        let error = validated_work_tree_files(&git, "pkg", &released, &WorkTreeModes::default())
            .unwrap_err();
        assert!(error.find_source::<SymlinkReleasedError>().is_some());
    }
}

#[test]
#[cfg_attr(miri, ignore = "queries real tracked and untracked Git paths")]
fn work_tree_selection_and_untracked_advice_share_packaging_boundaries() {
    let fixture = Repository::new();
    for path in [
        "pkg/src/lib.rs",
        "pkg/src/nested/Cargo.toml",
        "pkg/src/nested/lib.rs",
        "pkg/README.txt",
        "shared/LICENSE",
    ] {
        fixture.write(path, b"tracked");
    }
    fixture.command(&["add", "pkg", "shared"]);
    for path in [
        "pkg/src/new.rs",
        "pkg/src/nested/new.rs",
        "pkg/src/untracked/Cargo.toml",
        "pkg/src/untracked/lib.rs",
        "pkg/README.md",
        "shared/NOTICE",
    ] {
        fixture.write(path, b"untracked");
    }
    let resources = BTreeMap::from([
        ("LICENSE".to_string(), "shared/LICENSE".to_string()),
        ("NOTICE".to_string(), "shared/NOTICE".to_string()),
        ("MISSING".to_string(), "shared/MISSING".to_string()),
    ]);
    let manifest = parse_package_manifest(
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\ninclude = [\"src/\"]\n",
        "pkg/Cargo.toml",
        &WorkspaceInherit::default(),
    )
    .unwrap()
    .unwrap();
    let package = WorkPackage {
        manifest,
        manifest_path: fixture.path().join("pkg/Cargo.toml"),
        resources,
        dependencies: vec![],
        consumer_contract: true,
        has_lockfile_target: false,
    };
    let git = fixture.repo();
    assert_eq!(
        released_work_tree_paths(&git, &package, PathCase::Sensitive).unwrap(),
        BTreeSet::from([
            "src/lib.rs".to_string(),
            "README.txt".to_string(),
            "LICENSE".to_string(),
        ])
    );
    let side = work_tree_side(&package, PathCase::Sensitive);
    let tracked = git.ls_files("pkg").unwrap();
    let tracked_resources = BTreeMap::from([("LICENSE".to_string(), "shared/LICENSE".to_string())]);
    assert_eq!(
        untracked_released(&git, &side, &tracked_resources, &tracked).unwrap(),
        ["NOTICE", "README.md", "src/new.rs"]
    );
}

#[test]
#[cfg_attr(miri, ignore = "compares real Git objects and index modes")]
fn package_diff_preserves_presence_modes_content_and_external_resources() {
    let fixture = Repository::new();
    for (path, bytes) in [
        ("pkg/deleted", b"deleted\n".as_slice()),
        ("pkg/modified", b"before\n"),
        ("pkg/mode-only", b"\0binary"),
        ("pkg/unchanged", b"same\n"),
        ("shared/LICENSE", b"license\n"),
    ] {
        fixture.write(path, bytes);
    }
    fixture.command(&["add", "pkg", "shared"]);
    fixture.command(&["update-index", "--chmod=+x", "pkg/deleted"]);
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fixture.write("pkg/modified", b"after\n");
    fixture.write("pkg/added", b"added\n");
    fixture.command(&["add", "pkg/added"]);
    fixture.command(&[
        "update-index",
        "--chmod=+x",
        "pkg/mode-only",
        "shared/LICENSE",
    ]);
    fs::remove_file(fixture.path().join("pkg/deleted")).unwrap();
    let resources = BTreeMap::from([("LICENSE".to_string(), "shared/LICENSE".to_string())]);
    let rules = PackagingRules::default();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, patch, stat, untracked) =
        diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();
    let changes: Vec<_> = changed
        .iter()
        .map(|item| match item {
            ChangedItem::Package { path, change } => (path.as_str(), change.as_str()),
            _ => panic!("file comparison only produces package changes"),
        })
        .collect();
    assert_eq!(
        changes,
        [
            ("LICENSE", "modified"),
            ("added", "added"),
            ("deleted", "deleted"),
            ("mode-only", "modified"),
            ("modified", "modified"),
        ]
    );
    assert_eq!((stat.files, stat.insertions, stat.deletions), (5, 2, 2));
    assert!(untracked.is_empty());
    assert!(patch.contains("+added\n"));
    assert!(patch.contains("-deleted\n"));
    assert!(patch.contains("-before\n+after\n"));
    assert!(!patch.contains("Binary files"));
    // Additions and deletions have presence headers, not an extra mode transition.
    assert_eq!(patch.matches("old mode").count(), 2);
    assert_eq!(patch.matches("new mode").count(), 2);
    assert!(patch.contains("deleted file mode 100755"));
    assert!(patch.contains("new file mode 100644"));
}

#[test]
#[cfg_attr(miri, ignore = "discovers a nested real Git workspace")]
fn root_manifest_uses_the_discovered_workspace_prefix() {
    let fixture = Repository::new();
    assert_eq!(root_manifest_rel(&fixture.repo()), "Cargo.toml");
    fs::create_dir_all(fixture.path().join("workspace")).unwrap();
    let git = GitRepo::discover(&fixture.path().join("workspace")).unwrap();
    assert_eq!(root_manifest_rel(&git), "workspace/Cargo.toml");
}
