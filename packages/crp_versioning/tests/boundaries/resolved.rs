//! External acquisition for resolved.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::{fs, slice};

use crp_diag::Verbose;
use crp_versioning::classify::{PackageStatus, classify};
use crp_versioning::resolved::*;
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};

static GIT_CONFIG: LazyLock<TempDir> = LazyLock::new(|| {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("config"), "").unwrap();
    directory
});

fn git(root: &Path, args: &[&str]) -> String {
    // Capture fixtures need real index/history semantics, but no resolver or preview.
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", GIT_CONFIG.path().join("config"))
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env("GIT_TEMPLATE_DIR", "")
        .args([
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "gc.auto=0",
            "-c",
            "core.autocrlf=false",
            "-c",
            "init.templateDir=",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn capture_fixture(manifest: &str) -> TempDir {
    let directory = tempdir().unwrap();
    let path = directory.path().join(manifest);
    let package = path.parent().unwrap();
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        &path,
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
    )
    .unwrap();
    fs::write(package.join("src/lib.rs"), "pub fn released() {}\n").unwrap();
    fs::write(directory.path().join(".gitignore"), "**/src/generated.rs\n").unwrap();
    fs::create_dir_all(directory.path().join(".cargo")).unwrap();
    fs::write(
        directory.path().join(".cargo/config.toml"),
        "[term]\nquiet = true\n",
    )
    .unwrap();
    git(directory.path(), &["init", "--quiet"]);
    git(directory.path(), &["add", "."]);
    git(
        directory.path(),
        &["commit", "--quiet", "-m", "released fixture"],
    );
    directory
}

#[test]
#[cfg_attr(
    miri,
    ignore = "captures a real Git and Cargo workspace without resolution"
)]
fn capture_includes_local_sources_without_turning_them_into_release_reasons() {
    let directory = capture_fixture("Cargo.toml");
    let manifest = directory.path().join("Cargo.toml");
    let sources = ["src/untracked.rs", "src/generated.rs"];
    for source in sources {
        fs::write(directory.path().join(source), "pub fn local() {}\n").unwrap();
    }
    let inputs = Inputs::capture(&manifest, Some("HEAD")).unwrap();
    for source in sources {
        assert!(inputs.paths.contains(Path::new(source)));
        assert!(!inputs.index.contains(source));
        fs::write(directory.path().join(source), "pub fn changed() {}\n").unwrap();
        let _error = inputs.verify(&manifest, None).unwrap_err();
        fs::write(directory.path().join(source), "pub fn local() {}\n").unwrap();
    }
    let classification = classify(
        &manifest,
        Some("HEAD"),
        Verbose::new(false, &crp_diag::Discard),
    )
    .unwrap();
    assert_eq!(
        classification.packages.first().unwrap().status(),
        PackageStatus::Unchanged
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "captures a nested Git and Cargo workspace without resolution"
)]
fn nested_capture_records_ancestor_configuration_and_the_default_base() {
    let directory = capture_fixture("rust/Cargo.toml");
    let head = git(directory.path(), &["rev-parse", "HEAD"]);
    git(
        directory.path(),
        &["update-ref", "refs/remotes/origin/main", head.trim()],
    );
    let inputs = Inputs::capture(&directory.path().join("rust/Cargo.toml"), None).unwrap();
    assert_eq!(inputs.manifest, Path::new("rust/Cargo.toml"));
    assert_eq!(inputs.base, head.trim());
    assert!(inputs.paths.contains(Path::new(".cargo/config.toml")));
    assert!(inputs.paths.contains(Path::new("rust/.cargo/config.toml")));
    assert!(inputs.paths.contains(Path::new("rust/Cargo.lock")));
}

fn inputs() -> Inputs {
    Inputs {
        root: PathBuf::from("repository"),
        manifest: PathBuf::from("Cargo.toml"),
        head: "head".to_owned(),
        base: "base".to_owned(),
        base_revision: "main".to_owned(),
        index: "index".to_owned(),
        paths: ["Cargo.toml", "Cargo.lock", "src/lib.rs"]
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        digest: "initial".to_owned(),
    }
}

