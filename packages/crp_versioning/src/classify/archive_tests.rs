//! Archive traversal over acquired trees and unfollowed work-tree observations.

use std::collections::VecDeque;

use super::*;

fn tree(entries: &[(&str, &str)]) -> HistoricalTree {
    HistoricalTree::new(
        entries
            .iter()
            .map(|(path, mode)| TreeEntry {
                path: (*path).into(),
                id: format!("blob:{path}"),
                mode: (*mode).into(),
            })
            .collect(),
    )
}

fn rules(include: &[&str]) -> PackagingRules {
    PackagingRules::new(
        Some(
            &include
                .iter()
                .map(|value| (*value).into())
                .collect::<Vec<_>>(),
        ),
        None,
    )
    .unwrap()
}

#[test]
fn historical_directory_aliases_preserve_archive_names_modes_and_package_boundaries() {
    let tree = tree(&[
        ("pkg/alias", "120000"),
        ("shared/a", "100644"),
        ("shared/bin", "100755"),
        ("shared/nested/Cargo.toml", "100644"),
        ("shared/nested/hidden", "100644"),
    ]);
    let mut links = HashMap::from([("blob:pkg/alias", b"../shared".to_vec())]);
    let identities = historical_identities_with(
        "pkg",
        &tree,
        &HashMap::from([("alias".into(), "pkg/alias".into())]),
        "",
        &PackagingRules::default(),
        &BTreeSet::new(),
        PathCase::Sensitive,
        &mut |id| Ok(links.remove(id).unwrap()),
    )
    .unwrap();
    assert_eq!(
        identities,
        HashMap::from([
            (
                "alias/a".into(),
                ReleasedFileIdentity {
                    id: "blob:shared/a".into(),
                    executable: false
                }
            ),
            (
                "alias/bin".into(),
                ReleasedFileIdentity {
                    id: "blob:shared/bin".into(),
                    executable: true
                }
            ),
        ])
    );
    assert!(links.is_empty());
}

#[test]
fn historical_selection_keeps_resources_and_prunes_unselected_broken_links() {
    let tree = tree(&[("pkg/broken", "120000"), ("shared/license", "100644")]);
    let identities = historical_identities_with(
        "pkg",
        &tree,
        &HashMap::from([
            ("ignored".into(), "pkg/broken".into()),
            ("LICENSE".into(), "shared/license".into()),
        ]),
        "",
        &rules(&["/src/"]),
        &BTreeSet::from(["LICENSE".into()]),
        PathCase::Sensitive,
        &mut |_| panic!("unselected link must not be read"),
    )
    .unwrap();
    assert_eq!(
        identities,
        HashMap::from([(
            "LICENSE".into(),
            ReleasedFileIdentity {
                id: "blob:shared/license".into(),
                executable: false
            }
        ),])
    );
}

#[test]
fn historical_link_cycles_missing_targets_and_nonregular_nodes_are_rejected() {
    for (entries, targets, condition) in [
        (vec![("pkg/link", "120000")], vec!["link"], "cycle"),
        (vec![("pkg/link", "120000")], vec!["absent"], "missing"),
        (vec![("pkg/link", "160000")], vec![], "nonregular"),
        (
            vec![("pkg/link", "120000"), ("pkg/dir/loop", "120000")],
            vec!["dir", "../dir"],
            "cycle",
        ),
    ] {
        let tree = tree(&entries);
        let mut targets: VecDeque<_> = targets.into_iter().collect();
        let error = historical_identities_with(
            "pkg",
            &tree,
            &HashMap::from([("link".into(), "pkg/link".into())]),
            "",
            &PackagingRules::default(),
            &BTreeSet::new(),
            PathCase::Sensitive,
            &mut |_| Ok(targets.pop_front().unwrap().as_bytes().to_vec()),
        )
        .unwrap_err();
        match condition {
            "cycle" => assert!(error.find_source::<SymlinkCycleError>().is_some()),
            "missing" => assert!(error.find_source::<MissingSymlinkTargetError>().is_some()),
            _ => assert!(
                error
                    .find_source::<NonRegularSymlinkTargetError>()
                    .is_some()
            ),
        }
        assert!(targets.is_empty());
    }
}

