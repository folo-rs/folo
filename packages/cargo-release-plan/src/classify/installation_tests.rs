//! Historical installation acquisition and independent lockfile endpoints.

use super::*;
use crate::ParseTomlError;
use crate::git::testing::Repository;
use crate::groups::Groups;
use crate::manifest::DependencySource;

#[test]
#[cfg_attr(miri, ignore = "reads historical files through Git subprocesses")]
fn historical_registry_configuration_overlays_ambient_using_recorded_files() {
    let fixture = Repository::new();
    fixture.write(
        ".cargo/config.toml",
        b"[registries]\nshared.index = 'https://example.invalid/root'\n\
          root_only.index = 'https://example.invalid/root-only'\n",
    );
    fixture.write(
        "nested/.cargo/config",
        b"[registries]\nshared.index = 'https://example.invalid/parent'\n",
    );
    fixture.write("nested/.cargo/config.toml", b"not valid TOML");
    fixture.write(
        "nested/workspace/.cargo/config.toml",
        b"[registries]\nshared.index = 'sparse+https://example.invalid/workspace/'\n",
    );
    fixture.command(&["add", ".cargo", "nested"]);
    fixture.command(&["commit", "--quiet", "-m", "Registry configuration"]);
    let git = GitRepo::discover(&fixture.path().join("nested/workspace")).unwrap();
    let paths = git.ls_tree_paths("HEAD").unwrap();
    // Current content cannot replace the historical endpoint.
    fixture.write(".cargo/config.toml", b"not valid TOML");
    let ambient = BTreeMap::from([
        (
            "ambient".to_owned(),
            "https://example.invalid/ambient".to_owned(),
        ),
        (
            "shared".to_owned(),
            "https://example.invalid/ambient-shared".to_owned(),
        ),
    ]);
    assert_eq!(
        historical_registries(&git, "HEAD", &ambient, &paths).unwrap(),
        BTreeMap::from([
            (
                "ambient".to_owned(),
                "https://example.invalid/ambient".to_owned()
            ),
            (
                "shared".to_owned(),
                "sparse+https://example.invalid/workspace/".to_owned()
            ),
            (
                "root_only".to_owned(),
                "https://example.invalid/root-only".to_owned()
            ),
        ])
    );
    assert_eq!(
        historical_registries(&git, "HEAD", &ambient, &[]).unwrap(),
        ambient
    );
}

#[test]
#[cfg_attr(miri, ignore = "reads historical files through Git subprocesses")]
fn historical_registry_configuration_preserves_parse_errors() {
    let fixture = Repository::new();
    fixture.write(".cargo/config", b"not valid TOML");
    fixture.write(
        ".cargo/config.toml",
        b"[registries.private]\nindex = 'https://example.invalid/index'\n",
    );
    fixture.command(&["add", ".cargo"]);
    fixture.command(&[
        "commit",
        "--quiet",
        "-m",
        "Malformed preferred configuration",
    ]);
    let git = fixture.repo();
    let paths = git.ls_tree_paths("HEAD").unwrap();
    assert!(
        historical_registries(&git, "HEAD", &BTreeMap::new(), &paths)
            .unwrap_err()
            .find_source::<ParseTomlError>()
            .is_some()
    );
}

