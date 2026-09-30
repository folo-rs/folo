//! Tests of tracked metadata eligibility without invoking Cargo discovery.

use super::*;
use crate::git::testing::unopened;
use crate::manifest::{DependencySource, InstallationDependencies, parse_package_manifest};

#[test]
fn only_tracked_manifests_are_workspace_members() {
    let root = Path::new("workspace");
    let git = unopened(root);
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: root,
        paths: vec!["pkg/Cargo.toml".to_string()],
        case: PathCase::Sensitive,
    };
    assert!(tracked.contains_manifest(&root.join("pkg/Cargo.toml").to_string_lossy()));
    assert!(!tracked.contains_manifest(&root.join("untracked/Cargo.toml").to_string_lossy()));
    assert!(!tracked.contains_manifest("outside/Cargo.toml"));
}

#[test]
fn lockfile_relevance_uses_present_regular_binary_sources_not_examples() {
    let root = Path::new("workspace");
    let git = unopened(root);
    let manifest = parse_package_manifest(
        "[package]\nname='pkg'\nversion='1.0.0'\n",
        "pkg/Cargo.toml",
        &WorkspaceInherit::default(),
    )
    .unwrap()
    .unwrap();
    for path in [
        "pkg/src/main.rs",
        "pkg/examples/main.rs",
        "other/src/main.rs",
    ] {
        for regular in [false, true] {
            let tracked = TrackedMetadata {
                git: &git,
                workspace_root: root,
                paths: vec![path.to_owned()],
                case: PathCase::Sensitive,
            };
            assert_eq!(
                tracked
                    .has_lockfile_target_with(&manifest, |actual| {
                        assert_eq!(actual, root.join(path));
                        Ok(regular)
                    })
                    .unwrap(),
                regular && path == "pkg/src/main.rs"
            );
        }
    }
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: root,
        paths: vec!["pkg/src/main.rs".into()],
        case: PathCase::Sensitive,
    };
    assert!(
        !tracked
            .has_lockfile_target_with(&manifest, |_| Err(io::ErrorKind::NotFound.into()))
            .unwrap()
    );
    assert!(
        tracked
            .has_lockfile_target_with(&manifest, |_| Err(io::ErrorKind::PermissionDenied.into()))
            .unwrap_err()
            .find_source::<ReadFileError>()
            .is_some()
    );
}

#[test]
fn manifest_spelling_uses_observed_case_rules() {
    let path = Path::new("member").join("cargo.toml");
    assert_eq!(
        cargo_manifest_path_with(&path, |_| PathCase::Sensitive),
        path
    );
    assert_eq!(
        cargo_manifest_path_with(&path, |_| PathCase::Insensitive),
        Path::new("member").join("Cargo.toml")
    );
    let other = Path::new("member").join("different.toml");
    assert_eq!(
        cargo_manifest_path_with(&other, |_| PathCase::Insensitive),
        other
    );
}

#[test]
fn acquired_member_index_preserves_actual_directory_identity_and_errors() {
    let members = BTreeMap::from([(PathBuf::from("alias"), "pkg".into())]);
    assert_eq!(
        canonical_members_by_dir_with(&members, |path| {
            assert_eq!(path, Path::new("alias"));
            Ok(PathBuf::from("actual"))
        })
        .unwrap(),
        BTreeMap::from([(PathBuf::from("actual"), "pkg".into())])
    );
    assert!(
        canonical_members_by_dir_with(&members, |_| Err(io::ErrorKind::PermissionDenied.into()))
            .unwrap_err()
            .find_source::<MemberIdentityUnavailable>()
            .is_some()
    );
}