#[test]
fn historical_link_blob_errors_and_invalid_encoding_are_not_missing_content() {
    let tree = tree(&[("link", "120000")]);
    for result in [
        Err(io::Error::other("blob acquisition").into()),
        Ok(vec![0xff]),
    ] {
        let mut result = Some(result);
        let error = historical_identities_with(
            "pkg",
            &tree,
            &HashMap::from([("link".into(), "link".into())]),
            "",
            &PackagingRules::default(),
            &BTreeSet::new(),
            PathCase::Sensitive,
            &mut |id| {
                assert_eq!(id, "blob:link");
                result.take().unwrap()
            },
        )
        .unwrap_err();
        assert!(
            error.find_source::<io::Error>().is_some()
                || error.find_source::<SymlinkTargetEncodingError>().is_some()
        );
    }
}

#[test]
fn directory_and_package_identity_respect_recorded_spelling_and_component_boundaries() {
    let paths = ["Shared/data/file", "Shared/data/Cargo.toml"].map(str::to_owned);
    assert_eq!(
        recorded_directory(&paths, "shared/DATA", PathCase::Insensitive),
        Some("Shared/data".into())
    );
    assert_eq!(
        recorded_directory(&paths, "shared/DATA", PathCase::Sensitive),
        None
    );
    assert_eq!(
        recorded_directory(&paths, "Shared/dat", PathCase::Sensitive),
        None
    );
    assert!(is_package_directory(
        &paths,
        "shared/DATA",
        PathCase::Insensitive
    ));
    assert!(!is_package_directory(
        &paths,
        "shared/DATA",
        PathCase::Sensitive
    ));
    assert!(!is_package_directory(&paths, "Shared", PathCase::Sensitive));
    assert_eq!(
        archive_child(
            "alias",
            "Shared/data/file",
            "shared/DATA",
            PathCase::Insensitive
        ),
        "alias/file"
    );
    assert!(within_workspace("ws", "ws", PathCase::Sensitive));
    assert!(within_workspace("ws/file", "ws", PathCase::Sensitive));
    assert!(!within_workspace("wss/file", "ws", PathCase::Sensitive));
    assert!(within_workspace("anywhere", "", PathCase::Sensitive));
}

#[test]
fn subtree_admission_and_resource_exceptions_remain_independent() {
    let rules = rules(&["/src/"]);
    let exceptions = BTreeSet::from(["docs/LICENSE".into()]);
    for (path, selected) in [
        ("src", true),
        ("docs", true),
        ("docs/LICENSE", true),
        ("doc", false),
        ("docs/LICENSE/child", false),
        ("other", false),
    ] {
        assert_eq!(
            archive_subtree_selected(&rules, &exceptions, path),
            selected
        );
    }
    assert_eq!(
        packaging_rule_exceptions(
            &HashMap::from([
                ("src/lib.rs".into(), "pkg/src/lib.rs".into()),
                ("LICENSE".into(), "shared/LICENSE".into()),
            ]),
            &rules
        ),
        BTreeSet::from(["LICENSE".into()]),
    );
    for (path, selected) in [
        ("pkg/file", true),
        ("pkg2/file", false),
        ("shared/LICENSE", true),
        ("shared/other", false),
    ] {
        assert_eq!(
            package_or_resource_path(path, "pkg", &["shared/LICENSE"], PathCase::Sensitive),
            selected
        );
    }
}

#[test]
fn only_regular_git_modes_supply_terminal_file_identity() {
    for (mode, regular) in [
        ("100644", true),
        ("100755", true),
        ("120000", false),
        ("160000", false),
        ("040000", false),
    ] {
        assert_eq!(
            tree_entry_is_regular(&TreeEntry {
                path: "file".into(),
                id: "object".into(),
                mode: mode.into(),
            }),
            regular
        );
    }
}

#[test]
fn manifest_presence_queries_select_tracked_manifests_and_retain_the_response() {
    let tracked = ["pkg/Cargo.toml", "pkg/file", "nested/Cargo.toml"].map(str::to_owned);
    let present = present_tracked_manifests_with(&tracked, PathCase::Sensitive, |paths| {
        assert_eq!(paths, ["pkg/Cargo.toml", "nested/Cargo.toml"]);
        Ok(vec!["nested/Cargo.toml".into()])
    })
    .unwrap();
    assert_eq!(present, ["nested/Cargo.toml"]);
    let error = present_tracked_manifests_with(&tracked, PathCase::Sensitive, |_| {
        Err(io::Error::other("presence").into())
    })
    .unwrap_err();
    assert!(error.find_source::<io::Error>().is_some());
}

