//! External acquisition for classify.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;

use crp_diag::{Discard, Verbose};
use crp_versioning::classify::*;
use crp_versioning::resolved::Inputs;
use crp_workspace::git::GitRepo;
use crp_workspace::manifest::{
    PathCase, WorkspaceInherit, package_manifest_from_document, parse_document,
};
use crp_workspace::metadata::WorkPackage;
use crp_workspace::packaging::PackagingRules;

use crate::git_fixture::Repository;

#[test]
#[cfg_attr(
    miri,
    ignore = "classifies Git replacement changes across fresh acquisitions"
)]
fn reused_snapshots_follow_changed_git_interpretation() {
    let fixture = Repository::new();
    fixture.write(
        "Cargo.toml",
        b"[workspace]\nmembers=['member']\nresolver='3'\n",
    );
    let package = "[package]\nname='member'\nversion='1.0.0'\nedition='2024'\n";
    fixture.write("member/Cargo.toml", package.as_bytes());
    fixture.write("member/src/lib.rs", b"pub fn value() {}\n");
    fixture.command(&["add", "-A"]);
    fixture.command(&["commit", "--quiet", "-m", "root"]);
    let git = fixture.repo();
    let root = git.rev_parse("HEAD").unwrap();
    fixture.write(
        "member/Cargo.toml",
        package.replace("1.0.0", "1.1.0").as_bytes(),
    );
    fixture.command(&["add", "-A"]);
    fixture.command(&["commit", "--quiet", "-m", "release"]);
    let head = git.rev_parse("HEAD").unwrap();
    let manifest = fixture.path().join("Cargo.toml");
    let mut snapshots = Snapshots::default();
    let mut observe = || {
        classify_with_snapshots(
            &manifest,
            Some(&head),
            None,
            Verbose::new(false, &Discard),
            &mut snapshots,
        )
        .unwrap()
        .packages
        .remove(0)
        .status()
    };
    assert_eq!(observe(), PackageStatus::Unchanged);
    fixture.command(&["replace", &head, &root]);
    assert_eq!(observe(), PackageStatus::PendingRelease);
    fixture.command(&["replace", "-d", &head]);
    assert_eq!(observe(), PackageStatus::Unchanged);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "classifies real Git history with offline Cargo metadata"
)]
fn historical_manifest_batches_decode_only_selected_members() {
    let fixture = Repository::new();
    fixture.write(
        "Cargo.toml",
        b"[workspace]\nmembers=['member space']\nresolver='3'\n",
    );
    fixture.write(
        "member space/Cargo.toml",
        b"[package]\nname='member'\nversion='1.0.0'\nedition='2024'\n",
    );
    fixture.write("member space/src/lib.rs", b"pub fn value() {}\n");
    fixture.write("unrelated/Cargo.toml", b"\xff invalid UTF-8 and TOML");
    fixture.command(&["add", "-A"]);
    fixture.command(&["commit", "--quiet", "-m", "historical manifests"]);
    let manifest = fixture.path().join("Cargo.toml");
    let initial = classify(&manifest, Some("HEAD"), Verbose::new(false, &Discard)).unwrap();
    assert_eq!(initial.packages.len(), 1);
    assert_eq!(
        initial.packages.first().unwrap().status(),
        PackageStatus::Unchanged
    );
    drop(initial);
    fixture.write("member space/src/lib.rs", b"pub fn changed() {}\n");
    let changed = classify(&manifest, Some("HEAD"), Verbose::new(false, &Discard)).unwrap();
    let changed = changed.packages.first().unwrap();
    assert_eq!(changed.status(), PackageStatus::NeedsIncrement);
    assert!(!changed.patch().is_empty());
}