#[test]
#[cfg_attr(miri, ignore = "canonicalizes owned manifest paths")]
fn evidence_must_name_the_recorded_candidate_and_never_the_live_workspace() {
    let directory = tempdir().unwrap();
    let live = directory.path().join("Cargo.toml");
    let candidate = directory.path().join("candidate.toml");
    let other = directory.path().join("other.toml");
    for path in [&live, &candidate, &other] {
        fs::write(path, "").unwrap();
    }
    let mut inputs = inputs();
    inputs.root = directory.path().to_owned();
    let mut state = ResolvedState {
        inputs,
        files: Vec::new(),
        final_digest: "final".to_owned(),
        versions: BTreeMap::new(),
        evidence_manifest_path: candidate,
    };
    let _error = state.verify_candidate(&other).unwrap_err();
    state.evidence_manifest_path = live.clone();
    let _error = state.verify_candidate(&live).unwrap_err();
    let error = state
        .verify_candidate(&directory.path().join("absent.toml"))
        .unwrap_err();
    assert!(error.find_source::<std::io::Error>().is_some());
}

#[test]
#[cfg_attr(miri, ignore = "reads owned files and invokes Git hashing")]
fn resolved_artifacts_require_exact_versions_membership_and_complete_final_bytes() {
    let directory = tempdir().unwrap();
    let mut inputs = inputs();
    inputs.root = directory.path().to_owned();
    fs::create_dir_all(directory.path().join("src")).unwrap();
    fs::write(directory.path().join("src/lib.rs"), "source").unwrap();
    fs::write(directory.path().join("Cargo.toml"), "old manifest").unwrap();
    fs::write(directory.path().join("Cargo.lock"), "old lockfile").unwrap();
    let files = vec![
        Artifact {
            path: "Cargo.toml".into(),
            contents: "new manifest".to_owned(),
        },
        Artifact {
            path: "Cargo.lock".into(),
            contents: "new lockfile".to_owned(),
        },
    ];
    let versions = BTreeMap::from([("demo".to_owned(), "0.1.1".to_owned())]);
    let allowed = BTreeSet::from([PathBuf::from("Cargo.toml"), PathBuf::from("Cargo.lock")]);
    let state = ResolvedState {
        final_digest: inputs.final_digest(&files).unwrap(),
        inputs,
        files,
        versions: versions.clone(),
        evidence_manifest_path: "not-read/Cargo.toml".into(),
    };
    state.validate_artifacts(&versions, &allowed).unwrap();
    let original = serde_json::to_value(&state).unwrap();
    let original_files = original.get("files").unwrap().as_array().unwrap();
    let mut duplicate = original_files.clone();
    duplicate.push(original_files.first().unwrap().clone());
    let mut missing = original_files.clone();
    missing.pop().unwrap();
    let mut changed_bytes = original_files.clone();
    *changed_bytes
        .first_mut()
        .unwrap()
        .get_mut("contents")
        .unwrap() = json!("other manifest");
    let mut source = original_files.clone();
    source.push(json!({"path":"src/lib.rs","contents":"unplanned source"}));
    let mut outside = original_files.clone();
    outside.push(json!({"path":"../Cargo.toml","contents":"outside"}));
    for (field, value) in [
        ("versions", json!({"demo":"9.0.0"})),
        ("final_digest", json!("wrong digest")),
        ("files", json!(duplicate)),
        ("files", json!(missing)),
        ("files", json!(changed_bytes)),
        ("files", json!(source)),
        ("files", json!(outside)),
    ] {
        let mut changed: Value = original.clone();
        *changed.get_mut(field).unwrap() = value;
        let changed: ResolvedState = serde_json::from_value(changed).unwrap();
        let _error = changed.validate_artifacts(&versions, &allowed).unwrap_err();
    }
    let mut uncaptured = state.clone();
    uncaptured.inputs.paths.remove(Path::new("Cargo.lock"));
    let _error = uncaptured
        .validate_artifacts(&versions, &allowed)
        .unwrap_err();
    let mut unowned = allowed;
    unowned.remove(Path::new("Cargo.lock"));
    let _error = state.validate_artifacts(&versions, &unowned).unwrap_err();

    // Once the live bytes already match, an empty resolved write set is complete.
    for file in &state.files {
        fs::write(state.inputs.root.join(&file.path), &file.contents).unwrap();
    }
    let state = ResolvedState {
        files: Vec::new(),
        versions: BTreeMap::new(),
        ..state
    };
    state
        .validate_artifacts(&BTreeMap::new(), &BTreeSet::new())
        .unwrap();
}

