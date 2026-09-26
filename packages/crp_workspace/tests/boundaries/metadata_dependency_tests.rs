//! External acquisition for metadata.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crp_workspace::manifest::PathCase;
use crp_workspace::metadata::*;
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::git_fixture::Repository;

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
    fixture.write(
        "public/Cargo.toml",
        b"[package]\nname = 'public'\nversion = '1.2.3'\n\
          [dependencies]\nhelper = { path = '../helper', version = '=1.2.3' }\n",
    );
    fixture.command(&[
        "add",
        "Cargo.toml",
        "public/Cargo.toml",
        "helper/Cargo.toml",
    ]);
    let mut helper = package("helper", fixture.path());
    helper.publish = Some(Vec::new());
    let mut public = package("public", fixture.path());
    public.dependencies.push(MetadataDep {
        name: "helper".to_string(),
        req: "=1.2.3".to_string(),
        rename: None,
        path: Some(fixture.path().join("helper").to_string_lossy().into_owned()),
        kind: None,
        source: None,
    });
    let metadata = MetadataJson {
        packages: vec![
            package("loose", fixture.path()),
            helper,
            public,
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
        tree.exact_dependencies
            .iter()
            .map(|edge| (
                edge.source.as_str(),
                edge.target.as_str(),
                edge.requirement.as_str()
            ))
            .collect::<Vec<_>>(),
        [("public", "helper", "=1.2.3")]
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
#[cfg_attr(
    miri,
    ignore = "Resolves member path aliases against real directories and manifests"
)]
fn released_dependency_edges_resolve_member_path_aliases() {
    let fixture = Repository::new();
    fixture.write("Cargo.toml", b"[workspace]\n");
    fixture.write(
        "consumer/Cargo.toml",
        b"[package]\nname='consumer'\nversion='1.2.3'\n\
          [dependencies]\nmember={path='../member',version='1.2.3'}\n\
          [package.metadata.cargo_check_external_types]\nallowed_external_types=['member::*']\n",
    );
    fixture.write(
        "member/Cargo.toml",
        b"[package]\nname='member'\nversion='1.2.3'\n",
    );
    fixture.command(&["add", "."]);
    fs::create_dir_all(fixture.path().join("via")).unwrap();
    let mut consumer = package("consumer", fixture.path());
    consumer.metadata = json!({
        "cargo_check_external_types": {"allowed_external_types": ["member::*"]}
    });
    consumer.dependencies.push(MetadataDep {
        source: None,
        name: "member".to_owned(),
        req: "1.2.3".to_owned(),
        rename: None,
        path: Some(
            fixture
                .path()
                .join("via/../member")
                .to_string_lossy()
                .into_owned(),
        ),
        kind: None,
    });
    let metadata = MetadataJson {
        packages: vec![consumer, package("member", fixture.path())],
        workspace_members: vec!["consumer".to_owned(), "member".to_owned()],
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
    let consumer = tree
        .packages
        .iter()
        .find(|package| package.manifest.name == "consumer")
        .unwrap();
    assert_eq!(consumer.dependencies.len(), 1);
    let dependency = consumer.dependencies.first().unwrap();
    assert_eq!(dependency.name, "member");
    assert!(dependency.public);
}