#[test]
#[cfg_attr(miri, ignore = "captures, classifies and mutates a real Git workspace")]
fn acquired_classification_reuses_one_interval_and_observes_the_next_mutation() {
    let fixture = Repository::new();
    fixture.write(
        "Cargo.toml",
        b"[workspace]\nmembers=['pkg']\nresolver='3'\n",
    );
    fixture.write(
        "pkg/Cargo.toml",
        b"[package]\nname='pkg'\nversion='1.0.0'\nedition='2024'\n\
          readme='../shared/README.md'\ninclude=['src/']\n",
    );
    fixture.write("pkg/src/lib.rs", b"pub fn original() {}\n");
    fixture.write("pkg/src/nested/Cargo.toml", b"[workspace]\n");
    fixture.write(
        "pkg/src/nested/lib.rs",
        b"not released by the outer package",
    );
    fixture.write("shared/README.md", b"external resource\n");
    fixture.command(&["add", "-A"]);
    fixture.command(&["commit", "--quiet", "-m", "baseline"]);
    let manifest = fixture.path().join("Cargo.toml");
    let verbose = Verbose::new(false, &Discard);
    let mut snapshots = Snapshots::default();
    for pass in 0..2 {
        let (inputs, acquired) =
            Inputs::capture_with_snapshots(&manifest, Some("HEAD"), None, verbose, &mut snapshots)
                .unwrap();
        let classification = classify_acquired(acquired, verbose, &mut snapshots).unwrap();
        let package = classification.work_tree.packages.first().unwrap();
        let paths = classification.released_work_tree_paths(package).unwrap();
        let mut expected: BTreeSet<String> = ["Cargo.toml", "README.md", "src/lib.rs"]
            .map(str::to_owned)
            .into();
        if pass != 0 {
            expected.insert("src/added.rs".into());
        }
        assert_eq!(paths, expected);
        assert_eq!(
            paths,
            classification.released_work_tree_paths(package).unwrap()
        );
        let class = classification.packages.first().unwrap();
        if pass == 0 {
            assert_eq!(class.status(), PackageStatus::Unchanged);
            assert!(!paths.contains("src/added.rs"));
        } else {
            assert_eq!(class.status(), PackageStatus::NeedsIncrement);
            assert!(class.patch().contains("+pub fn changed() {}"));
            assert!(class.patch().contains("new mode 100755"));
            assert!(paths.contains("src/added.rs"));
            assert_eq!(class.untracked, ["src/untracked.rs"]);
            assert!(!paths.contains("src/untracked.rs"));
            assert!(!class.patch().contains("untracked"));
        }
        inputs.verify(&manifest, None).unwrap();
        drop(classification);
        fixture.write("pkg/src/lib.rs", b"pub fn changed() {}\n");
        fixture.write("pkg/src/added.rs", b"pub fn added() {}\n");
        fixture.command(&["add", "pkg/src/added.rs"]);
        fixture.command(&["update-index", "--chmod=+x", "pkg/src/lib.rs"]);
        fixture.write("pkg/src/untracked.rs", b"pub fn untracked() {}\n");
        fixture.write("pkg/src/nested/untracked.rs", b"not outer-package advice");
    }
}

#[test]
#[cfg_attr(miri, ignore = "uses real filesystem metadata and a Git index")]
fn work_tree_presence_distinguishes_missing_and_invalid_paths() {
    let fixture = Repository::new();
    let git = fixture.repo();
    fixture.write("pkg/present", b"contents");
    let paths = vec!["pkg/present".to_string(), "pkg/missing".to_string()];
    assert_eq!(present_in_work_tree(&git, &paths).unwrap(), ["pkg/present"]);

    // An interior NUL is rejected by filesystem APIs on both supported hosts.
    // Unlike permissions, this failure does not depend on the test user's privileges.
    let invalid = "pkg/invalid\0path".to_string();
    let error = present_in_work_tree(&git, std::slice::from_ref(&invalid)).unwrap_err();
    assert!(error.find_source::<std::io::Error>().is_some());
}

#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "creates real filesystem symbolic links")]
fn native_symlinks_are_dereferenced_for_package_comparison() {
    let fixture = Repository::new();
    fixture.write("shared/target", b"before\n");
    fs::create_dir_all(fixture.path().join("pkg")).unwrap();
    symlink("../shared/target", fixture.path().join("pkg/link")).unwrap();
    fixture.command(&["add", "pkg", "shared"]);
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fixture.write("shared/target", b"after\n");

    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (_, patch, stat, _) = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();
    assert_eq!(stat.files, 1);
    assert!(patch.contains("-before\n+after\n"));
}