#[test]
#[cfg_attr(miri, ignore = "reads an owned filesystem fixture")]
fn source_collection_handles_absence_recursion_and_non_directory_errors() {
    let directory = tempdir().unwrap();
    let source = directory.path().join("src");
    let mut paths = BTreeSet::new();
    collect_sources(directory.path(), &source, &mut paths).unwrap();
    assert!(paths.is_empty());
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("lib.rs"), "").unwrap();
    fs::write(source.join("nested/mod.rs"), "").unwrap();
    collect_sources(directory.path(), &source, &mut paths).unwrap();
    assert_eq!(
        paths,
        ["src/lib.rs", "src/nested/mod.rs"]
            .into_iter()
            .map(PathBuf::from)
            .collect()
    );
    let error = collect_sources(directory.path(), &source.join("lib.rs"), &mut paths).unwrap_err();
    assert!(error.find_source::<std::io::Error>().is_some());
}

#[test]
#[cfg_attr(miri, ignore = "reads files and invokes Git hashing")]
fn fingerprints_distinguish_missing_empty_content_and_paths() {
    let directory = tempdir().unwrap();
    let path = PathBuf::from("Cargo.lock");
    let paths = BTreeSet::from([path.clone()]);
    let missing = fingerprint(directory.path(), &paths, &BTreeMap::new()).unwrap();
    fs::write(directory.path().join(&path), "").unwrap();
    let empty = fingerprint(directory.path(), &paths, &BTreeMap::new()).unwrap();
    assert_ne!(missing, empty);
    let contents = b"resolved bytes".to_vec();
    let replacements = BTreeMap::from([(path.clone(), contents.clone())]);
    let replaced = fingerprint(directory.path(), &paths, &replacements).unwrap();
    assert_ne!(empty, replaced);
    fs::write(directory.path().join(&path), &contents).unwrap();
    assert_eq!(
        fingerprint(directory.path(), &paths, &BTreeMap::new()).unwrap(),
        replaced
    );
    fs::rename(
        directory.path().join(path),
        directory.path().join("Cargo.toml"),
    )
    .unwrap();
    let renamed = fingerprint(
        directory.path(),
        &BTreeSet::from([PathBuf::from("Cargo.toml")]),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_ne!(renamed, replaced);
}

#[test]
#[cfg_attr(miri, ignore = "reads filesystem case aliases and invokes Git hashing")]
fn fingerprint_replacements_follow_the_actual_case_identity() {
    let directory = tempdir().unwrap();
    let original = PathBuf::from("CaseDir/Cargo.toml");
    let alias = PathBuf::from("casedir/cargo.toml");
    fs::create_dir_all(directory.path().join("CaseDir")).unwrap();
    fs::write(directory.path().join(&original), "old").unwrap();
    let aliases = directory.path().join(&alias).exists();
    if !aliases {
        fs::create_dir_all(directory.path().join("casedir")).unwrap();
        fs::write(directory.path().join(&alias), "distinct").unwrap();
    }
    let other = PathBuf::from("other/Cargo.toml");
    fs::create_dir_all(directory.path().join("other")).unwrap();
    fs::write(directory.path().join(&other), "unrelated").unwrap();
    let paths = BTreeSet::from([original.clone(), alias.clone(), other.clone()]);
    let replacements = BTreeMap::from([(original.clone(), b"new".to_vec())]);
    let projected = fingerprint(directory.path(), &paths, &replacements).unwrap();
    fs::write(directory.path().join(original), "new").unwrap();
    assert_eq!(
        fingerprint(directory.path(), &paths, &BTreeMap::new()).unwrap(),
        projected
    );
    assert_eq!(
        fs::read_to_string(directory.path().join(alias)).unwrap(),
        if aliases { "new" } else { "distinct" }
    );
    assert_eq!(
        fs::read_to_string(directory.path().join(other)).unwrap(),
        "unrelated"
    );
}

#[test]
#[cfg_attr(miri, ignore = "reads filesystem case aliases and invokes Git hashing")]
fn missing_lockfile_replacements_follow_parent_directory_case_rules() {
    let directory = tempdir().unwrap();
    fs::create_dir_all(directory.path().join("CaseDir")).unwrap();
    fs::write(directory.path().join("CaseDir/Cargo.toml"), "manifest").unwrap();
    let aliases = directory.path().join("casedir/cargo.toml").exists();
    if !aliases {
        fs::create_dir_all(directory.path().join("casedir")).unwrap();
    }
    let original = PathBuf::from("CaseDir/Cargo.lock");
    let alias = PathBuf::from("casedir/cargo.lock");
    let inputs = Inputs {
        root: directory.path().to_owned(),
        paths: BTreeSet::from([original.clone(), alias.clone()]),
        ..inputs()
    };
    let file = Artifact {
        path: original.clone(),
        contents: "resolved lockfile".to_owned(),
    };
    let projected = inputs.final_digest(slice::from_ref(&file)).unwrap();
    // An explicit replacement at every alias expresses the same projected write without
    // involving filesystem-specific permissions assigned when a missing file is created.
    let mut replacements = BTreeMap::from([(original, file.contents.as_bytes().to_vec())]);
    if aliases {
        replacements.insert(alias.clone(), file.contents.as_bytes().to_vec());
    }
    assert_eq!(
        fingerprint(directory.path(), &inputs.paths, &replacements).unwrap(),
        projected
    );
    assert!(!directory.path().join(alias).exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "compares filesystem case identities and invokes Git hashing"
)]
fn candidate_case_aliases_preserve_captured_membership_and_bytes() {
    let directory = tempdir().unwrap();
    let original = PathBuf::from("CaseDir/Cargo.toml");
    let alias = PathBuf::from("casedir/cargo.toml");
    fs::create_dir_all(directory.path().join("CaseDir")).unwrap();
    fs::write(directory.path().join(&original), "final").unwrap();
    let aliases = directory.path().join(&alias).exists();
    if !aliases {
        fs::create_dir_all(directory.path().join("casedir")).unwrap();
        fs::write(directory.path().join(&alias), "final").unwrap();
    }
    let expected = Inputs {
        root: directory.path().to_owned(),
        manifest: original.clone(),
        paths: BTreeSet::from([original.clone()]),
        ..inputs()
    };
    let candidate = Inputs {
        manifest: alias.clone(),
        paths: BTreeSet::from([alias.clone()]),
        ..expected.clone()
    };
    let digest = fingerprint(directory.path(), &expected.paths, &BTreeMap::new()).unwrap();
    let result = expected.compare_candidate(&candidate, &digest);
    if aliases {
        result.unwrap();
        // A copy can collapse captured names without losing any physical source input.
        let expected = Inputs {
            paths: BTreeSet::from([original.clone(), alias]),
            ..expected.clone()
        };
        let digest = fingerprint(directory.path(), &expected.paths, &BTreeMap::new()).unwrap();
        expected.compare_candidate(&candidate, &digest).unwrap();
    } else {
        assert!(result.is_err());
    }
    for paths in [
        BTreeSet::new(),
        BTreeSet::from([original.clone(), PathBuf::from("uncaptured.rs")]),
    ] {
        let candidate = Inputs {
            paths,
            ..candidate.clone()
        };
        assert!(expected.compare_candidate(&candidate, &digest).is_err());
    }
    fs::write(directory.path().join(original), "changed").unwrap();
    assert!(expected.compare_candidate(&candidate, &digest).is_err());
}

#[test]
#[cfg_attr(miri, ignore = "validates filesystem aliases and invokes Git hashing")]
fn artifact_case_aliases_require_unique_captured_workspace_targets() {
    let directory = tempdir().unwrap();
    let original = PathBuf::from("CaseDir/Cargo.toml");
    let alias = PathBuf::from("casedir/cargo.toml");
    fs::create_dir_all(directory.path().join("CaseDir")).unwrap();
    fs::write(directory.path().join(&original), "old").unwrap();
    let aliases = directory.path().join(&alias).exists();
    if !aliases {
        fs::create_dir_all(directory.path().join("casedir")).unwrap();
        fs::write(directory.path().join(&alias), "distinct").unwrap();
    }
    let inputs = Inputs {
        root: directory.path().to_owned(),
        paths: BTreeSet::from([original.clone()]),
        ..inputs()
    };
    let file = Artifact {
        path: alias,
        contents: "new".to_owned(),
    };
    let result = inputs.final_digest(slice::from_ref(&file));
    if aliases {
        let state = ResolvedState {
            inputs,
            files: vec![file.clone()],
            final_digest: result.unwrap(),
            versions: BTreeMap::new(),
            evidence_manifest_path: "not-read/Cargo.toml".into(),
        };
        let allowed = BTreeSet::from([original.clone()]);
        state
            .validate_artifacts(&BTreeMap::new(), &allowed)
            .unwrap();
        let _error = state
            .validate_artifacts(&BTreeMap::new(), &BTreeSet::new())
            .unwrap_err();
        let _error = state
            .inputs
            .final_digest(&[
                file,
                Artifact {
                    path: original,
                    contents: "other".to_owned(),
                },
            ])
            .unwrap_err();
    } else {
        result.unwrap_err();
    }
}

#[test]
#[cfg(unix)]
#[cfg_attr(
    miri,
    ignore = "reads filenames with literal backslashes and invokes Git hashing"
)]
fn replacement_identity_preserves_literal_backslashes() {
    let directory = tempdir().unwrap();
    let literal = PathBuf::from("case\\dir/Cargo.toml");
    let separated = PathBuf::from("case/dir/Cargo.toml");
    for path in [&literal, &separated] {
        fs::create_dir_all(directory.path().join(path).parent().unwrap()).unwrap();
        fs::write(directory.path().join(path), "old").unwrap();
    }
    let paths = BTreeSet::from([literal.clone(), separated.clone()]);
    let replacements = BTreeMap::from([(literal.clone(), b"new".to_vec())]);
    let projected = fingerprint(directory.path(), &paths, &replacements).unwrap();
    fs::write(directory.path().join(literal), "new").unwrap();
    assert_eq!(
        fingerprint(directory.path(), &paths, &BTreeMap::new()).unwrap(),
        projected
    );
    assert_eq!(
        fs::read_to_string(directory.path().join(separated)).unwrap(),
        "old"
    );
}

