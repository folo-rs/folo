//! External acquisition for metadata.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crp_workspace::git::GitRepo;
use crp_workspace::lockfile::{InstallationGraph, Lockfile};
use crp_workspace::manifest::{
    DependencySource, PackageIdentity, PathCase, WorkspaceInherit, installation_patches,
    parse_document, parse_package_manifest,
};
use crp_workspace::metadata::*;
use semver::Version;

use crate::git_fixture::Repository;

#[test]
#[cfg_attr(miri, ignore = "uses a temporary repository and real filesystem")]
fn registry_configuration_uses_ancestor_and_filename_precedence() {
    let fixture = Repository::new();
    fixture.write(
        ".cargo/config.toml",
        b"[registries]\nshared.index = 'https://example.invalid/root'\n\
          root_only.index = 'https://example.invalid/root-only'\n",
    );
    fixture.write(
        "nested/.cargo/config",
        b"[registries]\nshared.index = 'https://example.invalid/parent'\n\
          parent_only.index = 'https://example.invalid/parent-only'\n",
    );
    // The alternate filename must not be parsed, even if it is unusable.
    fixture.write("nested/.cargo/config.toml", b"not valid TOML");
    fixture.write(
        "nested/workspace/.cargo/config.toml",
        b"[registries]\nshared.index = 'sparse+https://example.invalid/workspace/'\n",
    );
    let root = fixture.path().join("nested/workspace");
    let git = GitRepo::discover(&root).unwrap();
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: &root,
        paths: Vec::new(),
        case: PathCase::Sensitive,
    };
    assert_eq!(
        work_tree_registry_indices(&tracked).unwrap(),
        BTreeMap::from([
            (
                "shared".to_owned(),
                "sparse+https://example.invalid/workspace/".to_owned()
            ),
            (
                "root_only".to_owned(),
                "https://example.invalid/root-only".to_owned()
            ),
            (
                "parent_only".to_owned(),
                "https://example.invalid/parent-only".to_owned()
            ),
        ])
    );
}

#[test]
#[cfg_attr(miri, ignore = "uses a temporary repository and real filesystem")]
fn registry_configuration_distinguishes_absence_from_read_and_parse_errors() {
    let fixture = Repository::new();
    let git = fixture.repo();
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: fixture.path(),
        paths: Vec::new(),
        case: PathCase::Sensitive,
    };
    assert!(work_tree_registry_indices(&tracked).unwrap().is_empty());
    fixture.write(".cargo/config.toml", b"not valid TOML");
    assert!(
        work_tree_registry_indices(&tracked)
            .unwrap_err()
            .find_source::<toml_edit::TomlError>()
            .is_some()
    );
    // A read failure on the preferred name cannot fall back to the alternate.
    fixture.write(
        ".cargo/config.toml",
        b"[registries.private]\nindex = 'https://example.invalid/index'\n",
    );
    fs::create_dir_all(fixture.path().join(".cargo/config")).unwrap();
    assert!(
        work_tree_registry_indices(&tracked)
            .unwrap_err()
            .find_source::<std::io::Error>()
            .is_some()
    );
}

#[test]
#[cfg_attr(miri, ignore = "uses a temporary repository and real filesystem")]
fn path_acquisition_preserves_identity_and_missing_or_unreadable_targets() {
    let fixture = Repository::new();
    let git = fixture.repo();
    let manifest_path = "vendor/foo/Cargo.toml";
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: fixture.path(),
        // A tracked manifest may be deleted or replaced in the work tree.
        paths: vec![manifest_path.to_owned()],
        case: PathCase::Sensitive,
    };
    let manifests = ManifestSnapshot {
        documents: BTreeMap::new(),
        packages: BTreeMap::new(),
    };
    let lockfile = Lockfile::parse(
        "[[package]]\nname = 'tool'\nversion = '1.0.0'\ndependencies = ['foo']\n\
         [[package]]\nname = 'foo'\nversion = '1.2.0'\n",
        "Cargo.lock",
    )
    .unwrap();
    let mut missing = path_installation();
    resolve_installation_paths(&mut missing, &manifests, &tracked);
    assert!(
        lockfile
            .closure("tool", "1.0.0", &missing)
            .unwrap()
            .is_none()
    );

    fixture.write(
        manifest_path,
        b"[package]\nname = 'foo'\nversion = '1.2.0'\n",
    );
    let mut present = path_installation();
    resolve_installation_paths(&mut present, &manifests, &tracked);
    assert_eq!(
        lockfile
            .closure("tool", "1.0.0", &present)
            .unwrap()
            .unwrap(),
        BTreeMap::from([("foo".to_owned(), BTreeSet::from(["1.2.0".to_owned()]))])
    );
    assert_eq!(
        present
            .patches
            .first()
            .unwrap()
            .replacement
            .as_ref()
            .unwrap()
            .source,
        DependencySource::Path(PackageIdentity {
            name: "foo".to_owned(),
            version: Version::new(1, 2, 0),
        })
    );

    // Invalid UTF-8 is an operational read failure, not a missing path identity.
    fixture.write(manifest_path, &[0xff]);
    let mut unreadable = path_installation();
    resolve_installation_paths(&mut unreadable, &manifests, &tracked);
    assert!(
        lockfile
            .closure("tool", "1.0.0", &unreadable)
            .unwrap_err()
            .find_source::<std::io::Error>()
            .is_some()
    );
    fixture.write(manifest_path, b"not valid TOML");
    let mut malformed = path_installation();
    resolve_installation_paths(&mut malformed, &manifests, &tracked);
    assert!(
        lockfile
            .closure("tool", "1.0.0", &malformed)
            .unwrap_err()
            .find_source::<toml_edit::TomlError>()
            .is_some()
    );
}

#[test]
#[cfg_attr(miri, ignore = "uses a temporary repository and real filesystem")]
fn path_acquisition_does_not_read_untracked_manifests() {
    let fixture = Repository::new();
    fixture.write(
        "vendor/foo/Cargo.toml",
        b"[package]\nname = 'foo'\nversion = '1.2.0'\n",
    );
    let git = fixture.repo();
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: fixture.path(),
        paths: Vec::new(),
        case: PathCase::Sensitive,
    };
    let manifests = ManifestSnapshot {
        documents: BTreeMap::new(),
        packages: BTreeMap::new(),
    };
    let mut installation = path_installation();
    resolve_installation_paths(&mut installation, &manifests, &tracked);
    assert!(matches!(
        installation
            .patches
            .first()
            .unwrap()
            .replacement
            .as_ref()
            .unwrap()
            .source,
        DependencySource::UnresolvedPath(_)
    ));
}

fn path_installation() -> InstallationGraph {
    let root = parse_document(
        Path::new("Cargo.toml"),
        "[package]\nname = 'tool'\nversion = '1.0.0'\n\
         [dependencies]\nfoo = '1'\n\
         [patch.crates-io]\nfoo = { path = 'vendor/foo' }\n",
    )
    .unwrap();
    let package = parse_package_manifest(
        &root.to_string(),
        "Cargo.toml",
        &WorkspaceInherit::default(),
    )
    .unwrap()
    .unwrap();
    let mut installation = InstallationGraph::default();
    installation.insert(
        package.name,
        package.version,
        package.installation_dependencies,
    );
    installation.patches = installation_patches(&root);
    installation
}