#[test]
fn registry_reads_preserve_ancestor_and_filename_precedence() {
    let root = Path::new("repository");
    let git = GitRepo {
        root: root.into(),
        prefix: "nested".into(),
    };
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: root,
        paths: vec![],
        case: PathCase::Sensitive,
    };
    let mut reads = Vec::new();
    let indices = work_tree_registry_indices_with(&tracked, |path| {
        reads.push(path.to_path_buf());
        if path == root.join(".cargo/config") {
            Ok("[registries.outer]\nindex='outer'\n[registries.shared]\nindex='outer'\n".into())
        } else if path == root.join("nested/.cargo/config.toml") {
            Ok("[registries.shared]\nindex='inner'\n".into())
        } else {
            Err(io::ErrorKind::NotFound.into())
        }
    })
    .unwrap();
    assert_eq!(
        indices,
        BTreeMap::from([
            ("outer".into(), "outer".into()),
            ("shared".into(), "inner".into())
        ])
    );
    assert_eq!(
        reads,
        [
            root.join(".cargo/config"),
            root.join("nested/.cargo/config"),
            root.join("nested/.cargo/config.toml")
        ]
    );
    assert!(
        work_tree_registry_indices_with(&tracked, |_| Err(io::ErrorKind::PermissionDenied.into()))
            .unwrap_err()
            .find_source::<ReadFileError>()
            .is_some()
    );
}

#[test]
fn metadata_projection_reports_only_name_and_directory_matched_dependencies() {
    let root = Path::new("workspace");
    let git = unopened(root);
    let mut packages = ["dependent", "helper"].map(|name| MetadataPackage {
        name: name.into(),
        version: "1.0.0".into(),
        id: name.into(),
        manifest_path: root
            .join(name)
            .join("Cargo.toml")
            .to_string_lossy()
            .into_owned(),
        publish: None,
        dependencies: vec![],
        targets: vec![],
        metadata: Value::Null,
    });
    packages.first_mut().unwrap().dependencies = [
        ("helper", Some("alias")),
        ("unrelated", Some("alias")),
        ("helper", Some("dependent")),
        ("external", None),
    ]
    .map(|(name, path)| MetadataDep {
        name: name.into(),
        req: "=1.0.0".into(),
        rename: None,
        path: path.map(str::to_owned),
        kind: None,
        source: None,
    })
    .into();
    let metadata = MetadataJson {
        packages: packages.into(),
        workspace_members: vec!["dependent".into(), "helper".into()],
        workspace_root: root.to_string_lossy().into_owned(),
        metadata: Value::Null,
    };
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: root,
        paths: vec!["dependent/Cargo.toml".into(), "helper/Cargo.toml".into()],
        case: PathCase::Sensitive,
    };
    let mut aliases = 0;
    let work = work_tree_from_metadata_with(&metadata, &tracked, |path| {
        if path == root.join("Cargo.toml") { Ok("[workspace]\n".into()) }
        else if path == root.join("dependent/Cargo.toml") {
            Ok("[package]\nname='dependent'\nversion='1.0.0'\n[dependencies]\nhelper={path='../alias',version='=1.0.0'}\n".into())
        } else if path == root.join("helper/Cargo.toml") {
            Ok("[package]\nname='helper'\nversion='1.0.0'\n".into())
        } else { Err(io::ErrorKind::NotFound.into()) }
    }, |path| {
        if path == root.join("alias") {
            aliases += 1;
            Ok(root.join("helper"))
        } else {
            Ok(path.to_path_buf())
        }
    }, |_| Ok(true)).unwrap();
    assert_eq!(aliases, 3);
    assert_eq!(work.exact_dependencies.len(), 1);
    assert_eq!(work.exact_dependencies.first().unwrap().target, "helper");
    let dependent = work
        .packages
        .iter()
        .find(|package| package.manifest.name == "dependent")
        .unwrap();
    assert_eq!(
        dependent
            .dependencies
            .iter()
            .map(|dependency| dependency.name.as_str())
            .collect::<Vec<_>>(),
        ["helper"]
    );
}