#[cfg(unix)]
#[test]
#[cfg_attr(
    miri,
    ignore = "replaces a tracked directory with a real symbolic link"
)]
fn symlinked_target_ancestors_are_rejected_before_hashing() {
    let fixture = Repository::new();
    fixture.write("shared/target", b"before\n");
    fixture.command(&["add", "shared/target"]);
    stage_placeholder_symlink(&fixture, "pkg/link", "../shared/target");
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fs::remove_dir_all(fixture.path().join("shared")).unwrap();
    fixture.write("outside/target", b"outside\n");
    symlink("../outside", fixture.path().join("shared")).unwrap();

    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let error = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap_err();
    assert!(error.to_string().contains("symbolic-link directory"));
}

#[test]
#[cfg_attr(miri, ignore = "compares real Git symlink modes and work-tree files")]
fn placeholder_symlink_chains_use_terminal_content_and_mode() {
    let fixture = Repository::new();
    fixture.write("shared/target", b"before\n");
    fixture.command(&["add", "shared/target"]);
    stage_placeholder_symlink(&fixture, "shared/alias", "target");
    stage_placeholder_symlink(&fixture, "pkg/link", "../shared/alias");
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fixture.write("shared/target", b"after\n");
    fixture.command(&["update-index", "--chmod=+x", "shared/target"]);

    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, patch, stat, _) =
        diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();
    assert_eq!(
        changed,
        [ChangedItem::Package {
            path: "link".to_string(),
            change: "modified".to_string(),
        }]
    );
    assert_eq!((stat.files, stat.insertions, stat.deletions), (1, 1, 1));
    assert!(patch.contains("-before\n+after\n"));
    assert!(patch.contains("old mode 100644\nnew mode 100755"));
}

#[test]
#[cfg_attr(miri, ignore = "compares a tracked directory-link fixture")]
fn directory_links_duplicate_terminal_files_under_the_archive_alias() {
    let fixture = Repository::new();
    fixture.write("pkg/cycle/new/benchmark.json", b"before\n");
    fixture.write("pkg/cycle/new/estimates.json", b"estimates\n");
    fixture.command(&["add", "pkg/cycle/new"]);
    stage_placeholder_symlink(&fixture, "pkg/cycle/alias_to_new", "new");
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fixture.write("pkg/cycle/new/benchmark.json", b"after\n");

    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, patch, stat, _) =
        diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();
    let changed: BTreeSet<_> = changed
        .into_iter()
        .map(|change| match change {
            ChangedItem::Package { path, .. } => path,
            _ => panic!("file comparison only produces package changes"),
        })
        .collect();

    assert_eq!(
        changed,
        BTreeSet::from([
            "cycle/alias_to_new/benchmark.json".to_string(),
            "cycle/new/benchmark.json".to_string(),
        ])
    );
    assert_eq!((stat.files, stat.insertions, stat.deletions), (2, 2, 2));
    assert_eq!(patch.matches("-before\n+after\n").count(), 2);
}

#[test]
#[cfg_attr(miri, ignore = "compares a tracked cyclic directory-link fixture")]
fn directory_link_cycles_are_rejected_without_recursive_traversal() {
    let fixture = Repository::new();
    fixture.write("pkg/cycle/new/benchmark.json", b"benchmark\n");
    fixture.command(&["add", "pkg/cycle/new/benchmark.json"]);
    stage_placeholder_symlink(&fixture, "pkg/cycle/alias_to_new", "new");
    stage_placeholder_symlink(&fixture, "pkg/cycle/new/back_to_cycle", "..");
    fixture.command(&["commit", "--quiet", "-m", "directory cycle"]);

    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };

    _ = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap_err();
}

#[test]
#[cfg_attr(miri, ignore = "compares excluded tracked symbolic links")]
fn definitively_excluded_broken_and_cyclic_links_are_not_resolved() {
    let fixture = Repository::new();
    fixture.write("pkg/src/lib.rs", b"pub fn value() {}\n");
    fixture.command(&["add", "pkg/src/lib.rs"]);
    stage_placeholder_symlink(&fixture, "pkg/tests/broken", "missing");
    stage_placeholder_symlink(&fixture, "pkg/tests/loop", "loop");
    fixture.command(&["commit", "--quiet", "-m", "excluded links"]);

    let rules = PackagingRules::new(Some(&["src/**".to_string()]), None).unwrap();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, patch, stat, _) =
        diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();

    assert!(changed.is_empty());
    assert!(patch.is_empty());
    assert_eq!((stat.files, stat.insertions, stat.deletions), (0, 0, 0));
}