#[test]
#[cfg_attr(miri, ignore = "uses filesystem metadata")]
fn fingerprints_reject_a_directory_as_a_file() {
    let directory = tempdir().unwrap();
    fs::create_dir_all(directory.path().join("Cargo.lock")).unwrap();
    let _error = fingerprint(
        directory.path(),
        &BTreeSet::from([PathBuf::from("Cargo.lock")]),
        &BTreeMap::new(),
    )
    .unwrap_err();
}

#[test]
#[cfg_attr(miri, ignore = "reads local dependency manifests")]
fn capture_includes_workspace_and_replacement_sources() {
    let directory = tempdir().unwrap();
    // Fixture writes belong to the TempDir, never to a production path helper's result.
    // Canonicalization is observed only after setup, so a failing mutant cannot redirect it.
    let root = directory.path();
    fs::create_dir_all(root.join("replacement/src/nested")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[workspace.dependencies]\nhelper = { path = \"replacement\" }\n\
             [replace]\n\"helper:0.1.0\" = { path = \"replacement\" }\n",
    )
    .unwrap();
    fs::write(
        root.join("replacement/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(root.join("replacement/src/nested/lib.rs"), "").unwrap();
    let root = canonical(root).unwrap();
    let root = root.as_path();
    let mut paths = BTreeSet::new();
    capture_path_dependencies(root, [&root.join("Cargo.toml")], &mut paths).unwrap();
    assert_eq!(
        paths,
        [
            "Cargo.toml",
            "replacement/Cargo.toml",
            "replacement/src/nested/lib.rs"
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    );
}

#[test]
#[cfg_attr(miri, ignore = "reads an owned dependency manifest")]
fn absolute_dependency_paths_cannot_leak_back_to_the_live_workspace() {
    let directory = tempdir().unwrap();
    let manifest = directory.path().join("Cargo.toml");
    fs::write(
        &manifest,
        format!(
            "[dependencies]\nhelper = {{ path = {:?} }}\n",
            directory.path()
        ),
    )
    .unwrap();
    let _error =
        capture_path_dependencies(directory.path(), [&manifest], &mut BTreeSet::new()).unwrap_err();
}
