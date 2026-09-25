//! Dependency identity, publication, and version-group decisions over small snapshots.

use toml_edit::{InlineTable, Table, value};

use super::*;
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
fn exact_dependency_discovery_follows_optional_aliases_and_forms_groups() {
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
        document([("dependencies", table([("alias", alias)]))]),
        DocumentMut::new(),
    );
    assert_eq!(
        found,
        [ExactDependency {
            source: "member".to_string(),
            target: "helper".to_string(),
            requirement: "=1.2.2".to_string(),
            manifest_path: PathBuf::from("workspace/member/Cargo.toml"),
            location: "dependencies.alias".to_string(),
        }]
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
fn exact_dependency_discovery_resolves_inherited_build_paths_at_the_workspace_root() {
    let found = exact_dependencies(
        document([(
            "build-dependencies",
            table([("inherited", table([("workspace", value(true))]))]),
        )]),
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
        [ExactDependency {
            source: "member".to_string(),
            target: "helper".to_string(),
            requirement: "=1.2.3".to_string(),
            manifest_path: PathBuf::from("workspace/member/Cargo.toml"),
            location: "build-dependencies.inherited -> workspace.dependencies.inherited"
                .to_string(),
        }]
    );
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
    // exposure widens. Compact identifiers avoid irrelevant hashing work in Miri.
    const OUTER: &str = "a";
    const PRIVATE: &str = "b";
    const FACADE: &str = "c";
    const IMPLEMENTATION: &str = "d";
    let mut packages = [
        work_package(OUTER, vec![edge(PRIVATE, DepKind::Normal)]),
        work_package(PRIVATE, vec![edge(FACADE, relay_kind)]),
        work_package(FACADE, vec![edge(IMPLEMENTATION, DepKind::Normal)]),
        work_package(IMPLEMENTATION, Vec::new()),
    ];
    // Consumer-contract privacy does not cut the conservative exposure
    // chain: public types may still pass through implementation partitions.
    packages[1].consumer_contract = false;
    let exposed = [
        (OUTER, IMPLEMENTATION),
        (PRIVATE, FACADE),
        (FACADE, IMPLEMENTATION),
    ]
    .map(|(package, exposed)| (package.to_string(), vec![exposed.to_string()]))
    .into();
    let libraries = [OUTER, PRIVATE, FACADE, IMPLEMENTATION]
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