#[test]
#[cfg_attr(miri, ignore = "compares excluded recursive symbolic links")]
fn excluded_recursive_broken_and_cyclic_links_are_pruned_before_resolution() {
    let fixture = Repository::new();
    fixture.write("pkg/real/keep.json", b"keep\n");
    fixture.command(&["add", "pkg/real/keep.json"]);
    stage_placeholder_symlink(&fixture, "pkg/real/broken", "missing");
    stage_placeholder_symlink(&fixture, "pkg/real/loop", "loop");
    stage_placeholder_symlink(&fixture, "pkg/alias", "real");
    fixture.command(&["commit", "--quiet", "-m", "recursive excluded links"]);

    let rules = PackagingRules::new(
        None,
        Some(&[
            "real/**".to_string(),
            "alias/broken".to_string(),
            "alias/loop".to_string(),
        ]),
    )
    .unwrap();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, patch, stat, _) =
        diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();

    assert!(changed.is_empty());
    assert!(patch.is_empty());
    assert_eq!((stat.files, stat.insertions, stat.deletions), (0, 0, 0));
}

#[test]
#[cfg_attr(miri, ignore = "compares mandatory and regular paths under deny rules")]
fn subtree_pruning_preserves_mandatory_and_explicitly_released_files() {
    let fixture = Repository::new();
    fixture.write("pkg/Cargo.toml", b"before manifest\n");
    fixture.write("pkg/data", b"before data\n");
    fixture.command(&["add", "pkg"]);
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fixture.write("pkg/Cargo.toml", b"after manifest\n");
    fixture.write("pkg/data", b"after data\n");

    let rules = PackagingRules::new(
        None,
        Some(&["Cargo.toml".to_string(), "data/**".to_string()]),
    )
    .unwrap();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, _, _, _) = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();
    let changed: BTreeSet<_> = changed
        .into_iter()
        .map(|change| match change {
            ChangedItem::Package { path, .. } => path,
            _ => panic!("file comparison only produces package changes"),
        })
        .collect();

    assert_eq!(
        changed,
        BTreeSet::from(["Cargo.toml".to_string(), "data".to_string()])
    );
}

#[test]
#[cfg_attr(miri, ignore = "compares a directory link selected by brace syntax")]
fn brace_include_patterns_keep_possible_directory_link_roots() {
    let fixture = Repository::new();
    fixture.write("pkg/tests/real/file.json", b"before\n");
    fixture.command(&["add", "pkg/tests/real/file.json"]);
    stage_placeholder_symlink(&fixture, "pkg/tests/alias", "real");
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fixture.write("pkg/tests/real/file.json", b"after\n");

    let rules = PackagingRules::new(Some(&["tests/{alias,other}/**".to_string()]), None).unwrap();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, _, _, _) = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();

    assert!(matches!(
        changed.as_slice(),
        [ChangedItem::Package { path, .. }] if path == "tests/alias/file.json"
    ));
}

#[test]
#[cfg_attr(miri, ignore = "compares a nested slashless-excluded cyclic link")]
fn slashless_excludes_prune_matching_nested_link_components() {
    let fixture = Repository::new();
    fixture.write("pkg/src/lib.rs", b"pub fn value() {}\n");
    fixture.command(&["add", "pkg/src/lib.rs"]);
    stage_placeholder_symlink(&fixture, "pkg/tests/fixtures/loop", "loop");
    fixture.command(&["commit", "--quiet", "-m", "nested excluded link"]);

    for pattern in ["fixtures", "fixtures/"] {
        let rules = PackagingRules::new(None, Some(&[pattern.to_string()])).unwrap();
        let resources = BTreeMap::new();
        let side = PackageSide {
            dir: "pkg",
            rules: &rules,
            resources: &resources,
            auto_readme: false,
            case: PathCase::Sensitive,
        };

        _ = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "compares modes and nested links in a tracked directory outside the package"
)]
fn outside_directory_targets_acquire_child_modes_and_links() {
    let fixture = Repository::new();
    fixture.write("shared/tool", b"tool\n");
    fixture.command(&["add", "shared/tool"]);
    fixture.command(&["update-index", "--chmod=+x", "shared/tool"]);
    stage_placeholder_symlink(&fixture, "shared/alias", "tool");
    stage_placeholder_symlink(&fixture, "pkg/link", "../shared");
    fixture.command(&["commit", "--quiet", "-m", "outside directory target"]);

    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, patch, stat, _) =
        diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();

    assert!(changed.is_empty());
    assert!(patch.is_empty());
    assert_eq!((stat.files, stat.insertions, stat.deletions), (0, 0, 0));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "replaces a tracked directory with an untracked regular file"
)]
fn inferred_directory_targets_reject_untracked_file_replacements() {
    let fixture = Repository::new();
    fixture.write("shared/dir/old.txt", b"old\n");
    fixture.command(&["add", "shared/dir/old.txt"]);
    stage_placeholder_symlink(&fixture, "pkg/link", "../shared/dir");
    fixture.command(&["commit", "--quiet", "-m", "directory target"]);
    fs::remove_dir_all(fixture.path().join("shared/dir")).unwrap();
    fixture.write("shared/dir", b"untracked replacement\n");

    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };

    _ = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap_err();
}