#[test]
fn link_roots_require_presence_kind_scope_and_selected_descendants() {
    let tracked = [
        "pkg/alias",
        "pkg/missing",
        "pkg/regular",
        "pkg/ignored",
        "pkg/nested/Cargo.toml",
        "pkg/nested/link",
        "other/alias",
    ]
    .map(str::to_owned);
    let present: Vec<_> = tracked
        .iter()
        .filter(|path| path.as_str() != "pkg/missing")
        .cloned()
        .collect();
    let rules = rules(&["alias/child", "missing/child", "regular/child", "/nested/"]);
    let resources = BTreeMap::new();
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: false,
        case: PathCase::Sensitive,
    };
    let mut released = HashMap::new();
    add_symlink_roots(&mut released, &tracked, &present, &side, |path| {
        path != "pkg/regular"
    });
    assert_eq!(
        released,
        HashMap::from([("alias".into(), "pkg/alias".into())])
    );
}

#[test]
fn complete_and_initial_mode_observations_have_independent_lifetimes() {
    for first in [false, true] {
        for initial in [false, true] {
            assert!(use_captured_modes(first, initial, true));
            assert_eq!(use_captured_modes(first, initial, false), first && initial);
        }
    }
}

#[test]
fn work_links_follow_native_and_placeholder_targets_and_keep_terminal_modes() {
    for native in [false, true] {
        let tracked = ["pkg/link", "shared/file"].map(str::to_owned);
        let mut expected = VecDeque::from([("pkg/link", true), ("shared/file", false)]);
        let result = resolve_work_node_with(
            "pkg",
            "pkg/link",
            &tracked,
            "",
            PathCase::Sensitive,
            false,
            |path, first| {
                assert_eq!(Some((path, first)), expected.pop_front());
                if first {
                    let target = if native {
                        WorkTreeSymlinkTarget::Native("../shared/file".into())
                    } else {
                        WorkTreeSymlinkTarget::Placeholder(b"../shared/file".to_vec())
                    };
                    Ok((WorkTreePath::Symlink(target), false))
                } else {
                    Ok((WorkTreePath::Regular, true))
                }
            },
        )
        .unwrap()
        .unwrap();
        let ResolvedWorkNode::File { file, link_inputs } = result else {
            panic!()
        };
        assert_eq!(
            file,
            ResolvedWorkFile {
                path: "shared/file".into(),
                executable: true
            }
        );
        assert_eq!(link_inputs, BTreeSet::from(["pkg/link".into()]));
        assert!(expected.is_empty());
    }
}

#[test]
fn work_missing_targets_and_required_directory_kinds_are_distinct() {
    let tracked = ["link", "target/child"].map(str::to_owned);
    for terminal in [
        WorkTreePath::Missing,
        WorkTreePath::Regular,
        WorkTreePath::Other,
    ] {
        let missing = matches!(terminal, WorkTreePath::Missing);
        let mut observations = VecDeque::from([
            WorkTreePath::Symlink(WorkTreeSymlinkTarget::Placeholder(b"target".to_vec())),
            terminal,
        ]);
        let error = resolve_work_node_with(
            "pkg",
            "link",
            &tracked,
            "",
            PathCase::Sensitive,
            false,
            |_, _| Ok((observations.pop_front().unwrap(), false)),
        )
        .err()
        .unwrap();
        if missing {
            assert!(error.find_source::<MissingSymlinkTargetError>().is_some());
        } else {
            assert!(
                error
                    .find_source::<NonRegularSymlinkTargetError>()
                    .is_some()
            );
        }
        assert!(observations.is_empty());
    }
    assert!(
        resolve_work_node_with(
            "pkg",
            "deleted",
            &[],
            "",
            PathCase::Sensitive,
            false,
            |_, _| Ok((WorkTreePath::Missing, false)),
        )
        .unwrap()
        .is_none()
    );
    let mut observations = VecDeque::from([
        WorkTreePath::Symlink(WorkTreeSymlinkTarget::Placeholder(b"link".to_vec())),
        WorkTreePath::Symlink(WorkTreeSymlinkTarget::Placeholder(b"link".to_vec())),
    ]);
    let error = resolve_work_node_with(
        "pkg",
        "link",
        &tracked,
        "",
        PathCase::Sensitive,
        false,
        |_, _| Ok((observations.pop_front().unwrap(), false)),
    )
    .err()
    .unwrap();
    assert!(error.find_source::<SymlinkCycleError>().is_some());
}

