//! Historical installation acquisition and independent lockfile endpoints.

use crp_workspace::manifest::{DependencySource, InstallationDependencies};

use super::*;

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
fn historical_registry_overlay_prefers_recorded_extensionless_and_inner_configuration() {
    let paths = [
        ".cargo/CONFIG",
        ".cargo/config.toml",
        "nested/.cargo/config.toml",
    ]
    .map(str::to_owned);
    let ambient = BTreeMap::from([
        ("ambient".into(), "ambient-index".into()),
        ("shared".into(), "ambient-index".into()),
    ]);
    let mut reads = Vec::new();
    let indices =
        historical_registries_with("nested", &ambient, &paths, PathCase::Insensitive, |path| {
            reads.push(path.to_owned());
            Ok(Some(
                if path == ".cargo/CONFIG" {
                    "[registries.shared]\nindex='outer'\n[registries.outer]\nindex='outer'\n"
                } else {
                    assert_eq!(path, "nested/.cargo/config.toml");
                    "[registries.shared]\nindex='inner'\n"
                }
                .into(),
            ))
        })
        .unwrap();
    assert_eq!(
        indices,
        BTreeMap::from([
            ("ambient".into(), "ambient-index".into()),
            ("outer".into(), "outer".into()),
            ("shared".into(), "inner".into())
        ])
    );
    assert_eq!(reads, [".cargo/CONFIG", "nested/.cargo/config.toml"]);
    historical_registries_with("", &ambient, &paths, PathCase::Insensitive, |_| {
        Err(io::Error::other("git").into())
    })
    .unwrap_err();
}

#[test]
fn historical_path_acquisition_resolves_recorded_identity_and_retains_operational_errors() {
    let git = GitRepo {
        root: PathBuf::from("repository"),
        prefix: String::new(),
    };
    let paths = ["Helper/Cargo.toml".to_owned()];
    let package = parse_package_manifest(
        "[package]\nname='tool'\nversion='1.0.0'\n[dependencies]\nhelper={path='../helper'}\n",
        "tool/Cargo.toml",
        &WorkspaceInherit::default(),
    )
    .unwrap()
    .unwrap();
    for failure in [false, true] {
        let mut graph = InstallationGraph::default();
        graph.insert(
            "tool".into(),
            Version::new(1, 0, 0),
            package.installation_dependencies.clone(),
        );
        resolve_historical_installation_paths_with(
            &mut graph,
            BTreeMap::new(),
            &git,
            &paths,
            PathCase::Insensitive,
            |path| {
                assert_eq!(path, "Helper/Cargo.toml");
                if failure {
                    Err(io::Error::other("git").into())
                } else {
                    Ok(Some("[package]\nname='helper'\nversion='2.0.0'\n".into()))
                }
            },
        );
        let InstallationDependencies::Parsed(dependencies) = &graph.members.get("tool").unwrap().1
        else {
            panic!("valid declarations")
        };
        assert_eq!(
            matches!(
                dependencies.first().unwrap().source,
                DependencySource::Path(_)
            ),
            !failure
        );
        assert_eq!(graph.path_errors.len(), usize::from(failure));
    }
}

#[test]
fn anchor_lockfiles_are_acquired_once_per_commit_and_missing_is_an_error() {
    let mut cache = LockfileCache {
        work: None,
        anchors: HashMap::new(),
        case: PathCase::Sensitive,
    };
    let bytes = b"version = 4\n[[package]]\nname='tool'\nversion='1.0.0'\n";
    cache
        .anchor_with("tool", "first", "Cargo.lock", || Ok(Some(bytes.to_vec())))
        .unwrap();
    cache
        .anchor_with("tool", "first", "Cargo.lock", || panic!("cached anchor"))
        .unwrap();
    cache
        .anchor_with("tool", "second", "Cargo.lock", || Ok(Some(bytes.to_vec())))
        .unwrap();
    assert_eq!(cache.anchors.len(), 2);
    assert!(
        cache
            .anchor_with("tool", "absent", "Cargo.lock", || Ok(None))
            .unwrap_err()
            .find_source::<LockfileClosureUnavailableError>()
            .is_some()
    );
    cache
        .anchor_with("tool", "failed", "Cargo.lock", || {
            Err(io::Error::other("git").into())
        })
        .unwrap_err();
}

#[test]
fn binary_endpoints_independently_contribute_locked_closures() {
    let lock = |version| {
        Lockfile::parse(&format!(
        "version=4\n[[package]]\nname='tool'\nversion='1.0.0'\ndependencies=['dependency']\n[[package]]\nname='dependency'\nversion='{version}'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n"
    ), "Cargo.lock").unwrap()
    };
    let git = GitRepo {
        root: PathBuf::from("unused"),
        prefix: String::new(),
    };
    let mut work = fixture::classification(vec![]).work_tree;
    let manifest = parse_package_manifest(
        "[package]\nname='tool'\nversion='1.0.0'\n[dependencies]\ndependency='*'\n",
        "tool/Cargo.toml",
        &WorkspaceInherit::default(),
    )
    .unwrap()
    .unwrap();
    work.installation.insert(
        "tool".into(),
        manifest.version.clone(),
        manifest.installation_dependencies.clone(),
    );
    let mut cache = LockfileCache {
        work: Some(lock("2.0.0")),
        anchors: HashMap::from([("anchor".into(), lock("1.0.0"))]),
        case: PathCase::Sensitive,
    };
    for anchor_binary in [false, true] {
        for work_binary in [false, true] {
            let anchor = HistoricalPackage {
                directory: "tool".into(),
                version: manifest.version.clone(),
                packaging: PackagingRules::default(),
                resources: BTreeMap::new(),
                auto_readme: false,
                has_lockfile_target: anchor_binary,
            };
            let package = WorkPackage {
                manifest: manifest.clone(),
                manifest_path: "tool/Cargo.toml".into(),
                dependencies: vec![],
                consumer_contract: false,
                has_lockfile_target: work_binary,
                resources: BTreeMap::new(),
            };
            let changes = lockfile_closure_changes(
                &mut cache,
                &git,
                &work,
                "tool",
                &anchor,
                &package,
                "anchor",
                &work.installation,
            )
            .unwrap();
            let expected = match (anchor_binary, work_binary) {
                (false, false) => vec![],
                (false, true) => vec![("dependency".into(), ClosureChange::Added)],
                (true, false) => vec![("dependency".into(), ClosureChange::Deleted)],
                (true, true) => vec![("dependency".into(), ClosureChange::Modified)],
            };
            assert_eq!(changes, expected);
        }
    }
}