#[test]
#[cfg_attr(
    miri,
    ignore = "replaces a recursively inferred directory with an untracked regular file"
)]
fn recursive_inferred_directories_reject_untracked_file_replacements() {
    let fixture = Repository::new();
    fixture.write("shared/nested/old.txt", b"old\n");
    fixture.command(&["add", "shared/nested/old.txt"]);
    stage_placeholder_symlink(&fixture, "pkg/link", "../shared");
    fixture.command(&["commit", "--quiet", "-m", "recursive directory target"]);
    fs::remove_dir_all(fixture.path().join("shared/nested")).unwrap();
    fixture.write("shared/nested", b"untracked replacement\n");

    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };

    _ = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap_err();
}

#[test]
#[cfg_attr(miri, ignore = "compares an unanchored nested directory include")]
fn unanchored_directory_includes_keep_nested_link_roots() {
    let fixture = Repository::new();
    fixture.write("pkg/tests/real/data.json", b"before\n");
    fixture.command(&["add", "pkg/tests/real/data.json"]);
    stage_placeholder_symlink(&fixture, "pkg/tests/fixtures", "real");
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fixture.write("pkg/tests/real/data.json", b"after\n");

    let rules =
        PackagingRules::new(Some(&["src/**".to_string(), "fixtures/".to_string()]), None).unwrap();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, _, _, _) = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();

    assert!(matches!(
        changed.as_slice(),
        [ChangedItem::Package { path, .. }] if path == "tests/fixtures/data.json"
    ));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "compares an unanchored directory include below an alias root"
)]
fn unanchored_directory_includes_can_match_only_below_the_alias() {
    let fixture = Repository::new();
    fixture.write("shared/fixtures/data.json", b"before\n");
    fixture.command(&["add", "shared/fixtures/data.json"]);
    stage_placeholder_symlink(&fixture, "pkg/alias", "../shared");
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fixture.write("shared/fixtures/data.json", b"after\n");

    let rules =
        PackagingRules::new(Some(&["src/**".to_string(), "fixtures/".to_string()]), None).unwrap();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, _, _, _) = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();

    assert!(matches!(
        changed.as_slice(),
        [ChangedItem::Package { path, .. }] if path == "alias/fixtures/data.json"
    ));
}

#[test]
#[cfg_attr(miri, ignore = "compares real Git symlink modes and work-tree files")]
fn changing_a_link_pointer_without_changing_its_payload_is_unchanged() {
    let fixture = Repository::new();
    fixture.write("shared/first", b"same\n");
    fixture.write("shared/second", b"same\n");
    fixture.command(&["add", "shared"]);
    stage_placeholder_symlink(&fixture, "pkg/link", "../shared/first");
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    stage_placeholder_symlink(&fixture, "pkg/link", "../shared/second");

    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let (changed, patch, stat, _) =
        diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap();
    assert!(changed.is_empty());
    assert!(patch.is_empty());
    assert_eq!((stat.files, stat.insertions, stat.deletions), (0, 0, 0));
}

#[test]
#[cfg_attr(miri, ignore = "compares real Git symlink modes")]
fn symlink_cycles_are_rejected_without_traversal() {
    let fixture = Repository::new();
    stage_placeholder_symlink(&fixture, "pkg/link", "../shared/alias");
    stage_placeholder_symlink(&fixture, "shared/alias", "../pkg/link");
    fixture.command(&["commit", "--quiet", "-m", "cycle"]);

    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let error = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap_err();
    assert!(error.to_string().contains("cycles"));
}