#[test]
fn work_directory_expansion_keeps_aliases_resources_and_capture_dependencies() {
    let tracked = [
        "shared/file",
        "shared/nested/Cargo.toml",
        "shared/nested/hidden",
    ]
    .map(str::to_owned);
    let mut links = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut files = BTreeMap::new();
    let mut expected = VecDeque::from([("pkg/alias", true, false), ("shared/file", false, false)]);
    expand_work_released_path(
        "pkg",
        "alias",
        "pkg/alias",
        &tracked,
        &tracked,
        &rules(&["/src/"]),
        &BTreeSet::from(["alias/file".into()]),
        PathCase::Sensitive,
        true,
        false,
        &mut BTreeSet::new(),
        &mut links,
        &mut directories,
        &mut files,
        &mut |path, initial, required| {
            assert_eq!(Some((path, initial, required)), expected.pop_front());
            if initial {
                Ok(Some(ResolvedWorkNode::Directory {
                    path: "shared".into(),
                    link_inputs: BTreeSet::from(["pkg/alias".into()]),
                }))
            } else {
                Ok(Some(ResolvedWorkNode::File {
                    file: ResolvedWorkFile {
                        path: path.into(),
                        executable: false,
                    },
                    link_inputs: BTreeSet::new(),
                }))
            }
        },
    )
    .unwrap();
    assert_eq!(
        files,
        BTreeMap::from([(
            "alias/file".into(),
            ResolvedWorkFile {
                path: "shared/file".into(),
                executable: false
            }
        )])
    );
    assert_eq!(links, BTreeSet::from(["pkg/alias".into()]));
    assert_eq!(directories, BTreeSet::from(["shared".into()]));
    assert!(expected.is_empty());
}

#[test]
fn work_expansion_prunes_excluded_nodes_and_rejects_directory_cycles() {
    let tracked = ["shared/link"].map(str::to_owned);
    let mut calls = VecDeque::from(["alias", "shared/link"]);
    for excluded in [true, false] {
        let result = expand_work_released_path(
            "pkg",
            "alias",
            "alias",
            &tracked,
            &[],
            &rules(if excluded { &["/src/"] } else { &["/alias/"] }),
            &BTreeSet::new(),
            PathCase::Sensitive,
            true,
            false,
            &mut BTreeSet::new(),
            &mut BTreeSet::new(),
            &mut BTreeSet::new(),
            &mut BTreeMap::new(),
            &mut |path, _, _| {
                assert_eq!(Some(path), calls.pop_front());
                Ok(Some(ResolvedWorkNode::Directory {
                    path: "shared".into(),
                    link_inputs: BTreeSet::new(),
                }))
            },
        );
        if excluded {
            result.unwrap();
        } else {
            assert!(
                result
                    .unwrap_err()
                    .find_source::<SymlinkCycleError>()
                    .is_some()
            );
        }
    }
    assert!(calls.is_empty());
}

#[test]
fn work_directory_resolution_feeds_recursive_archive_expansion() {
    let tracked = ["pkg/alias", "Shared/Data/nested/file"].map(str::to_owned);
    let mut nodes = VecDeque::from([
        (
            "pkg/alias",
            true,
            WorkTreePath::Symlink(WorkTreeSymlinkTarget::Placeholder(
                b"../shared/data".to_vec(),
            )),
        ),
        ("Shared/Data", false, WorkTreePath::Directory),
        ("Shared/Data/nested", true, WorkTreePath::Directory),
        ("Shared/Data/nested/file", true, WorkTreePath::Regular),
    ]);
    let mut calls = VecDeque::from([
        ("pkg/alias", true, false),
        ("Shared/Data/nested", false, true),
        ("Shared/Data/nested/file", false, false),
    ]);
    let mut links = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut files = BTreeMap::new();
    expand_work_released_path(
        "pkg",
        "alias",
        "pkg/alias",
        &tracked,
        &[],
        &PackagingRules::default(),
        &BTreeSet::new(),
        PathCase::Insensitive,
        true,
        false,
        &mut BTreeSet::new(),
        &mut links,
        &mut directories,
        &mut files,
        &mut |path, initial, required| {
            assert_eq!(Some((path, initial, required)), calls.pop_front());
            resolve_work_node_with(
                "pkg",
                path,
                &tracked,
                "",
                PathCase::Insensitive,
                required,
                |path, first| {
                    let (expected, expected_first, kind) = nodes.pop_front().unwrap();
                    assert_eq!(path, expected);
                    assert_eq!(first, expected_first);
                    Ok((kind, false))
                },
            )
        },
    )
    .unwrap();
    assert!(calls.is_empty());
    assert!(nodes.is_empty());
    assert_eq!(
        files,
        BTreeMap::from([(
            "alias/nested/file".into(),
            ResolvedWorkFile {
                path: "Shared/Data/nested/file".into(),
                executable: false
            },
        )])
    );
    assert_eq!(links, BTreeSet::from(["pkg/alias".into()]));
    assert_eq!(
        directories,
        BTreeSet::from(["Shared/Data".into(), "Shared/Data/nested".into()])
    );
}
