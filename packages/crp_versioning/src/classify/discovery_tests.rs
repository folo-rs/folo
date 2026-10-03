//! Released-content acquisition tests without full Cargo workspace classification.

use crp_workspace::git::testing::tree_entry;

use super::*;

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
    let entries = HistoricalTree::new(vec![
        tree_entry("pkg/link", "120000"),
        tree_entry("pkg/plain", tree_mode(false)),
    ]);
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

#[test]
fn file_observations_preserve_presence_and_propagate_operational_failures() {
    let root = Path::new("repository");
    let released = HashMap::from([("file".to_owned(), "pkg/file".to_owned())]);
    let paths = vec!["pkg/file".to_owned()];
    for missing in [false, true] {
        let files = validated_work_tree_files_with(
            root,
            "pkg",
            &released,
            &WorkTreeModes::default(),
            |_| {
                if missing {
                    Err(io::ErrorKind::NotFound.into())
                } else {
                    Ok(false)
                }
            },
        )
        .unwrap();
        assert_eq!(
            files,
            if missing {
                vec![]
            } else {
                vec![("file", "pkg/file")]
            }
        );
        let present = present_in_work_tree_with(root, &paths, |path| {
            assert_eq!(path, root.join("pkg/file"));
            if missing {
                Err(io::ErrorKind::NotFound.into())
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(present, if missing { vec![] } else { paths.clone() });
    }
    let mut modes = WorkTreeModes::default();
    modes.set("pkg/file", "120000");
    assert!(
        validated_work_tree_files_with(root, "pkg", &released, &modes, |_| panic!("index link"))
            .unwrap_err()
            .find_source::<SymlinkReleasedError>()
            .is_some()
    );
    assert!(
        validated_work_tree_files_with(root, "pkg", &released, &WorkTreeModes::default(), |_| Err(
            io::ErrorKind::PermissionDenied.into()
        ))
        .unwrap_err()
        .find_source::<ReadFileError>()
        .is_some()
    );
    assert!(
        present_in_work_tree_with(
            root,
            &paths,
            |_| Err(io::ErrorKind::PermissionDenied.into())
        )
        .unwrap_err()
        .find_source::<ReadFileError>()
        .is_some()
    );
}

#[test]
fn acquired_hashes_keep_archive_names_and_source_order() {
    let files = vec![
        ("LICENSE", "shared/LICENSE"),
        ("src/lib.rs", "pkg/src/lib.rs"),
    ];
    assert_eq!(
        work_blob_ids_with(files.clone(), |paths| {
            assert_eq!(paths, ["shared/LICENSE", "pkg/src/lib.rs"]);
            Ok(vec!["license-id".into(), "source-id".into()])
        })
        .unwrap(),
        HashMap::from([
            ("LICENSE".into(), "license-id".into()),
            ("src/lib.rs".into(), "source-id".into())
        ])
    );
    work_blob_ids_with(files, |_| Err(io::Error::other("hashing failed").into())).unwrap_err();
}

#[test]
fn acquired_diff_absent_at_both_ends() {
    check_acquired_diff(false, false);
}

#[test]
fn acquired_diff_added() {
    check_acquired_diff(false, true);
}

#[test]
fn acquired_diff_deleted() {
    check_acquired_diff(true, false);
}

#[test]
fn acquired_diff_present_at_both_ends() {
    check_acquired_diff(true, true);
}

fn check_acquired_diff(old_present: bool, new_present: bool) {
    for old_executable in [false, true] {
        for new_executable in [false, true] {
            for same_content in [false, true] {
                let mut entry = tree_entry("old/file", tree_mode(old_executable));
                entry.id = "old-id".into();
                let tree = HistoricalTree::new(if old_present { vec![entry] } else { vec![] });
                let anchor = if old_present {
                    HashMap::from([("file".into(), "old/file".into())])
                } else {
                    HashMap::new()
                };
                // A tracked work-tree path remains selected even when deleted on disk.
                let work = HashMap::from([("file".into(), "new/file".into())]);
                let new_id = if same_content { "old-id" } else { "new-id" };
                let ids = if new_present {
                    HashMap::from([("file".into(), new_id.into())])
                } else {
                    HashMap::new()
                };
                let mut modes = WorkTreeModes::default();
                modes.set("new/file", tree_mode(new_executable));
                let content_changed = old_present != new_present || (old_present && !same_content);
                let mode_changed = old_present && new_present && old_executable != new_executable;
                let (changes, patch, stat) = PackageDiff {
                    anchor_files: &anchor,
                    work_files: &work,
                    anchor_tree: &tree,
                    work_modes: &modes,
                    work_ids: &ids,
                }
                .identify()
                .render(|id| {
                    assert!(content_changed);
                    if old_present && id == "old-id" {
                        Ok(Rc::from(b"old\n".as_slice()))
                    } else {
                        assert!(new_present);
                        assert_eq!(id, new_id);
                        Ok(Rc::from(b"new\n".as_slice()))
                    }
                })
                .unwrap();
                let changed = content_changed || mode_changed;
                assert_eq!(changes.len(), usize::from(changed));
                assert_eq!(stat.files, usize::from(changed));
                assert_eq!(patch.contains("old mode "), mode_changed);
                if changed {
                    let kind = match (old_present, new_present) {
                        (false, true) => "added",
                        (true, false) => "deleted",
                        _ => "modified",
                    };
                    assert!(
                        matches!(changes.first().unwrap(), ChangedItem::Package { path, change } if path == "file" && change == kind)
                    );
                } else {
                    assert!(patch.is_empty());
                }
                assert_eq!(stat.insertions, usize::from(content_changed && new_present));
                assert_eq!(stat.deletions, usize::from(content_changed && old_present));
            }
        }
    }
}

#[test]
fn untracked_advice_observes_nested_boundaries_resources_and_readme_priority() {
    let rules = PackagingRules::new(Some(&["/src/".into()]), None).unwrap();
    let resources = BTreeMap::from([
        ("missing".into(), "shared/missing".into()),
        ("tracked".into(), "shared/tracked".into()),
        ("present".into(), "shared/present".into()),
    ]);
    let side = PackageSide {
        dir: "pkg",
        rules: &rules,
        resources: &resources,
        auto_readme: true,
        case: PathCase::Sensitive,
    };
    let tracked_resources = BTreeMap::from([("tracked".into(), "shared/tracked".into())]);
    let listed = [
        "pkg/src/lib.rs",
        "pkg/src/nested/Cargo.toml",
        "pkg/src/nested/file",
        "pkg/src/tracked-nested/file",
        "pkg/README.md",
        "pkg/excluded",
    ]
    .map(str::to_owned);
    let tracked = ["pkg/src/tracked-nested/Cargo.toml", "pkg/README.txt"].map(str::to_owned);
    assert_eq!(
        untracked_released_with(&side, &tracked_resources, &tracked, &listed, |path| path
            != "shared/missing"),
        ["README.md", "present", "src/lib.rs"]
    );
}

#[test]
fn root_manifest_addressing_keeps_the_selected_workspace_prefix() {
    for (prefix, expected) in [("", "Cargo.toml"), ("nested", "nested/Cargo.toml")] {
        let git = GitRepo {
            root: PathBuf::from("unused"),
            prefix: prefix.into(),
        };
        assert_eq!(root_manifest_rel(&git), expected);
    }
}

#[test]
fn timeline_acquisition_preserves_meaningful_states_and_queries_only_the_endpoint_parent() {
    let commits = ["newest", "middle", "oldest"].map(str::to_owned);
    for presence in [
        Presence::Absent,
        Presence::Unpublished,
        Presence::Published(Version::new(1, 0, 0)),
    ] {
        for boundary in [false, true] {
            let mut observed = Vec::new();
            let mut parent_queries = Vec::new();
            let timeline = build_timeline_with(
                &commits,
                |commit| {
                    observed.push(commit.to_owned());
                    Ok(presence.clone())
                },
                |commit| {
                    parent_queries.push(commit.to_owned());
                    Ok(boundary)
                },
            )
            .unwrap();
            assert_eq!(observed, commits);
            assert_eq!(parent_queries, ["oldest"]);
            assert_eq!(timeline.len(), commits.len());
            for (entry, commit) in timeline.iter().zip(&commits) {
                assert_eq!(entry.commit, *commit);
                assert_eq!(entry.presence, presence);
                assert_eq!(entry.has_parent, commit != "oldest" || boundary);
            }
        }
    }
    build_timeline_with(
        &commits,
        |_| Err(io::Error::other("snapshot").into()),
        |_| panic!("no snapshot"),
    )
    .unwrap_err();
    build_timeline_with(
        &commits[..1],
        |_| Ok(Presence::Absent),
        |_| Err(io::Error::other("parent").into()),
    )
    .unwrap_err();
}

#[test]
fn captured_manifests_cache_recorded_paths_and_bound_implicit_members_to_the_workspace() {
    let root = parse_document(
        Path::new("nested/Cargo.toml"),
        "[workspace]\nmembers=['Member']\n[workspace.dependencies]\nshared={path='Helper'}\n",
    )
    .unwrap();
    let paths: BTreeMap<String, String> = [
        ("", "nested/Cargo.toml"),
        ("..", "Cargo.toml"),
        ("../outside", "outside/Cargo.toml"),
        ("Helper", "nested/Helper/Cargo.toml"),
        ("Member", "nested/Member/Cargo.toml"),
    ]
    .map(|(key, path)| (key.into(), path.into()))
    .into();
    let mut reads = Vec::new();
    let mut source = GitManifestSource {
        read: |path: &str| {
            reads.push(path.to_owned());
            parse_document(Path::new(path), if path == "nested/Member/Cargo.toml" {
                            "[package]\nname='member'\nversion='1.0.0'\n[dependencies]\nlocal={path='../helper'}\nroot={path='..'}\nparent={path='../..'}\noutside={path='../../outside'}\nbeyond={path='../../../absent'}\nshared.workspace=true\n"
                        } else {
                            assert_eq!(path, "nested/Cargo.toml");
                            "[workspace]\n"
                        }).map(Some)
        },
        workspace_prefix: "nested/",
        workspace: WorkspaceInherit::from_root(&root),
        paths: paths.clone(),
        parsed: BTreeMap::new(),
        case: PathCase::Insensitive,
    };
    assert_eq!(
        source.candidate_dirs(),
        paths.keys().cloned().collect::<Vec<_>>()
    );
    assert_eq!(
        source
            .path_edges("member")
            .unwrap()
            .into_iter()
            .collect::<BTreeSet<_>>(),
        ["", "Helper"].map(str::to_owned).into()
    );
    assert_eq!(source.manifest("MEMBER").unwrap().unwrap().name, "member");
    assert!(source.manifest("absent").unwrap().is_none());
    assert!(source.path_edges("").unwrap().is_empty());
    assert!(source.path_edges("absent").unwrap().is_empty());
    assert_eq!(reads, ["nested/Member/Cargo.toml", "nested/Cargo.toml"]);
}

#[test]
fn acquired_snapshots_distinguish_published_unpublished_and_absent_packages() {
    let git = GitRepo {
        root: PathBuf::from("unused"),
        prefix: "nested".into(),
    };
    let files = BTreeMap::from([
        (
            "nested/Cargo.toml",
            "[workspace]\nmembers=['public','private']\n",
        ),
        (
            "nested/public/Cargo.toml",
            "[package]\nname='public'\nversion='1.2.3'\n",
        ),
        (
            "nested/private/Cargo.toml",
            "[package]\nname='private'\nversion='1.0.0'\npublish=false\n",
        ),
        (
            "nested/unselected/Cargo.toml",
            "malformed unrelated content",
        ),
        ("nested/public/src/main.rs", "fn main() {}"),
    ]);
    let tree = HistoricalTree::new(
        files
            .keys()
            .map(|path| tree_entry(path, tree_mode(false)))
            .collect(),
    );
    let snapshot = load_snapshot_with(
        &git,
        PathCase::Sensitive,
        &BTreeMap::new(),
        Rc::new(tree),
        |path| {
            assert_ne!(path, "nested/unselected/Cargo.toml");
            Ok(files.get(path).map(|text| (*text).to_owned()))
        },
    )
    .unwrap();
    assert_eq!(
        snapshot_presence(&snapshot, "public"),
        Presence::Published(Version::new(1, 2, 3))
    );
    assert_eq!(
        snapshot_presence(&snapshot, "private"),
        Presence::Unpublished
    );
    assert_eq!(snapshot_presence(&snapshot, "missing"), Presence::Absent);
    assert_eq!(snapshot.packages.len(), 1);
    assert!(snapshot.packages.get("public").unwrap().has_lockfile_target);
    assert_eq!(snapshot.unpublished, BTreeSet::from(["private".into()]));
    let absent = load_snapshot_with(
        &git,
        PathCase::Sensitive,
        &BTreeMap::new(),
        Rc::default(),
        |_| panic!("no tracked manifest"),
    )
    .unwrap();
    assert!(absent.packages.is_empty());
    assert!(absent.unpublished.is_empty());
}