#[test]
#[cfg_attr(miri, ignore = "compares real Git symlink modes and filesystem state")]
fn symlink_targets_must_be_present_tracked_regular_files() {
    let fixture = Repository::new();
    stage_placeholder_symlink(&fixture, "pkg/link", "../shared/missing");
    fixture.command(&["commit", "--quiet", "-m", "invalid target"]);
    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    _ = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap_err();

    let fixture = Repository::new();
    fixture.write("shared/target", b"tracked\n");
    fixture.command(&["add", "shared/target"]);
    stage_placeholder_symlink(&fixture, "pkg/link", "../shared/target");
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fs::remove_file(fixture.path().join("shared/target")).unwrap();
    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    _ = diff_package(&fixture.repo(), "pkg", "HEAD", &side, &side).unwrap_err();
}

#[test]
#[cfg_attr(miri, ignore = "compares a real nested Git workspace")]
fn symlink_targets_cannot_escape_a_nested_workspace() {
    let fixture = Repository::new();
    fixture.write("outside", b"outside\n");
    stage_placeholder_symlink(&fixture, "workspace/pkg/link", "../../outside");
    fixture.command(&["add", "outside"]);
    fixture.command(&["commit", "--quiet", "-m", "escaping link"]);
    let git = GitRepo::discover(&fixture.path().join("workspace")).unwrap();
    let rules = PackagingRules::default();
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "workspace/pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let error = diff_package(&git, "pkg", "HEAD", &side, &side).unwrap_err();
    assert!(error.to_string().contains("outside the workspace"));
}

fn stage_placeholder_symlink(fixture: &Repository, path: &str, target: &str) {
    fixture.write(path, target.as_bytes());
    fixture.command(&["add", path]);
    let id = fixture.command(&["rev-parse", &format!(":{path}")]);
    fixture.command(&[
        "update-index",
        "--cacheinfo",
        &format!("120000,{},{}", id.trim(), path),
    ]);
    fixture.command(&["config", "core.symlinks", "false"]);
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
    let document = parse_document(
        Path::new("pkg/Cargo.toml"),
        "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\ninclude = [\"src/\"]\n",
    )
    .unwrap();
    let manifest =
        package_manifest_from_document(&document, "pkg/Cargo.toml", &WorkspaceInherit::default())
            .unwrap()
            .unwrap();
    let package = WorkPackage {
        public_origins: Vec::new(),
        manifest,
        manifest_path: fixture.path().join("pkg/Cargo.toml"),
        resources,
        dependencies: vec![],
        consumer_contract: true,
        proc_macro: false,
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
    fixture.write(".git/config", b"[invalid");
    work_tree_modes(&git, &side, &tracked_resources).unwrap_err();
    untracked_released(&git, &side, &tracked_resources, &tracked).unwrap_err();
    released_work_tree_paths(&git, &package, PathCase::Sensitive).unwrap_err();
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
#[cfg_attr(
    miri,
    ignore = "renders singleton blobs from real Git additions and deletions"
)]
fn package_diff_reads_singleton_additions_and_deletions() {
    let fixture = Repository::new();
    fixture.write("pkg/deleted", b"deleted\n");
    fixture.command(&["add", "pkg"]);
    fixture.command(&["commit", "--quiet", "-m", "anchor"]);
    fs::remove_file(fixture.path().join("pkg/deleted")).unwrap();
    let resources = BTreeMap::new();
    let rules = PackagingRules::default();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let git = fixture.repo();
    let (_, patch, stat, _) = diff_package(&git, "pkg", "HEAD", &side, &side).unwrap();
    assert_eq!((stat.files, stat.insertions, stat.deletions), (1, 0, 1));
    assert!(patch.contains("-deleted\n"));
    fixture.command(&["add", "-u"]);
    fixture.command(&["commit", "--quiet", "-m", "empty package"]);
    fixture.write("pkg/added", b"added\n");
    fixture.command(&["add", "pkg"]);
    let (_, patch, stat, _) = diff_package(&git, "pkg", "HEAD", &side, &side).unwrap();
    assert_eq!((stat.files, stat.insertions, stat.deletions), (1, 1, 0));
    assert!(patch.contains("+added\n"));
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