#[test]
fn metadata_projection_requires_tracked_members_and_agreement_on_publication() {
    let root = Path::new("workspace");
    let git = unopened(root);
    for cargo_publish in [false, true] {
        for raw_publish in [false, true] {
            let packages = ["pkg", "untracked", "nonmember"]
                .map(|name| MetadataPackage {
                    name: name.into(),
                    version: "1.0.0".into(),
                    id: name.into(),
                    manifest_path: root
                        .join(name)
                        .join("Cargo.toml")
                        .to_string_lossy()
                        .into_owned(),
                    publish: (!cargo_publish).then(Vec::new),
                    dependencies: vec![],
                    targets: vec![],
                    metadata: Value::Null,
                })
                .into();
            let metadata = MetadataJson {
                packages,
                workspace_members: vec!["pkg".into(), "untracked".into()],
                workspace_root: root.to_string_lossy().into_owned(),
                metadata: Value::Null,
            };
            let tracked = TrackedMetadata {
                git: &git,
                workspace_root: root,
                paths: vec!["pkg/Cargo.toml".into(), "nonmember/Cargo.toml".into()],
                case: PathCase::Sensitive,
            };
            let work = work_tree_from_metadata_with(
                &metadata,
                &tracked,
                |path| {
                    if path == root.join("Cargo.toml") {
                        Ok("[workspace]\n".into())
                    } else if path == root.join("pkg/Cargo.toml") {
                        Ok(format!(
                            "[package]\nname='pkg'\nversion='1.0.0'\npublish={raw_publish}\n"
                        ))
                    } else {
                        assert!(path.to_string_lossy().contains(".cargo"));
                        Err(io::ErrorKind::NotFound.into())
                    }
                },
                |path| Ok(path.to_path_buf()),
                |_| Ok(true),
            )
            .unwrap();
            assert_eq!(work.version_targets.len(), 1);
            assert_eq!(work.version_targets.first().unwrap().name, "pkg");
            assert_eq!(
                work.version_targets.first().unwrap().publishable,
                cargo_publish && raw_publish
            );
            assert_eq!(
                work.packages.len(),
                usize::from(cargo_publish && raw_publish)
            );
            assert_eq!(
                work.member_manifests,
                [
                    root.join("pkg/Cargo.toml"),
                    root.join("untracked/Cargo.toml")
                ]
            );
        }
    }
}

#[test]
fn installation_acquisition_uses_tracked_spelling_and_distinguishes_missing_from_read_failure() {
    let root = Path::new("repository");
    let git = unopened(root);
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: root,
        paths: vec!["Helper/Cargo.toml".into()],
        case: PathCase::Insensitive,
    };
    let manifests = ManifestSnapshot {
        documents: BTreeMap::new(),
        packages: BTreeMap::new(),
    };
    let package = parse_package_manifest(
        "[package]\nname='tool'\nversion='1.0.0'\n[dependencies]\nhelper={path='../helper'}\n",
        "tool/Cargo.toml",
        &WorkspaceInherit::default(),
    )
    .unwrap()
    .unwrap();
    for failure in [
        None,
        Some(io::ErrorKind::NotFound),
        Some(io::ErrorKind::PermissionDenied),
    ] {
        let mut graph = InstallationGraph::default();
        graph.insert(
            "tool".into(),
            Version::new(1, 0, 0),
            package.installation_dependencies.clone(),
        );
        let mut reads = Vec::new();
        resolve_installation_paths_with(&mut graph, &manifests, &tracked, |path| {
            reads.push(path.to_path_buf());
            assert_eq!(path, root.join("Helper/Cargo.toml"));
            match failure {
                Some(kind) => Err(kind.into()),
                None => Ok("[package]\nname='helper'\nversion='2.0.0'\n".into()),
            }
        });
        assert_eq!(reads.len(), 1);
        let InstallationDependencies::Parsed(dependencies) = &graph.members.get("tool").unwrap().1
        else {
            panic!("valid declarations")
        };
        assert_eq!(
            matches!(
                dependencies.first().unwrap().source,
                DependencySource::Path(_)
            ),
            failure.is_none()
        );
        if let DependencySource::Path(identity) = &dependencies.first().unwrap().source {
            assert_eq!(identity.name, "helper");
            assert_eq!(identity.version, Version::new(2, 0, 0));
        }
        assert_eq!(
            !graph.path_errors.is_empty(),
            failure == Some(io::ErrorKind::PermissionDenied)
        );
    }
}