#[test]
#[cfg_attr(miri, ignore = "reads historical files through Git subprocesses")]
fn historical_paths_read_target_versions_from_their_own_workspace() {
    let fixture = Repository::new();
    fixture.write(
        "vendor/Cargo.toml",
        b"[workspace]\nmembers = ['foo']\n[workspace.package]\nversion = '1.2.0'\n",
    );
    fixture.write(
        "vendor/foo/Cargo.toml",
        b"[package]\nname = 'foo'\nversion.workspace = true\n",
    );
    fixture.command(&["add", "vendor"]);
    fixture.command(&["commit", "--quiet", "-m", "Path package"]);
    let git = fixture.repo();
    let paths = git.ls_tree_paths("HEAD").unwrap();
    fixture.write(
        "vendor/Cargo.toml",
        b"[workspace]\n[workspace.package]\nversion = '9.0.0'\n",
    );
    let root = parse_document(
        Path::new("Cargo.toml"),
        "[patch.crates-io]\nfoo = { path = 'vendor/foo' }\n",
    )
    .unwrap();
    let mut installation = InstallationGraph::default();
    installation.patches = installation_patches(&root);
    resolve_historical_installation_paths(
        &mut installation,
        BTreeMap::new(),
        &git,
        "HEAD",
        &paths,
        PathCase::Sensitive,
    );
    assert_eq!(
        installation
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
}

#[test]
fn lockfile_decoding_preserves_text_and_rejects_non_utf8() {
    let text = "[[package]]\nname = 'tool'\nversion = '1.0.0'\n";
    assert_eq!(
        decode_lockfile(text.as_bytes().to_vec(), "Cargo.lock").unwrap(),
        text
    );
    assert_eq!(decode_lockfile(Vec::new(), "Cargo.lock").unwrap(), "");
    assert!(
        decode_lockfile(vec![0xff], "Cargo.lock")
            .unwrap_err()
            .find_source::<MalformedLockfileError>()
            .is_some()
    );
}

#[test]
#[cfg_attr(miri, ignore = "reads historical lockfiles through Git subprocesses")]
fn lockfile_changes_select_each_endpoint_independently() {
    let fixture = Repository::new();
    fixture.write("Cargo.lock", locked_dependency("1.0.0").as_bytes());
    fixture.command(&["add", "Cargo.lock"]);
    fixture.command(&["commit", "--quiet", "-m", "Anchor resolution"]);
    fixture.write("Cargo.lock", locked_dependency("1.1.0").as_bytes());
    let git = fixture.repo();
    for (anchor_binary, work_binary, expected) in [
        (false, false, None),
        (false, true, Some(ClosureChange::Added)),
        (true, false, Some(ClosureChange::Deleted)),
        (true, true, Some(ClosureChange::Modified)),
    ] {
        let (work_tree, work_package, anchor) =
            endpoints(fixture.path(), anchor_binary, work_binary);
        let mut cache = LockfileCache::default();
        let changes = lockfile_closure_changes(
            &mut cache,
            &git,
            &work_tree,
            "tool",
            &anchor,
            &work_package,
            "HEAD",
            &InstallationGraph::default(),
        )
        .unwrap();
        assert_eq!(
            changes,
            expected
                .map(|change| ("foo".to_owned(), change))
                .into_iter()
                .collect::<Vec<_>>()
        );
        assert_eq!(cache.anchors.contains_key("HEAD"), anchor_binary);
        assert_eq!(cache.work.is_some(), work_binary);
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads historical lockfiles through Git subprocesses")]
fn library_endpoints_need_no_lockfiles_but_binary_endpoints_do() {
    let fixture = Repository::new();
    fixture.command(&["commit", "--quiet", "--allow-empty", "-m", "No lockfile"]);
    let git = fixture.repo();
    for (anchor_binary, work_binary) in [(false, false), (true, false), (false, true)] {
        let (work_tree, work_package, anchor) =
            endpoints(fixture.path(), anchor_binary, work_binary);
        let result = lockfile_closure_changes(
            &mut LockfileCache::default(),
            &git,
            &work_tree,
            "tool",
            &anchor,
            &work_package,
            "HEAD",
            &InstallationGraph::default(),
        );
        if anchor_binary || work_binary {
            assert!(
                result
                    .unwrap_err()
                    .find_source::<LockfileClosureUnavailableError>()
                    .is_some()
            );
        } else {
            assert!(result.unwrap().is_empty());
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "reads historical lockfiles through Git subprocesses")]
fn anchor_lockfiles_are_selected_by_commit_not_current_work_tree() {
    let fixture = Repository::new();
    fixture.write("Cargo.lock", locked_dependency("1.0.0").as_bytes());
    fixture.command(&["add", "Cargo.lock"]);
    fixture.command(&["commit", "--quiet", "-m", "First resolution"]);
    let first = fixture.command(&["rev-parse", "HEAD"]).trim().to_owned();
    fixture.write("Cargo.lock", locked_dependency("1.1.0").as_bytes());
    fixture.command(&["add", "Cargo.lock"]);
    fixture.command(&["commit", "--quiet", "-m", "Second resolution"]);
    let second = fixture.command(&["rev-parse", "HEAD"]).trim().to_owned();
    fixture.write("Cargo.lock", b"not valid TOML");
    let git = fixture.repo();
    let mut cache = LockfileCache::default();
    for (commit, version) in [(&first, "1.0.0"), (&second, "1.1.0"), (&first, "1.0.0")] {
        let lockfile = cache.anchor(&git, "tool", commit, "Cargo.lock").unwrap();
        assert_eq!(
            lockfile
                .closure("tool", "1.0.0", &InstallationGraph::default())
                .unwrap()
                .unwrap(),
            BTreeMap::from([(
                "foo".to_owned(),
                BTreeSet::from([format!(
                    "{version} (registry+https://example.invalid/index)"
                )])
            )])
        );
    }
}

fn locked_dependency(version: &str) -> String {
    format!(
        "[[package]]\nname = 'tool'\nversion = '1.0.0'\ndependencies = ['foo']\n\
         [[package]]\nname = 'foo'\nversion = '{version}'\n\
         source = 'registry+https://example.invalid/index'\n"
    )
}

fn endpoints(
    root: &Path,
    anchor_binary: bool,
    work_binary: bool,
) -> (WorkTree, WorkPackage, HistoricalPackage) {
    let manifest = parse_package_manifest(
        "[package]\nname = 'tool'\nversion = '1.0.0'\n",
        "Cargo.toml",
        &WorkspaceInherit::default(),
    )
    .unwrap()
    .unwrap();
    let anchor = HistoricalPackage {
        directory: manifest.directory.clone(),
        version: manifest.version.clone(),
        packaging: manifest.packaging.clone(),
        resources: BTreeMap::new(),
        auto_readme: false,
        has_lockfile_target: anchor_binary,
    };
    let work_package = WorkPackage {
        manifest,
        manifest_path: root.join("Cargo.toml"),
        dependencies: Vec::new(),
        consumer_contract: false,
        has_lockfile_target: work_binary,
        resources: BTreeMap::new(),
    };
    let work_tree = WorkTree {
        workspace_root: root.to_path_buf(),
        packages: vec![work_package.clone()],
        version_targets: Vec::new(),
        exact_dependencies: Vec::new(),
        member_manifests: vec![work_package.manifest_path.clone()],
        members_by_dir: BTreeMap::new(),
        groups: Groups::default(),
        installation: InstallationGraph::default(),
    };
    (work_tree, work_package, anchor)
}
