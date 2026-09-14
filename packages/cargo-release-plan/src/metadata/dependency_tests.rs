//! Dependency identity, publication, and version-group decisions over small snapshots.

use tempfile::tempdir;
use toml_edit::{InlineTable, Table, value};

use super::*;
use crate::git::testing::Repository;
use crate::manifest::InstallationDependencies;

// Policy tests consume TOML's parsed model directly; parsing belongs to manifest
// tests and would dominate these small decision workloads under Miri.
fn document(entries: impl IntoIterator<Item = (&'static str, Item)>) -> DocumentMut {
    let mut document = DocumentMut::new();
    document.as_table_mut().extend(entries);
    document
}

fn table(entries: impl IntoIterator<Item = (&'static str, Item)>) -> Item {
    Item::Table(Table::from_iter(entries))
}

fn dependency(fields: &[(&str, &str)]) -> Item {
    let mut table = InlineTable::new();
    for (name, value) in fields {
        table.insert(*name, (*value).into());
    }
    Item::Value(table.into())
}

fn package(name: &str, root: &Path) -> MetadataPackage {
    MetadataPackage {
        name: name.to_string(),
        version: "1.2.3".to_string(),
        id: name.to_string(),
        manifest_path: root
            .join(name)
            .join("Cargo.toml")
            .to_string_lossy()
            .into_owned(),
        publish: None,
        dependencies: Vec::new(),
        targets: vec![MetadataTarget {
            name: name.to_string(),
            kind: vec!["lib".to_string()],
        }],
        metadata: Value::Null,
    }
}

#[test]
fn dependency_kinds_preserve_cargo_classification() {
    for (kind, expected) in [
        (None, DepKind::Normal),
        (Some("normal"), DepKind::Normal),
        (Some("build"), DepKind::Build),
        (Some("dev"), DepKind::Dev),
    ] {
        assert_eq!(DepKind::from_metadata(kind), expected);
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads real manifests and a Git index")]
fn workspace_projections_distinguish_version_targets_and_rewrite_members() {
    let fixture = Repository::new();
    fixture.write("Cargo.toml", b"[workspace]\n");
    for (name, publication) in [
        ("public", ""),
        ("helper", "publish = false\n"),
        ("loose", ""),
    ] {
        fixture.write(
            &format!("{name}/Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"1.2.3\"\n{publication}").as_bytes(),
        );
    }
    fixture.command(&[
        "add",
        "Cargo.toml",
        "public/Cargo.toml",
        "helper/Cargo.toml",
    ]);
    let mut helper = package("helper", fixture.path());
    helper.publish = Some(Vec::new());
    let metadata = MetadataJson {
        packages: vec![
            package("loose", fixture.path()),
            helper,
            package("public", fixture.path()),
            package("nonmember", fixture.path()),
        ],
        workspace_members: ["public", "helper", "loose"].map(str::to_string).to_vec(),
        workspace_root: fixture.path().to_string_lossy().into_owned(),
        metadata: Value::Null,
    };
    let git = fixture.repo();
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: fixture.path(),
        paths: git.ls_files("").unwrap(),
        case: PathCase::Sensitive,
    };

    let tree = work_tree_from_metadata(&metadata, &tracked).unwrap();

    assert_eq!(
        tree.version_targets
            .iter()
            .map(|target| (target.name.as_str(), target.publishable))
            .collect::<Vec<_>>(),
        [("helper", false), ("public", true)]
    );
    assert_eq!(
        tree.packages
            .iter()
            .map(|package| package.manifest.name.as_str())
            .collect::<Vec<_>>(),
        ["public"]
    );
    assert_eq!(
        tree.members_by_dir,
        ["helper", "loose", "public"]
            .map(|name| (fixture.path().join(name), name.to_string()))
            .into()
    );
    assert_eq!(
        tree.member_manifests,
        ["helper", "loose", "public"].map(|name| fixture.path().join(name).join("Cargo.toml"))
    );
}

#[test]
#[cfg_attr(miri, ignore = "loads filesystem manifests after metadata acquisition")]
fn either_publication_source_can_withhold_a_version_target_from_release() {
    let fixture = Repository::new();
    fixture.write("Cargo.toml", b"[workspace]\n");
    fixture.write(
        "member/Cargo.toml",
        b"[package]\nname = \"member\"\nversion = \"1.2.3\"\n",
    );
    fixture.command(&["add", "Cargo.toml", "member/Cargo.toml"]);
    let git = fixture.repo();
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: fixture.path(),
        paths: git.ls_files("").unwrap(),
        case: PathCase::Sensitive,
    };
    let mut metadata = MetadataJson {
        packages: vec![package("member", fixture.path())],
        workspace_members: vec!["member".to_string()],
        workspace_root: fixture.path().to_string_lossy().into_owned(),
        metadata: Value::Null,
    };
    // Metadata and manifests are acquired separately. Keep the conservative
    // intersection when publication changes between those acquisitions.
    for (metadata_publish, manifest_publish) in [(false, true), (true, false)] {
        metadata.packages.first_mut().unwrap().publish = (!metadata_publish).then(Vec::new);
        fixture.write(
            "member/Cargo.toml",
            format!(
                "[package]\nname = \"member\"\nversion = \"1.2.3\"\npublish = {manifest_publish}\n"
            )
            .as_bytes(),
        );
        let tree = work_tree_from_metadata(&metadata, &tracked).unwrap();
        assert_eq!(tree.version_targets.len(), 1);
        assert!(!tree.version_targets.first().unwrap().publishable);
        assert!(tree.packages.is_empty());
    }
}

fn exact_dependencies(member: DocumentMut, workspace: DocumentMut) -> Vec<ExactDependency> {
    let root = Path::new("workspace");
    let metadata = MetadataJson {
        packages: ["member", "helper", "unselected"]
            .map(|name| package(name, root))
            .into(),
        workspace_members: ["member", "helper", "unselected"]
            .map(str::to_string)
            .into(),
        workspace_root: root.to_string_lossy().into_owned(),
        metadata: Value::Null,
    };
    let selected = HashSet::from(["member", "helper"]);
    // Discovery consumes parsed declarations, not the derived package facts.
    // Snapshot acquisition and package projection have their own tests.
    let snapshot = ManifestSnapshot {
        documents: BTreeMap::from([
            (root.join("Cargo.toml"), workspace),
            (root.join("member/Cargo.toml"), member),
            (root.join("helper/Cargo.toml"), DocumentMut::new()),
        ]),
        packages: BTreeMap::new(),
    };
    let members = ["member", "helper"]
        .map(|name| (root.join(name), name.to_string()))
        .into();
    discover_exact_dependencies(
        &metadata,
        &selected,
        &members,
        &BTreeMap::new(),
        &snapshot,
        snapshot.root(root),
        root,
    )
    .unwrap()
}

#[test]
fn exact_dependency_discovery_follows_aliases_and_inherited_paths() {
    let mut alias = dependency(&[
        ("package", "helper"),
        ("path", "../helper"),
        ("version", "=1.2.2"),
    ]);
    alias
        .as_table_like_mut()
        .unwrap()
        .insert("optional", value(true));
    let found = exact_dependencies(
        document([
            ("dependencies", table([("alias", alias)])),
            (
                "build-dependencies",
                table([("inherited", table([("workspace", value(true))]))]),
            ),
        ]),
        document([(
            "workspace",
            table([(
                "dependencies",
                table([(
                    "inherited",
                    dependency(&[
                        ("package", "helper"),
                        ("path", "helper"),
                        ("version", "=1.2.3"),
                    ]),
                )]),
            )]),
        )]),
    );
    assert_eq!(
        found,
        [
            ("=1.2.2", "dependencies.alias"),
            (
                "=1.2.3",
                "build-dependencies.inherited -> workspace.dependencies.inherited"
            ),
        ]
        .map(|(requirement, location)| ExactDependency {
            source: "member".to_string(),
            target: "helper".to_string(),
            requirement: requirement.to_string(),
            manifest_path: PathBuf::from("workspace/member/Cargo.toml"),
            location: location.to_string(),
        })
    );
    let groups = Groups::from_edges(
        ["member", "helper"].map(str::to_string),
        found
            .into_iter()
            .map(|dependency| (dependency.source, dependency.target)),
    );
    assert_eq!(groups.members("helper"), ["helper", "member"]);
}

#[test]
fn exact_dependency_discovery_includes_target_specific_development_pins() {
    assert_eq!(
        exact_dependencies(
            document([(
                "target",
                table([(
                    "cfg(unix)",
                    table([(
                        "dev-dependencies",
                        table([(
                            "helper",
                            dependency(&[("path", "../helper"), ("version", "=1.2.3")]),
                        )])
                    )])
                )])
            )]),
            DocumentMut::new(),
        ),
        [ExactDependency {
            source: "member".to_string(),
            target: "helper".to_string(),
            requirement: "=1.2.3".to_string(),
            manifest_path: PathBuf::from("workspace/member/Cargo.toml"),
            location: "target.cfg(unix).dev-dependencies.helper".to_string(),
        }]
    );
}

#[test]
fn exact_dependency_discovery_ignores_nonexact_unmatched_and_unused_declarations() {
    assert!(
        exact_dependencies(
            document([(
                "dependencies",
                table([
                    (
                        "nonexact",
                        dependency(&[
                            ("package", "helper"),
                            ("path", "../helper"),
                            ("version", "^1.2.3"),
                        ])
                    ),
                    (
                        "wrong_name",
                        dependency(&[("path", "../helper"), ("version", "=1.2.3")])
                    ),
                    (
                        "versionless",
                        dependency(&[("package", "helper"), ("path", "../helper")])
                    ),
                    ("registry", value("=1.2.3")),
                ])
            )]),
            document([(
                "workspace",
                table([(
                    "dependencies",
                    table([(
                        "unused",
                        dependency(&[
                            ("package", "helper"),
                            ("path", "helper"),
                            ("version", "=1.2")
                        ]),
                    )])
                )])
            )]),
        )
        .is_empty()
    );
}

#[test]
fn dependency_requirements_preserve_string_and_table_forms() {
    for (item, expected) in [
        (value("=1.2.3"), Some("=1.2.3")),
        (dependency(&[("version", "^1.2.3")]), Some("^1.2.3")),
        (table([("version", value("*"))]), Some("*")),
        (dependency(&[("path", "member")]), None),
    ] {
        assert_eq!(dependency_requirement(&item), expected);
    }
}

#[test]
#[cfg_attr(miri, ignore = "canonicalizes real filesystem directories")]
fn canonical_member_index_retains_only_resolvable_member_identities() {
    let directory = tempdir().unwrap();
    for name in ["member", "outside", "via"] {
        fs::create_dir_all(directory.path().join(name)).unwrap();
    }
    let member = directory.path().join("member");
    let members = BTreeMap::from([
        (directory.path().join("via/../member"), "member".to_string()),
        (directory.path().join("missing"), "missing".to_string()),
    ]);
    let canonical = canonical_members_by_dir(&members);
    assert_eq!(
        canonical,
        BTreeMap::from([(fs::canonicalize(&member).unwrap(), "member".to_string())])
    );
    assert_eq!(
        resolved_member(directory.path(), "member", &members, &canonical),
        Some("member")
    );
    assert_eq!(
        resolved_member(directory.path(), "outside", &members, &canonical),
        None
    );
    assert_eq!(
        resolved_member(directory.path(), "absent", &members, &canonical),
        None
    );
}

#[test]
fn development_versions_ignore_other_kinds_and_retain_any_versioned_target() {
    let declared = MetadataDep {
        name: "helper".to_string(),
        req: "*".to_string(),
        rename: None,
        path: None,
        kind: Some("dev".to_string()),
        source: None,
    };
    let root = DocumentMut::new();
    assert!(!dev_dependency_declares_version(
        &declared,
        &document([
            ("dependencies", table([("helper", value("1"))])),
            ("build-dependencies", table([("helper", value("1"))])),
            (
                "dev-dependencies",
                table([("helper", dependency(&[("path", "../helper")]))])
            ),
        ]),
        &root,
    ));
    assert!(dev_dependency_declares_version(
        &declared,
        &document([
            (
                "dev-dependencies",
                table([(
                    "helper",
                    dependency(&[("path", "../helper"), ("version", "*")]),
                )])
            ),
            (
                "target",
                table([(
                    "cfg(unix)",
                    table([(
                        "dev-dependencies",
                        table([("helper", dependency(&[("path", "../helper")]),)])
                    )])
                )])
            ),
        ]),
        &root,
    ));
}

fn work_package(name: &str, dependencies: Vec<ReportedDep>) -> WorkPackage {
    WorkPackage {
        manifest: PackageManifest {
            name: name.to_string(),
            version: Version::new(1, 2, 3),
            directory: name.to_string(),
            packaging: PackagingRules::default(),
            inherited: InheritedKeys::default(),
            publish: true,
            path_dependencies: Vec::new(),
            inherited_path_dependencies: Vec::new(),
            installation_dependencies: InstallationDependencies::default(),
            resource_paths: Vec::new(),
            inherited_resource_paths: Vec::new(),
            auto_readme: false,
            targets: TargetDiscovery::default(),
        },
        manifest_path: PathBuf::from(name).join("Cargo.toml"),
        dependencies,
        consumer_contract: true,
        has_lockfile_target: false,
        resources: BTreeMap::new(),
    }
}

fn edge(name: &str, kind: DepKind) -> ReportedDep {
    ReportedDep {
        name: name.to_string(),
        req: "=1.2.3".to_string(),
        exact_pin: true,
        kind,
        public: false,
    }
}

#[test]
fn exposure_reaches_a_fixed_point_through_private_normal_intermediaries() {
    assert_exposure_chain(DepKind::Normal);
}

#[test]
fn build_dependencies_do_not_relay_public_exposure() {
    assert_exposure_chain(DepKind::Build);
}

#[test]
fn development_dependencies_do_not_relay_public_exposure() {
    assert_exposure_chain(DepKind::Dev);
}

fn assert_exposure_chain(relay_kind: DepKind) {
    // Name order makes the outer package visit the intermediary before its
    // exposure widens. A later pass must revisit the outer package.
    let mut packages = [
        work_package("a_outer", vec![edge("b_private", DepKind::Normal)]),
        work_package("b_private", vec![edge("c_facade", relay_kind)]),
        work_package("c_facade", vec![edge("d_impl", DepKind::Normal)]),
        work_package("d_impl", Vec::new()),
    ];
    // Consumer-contract privacy does not cut the conservative exposure
    // chain: public types may still pass through implementation partitions.
    packages[1].consumer_contract = false;
    let exposed = [
        ("a_outer", "d_impl"),
        ("b_private", "c_facade"),
        ("c_facade", "d_impl"),
    ]
    .map(|(package, exposed)| (package.to_string(), vec![exposed.to_string()]))
    .into();
    let libraries = ["a_outer", "b_private", "c_facade", "d_impl"]
        .map(|name| (name, name.to_string()))
        .into();

    mark_public_dependencies(&mut packages, &exposed, &libraries);

    assert_eq!(
        packages[0].dependencies.first().unwrap().public,
        relay_kind == DepKind::Normal
    );
    assert_eq!(
        packages[1].dependencies.first().unwrap().public,
        relay_kind == DepKind::Normal
    );
    assert!(packages[2].dependencies.first().unwrap().public);
}
