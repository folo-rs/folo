//! Synthetic manifests and injected acquisition failures for edit calculation.

use crp_workspace::lockfile::InstallationGraph;
use crp_workspace::metadata::{ExactDependency, ManifestSnapshot, VersionTarget};

use super::super::*;

/// Distinguishes injected acquisition/application failures from successful empty work.
#[ohno::error]
pub(crate) struct ApplicationFailure;

pub(crate) fn failed_edit_computation(failure: Result<String, AppError>) -> AppError {
    let originals = manifests();
    let mut failure = Some(failure);
    let mut reads = Vec::new();
    let paths = unique_paths();
    let last = paths.last().unwrap();
    let error = compute_edits_with(
        &work_tree(),
        &versions("0.2.0"),
        Verbose::new(false, &crp_diag::Discard),
        |path| {
            reads.push(path.to_path_buf());
            if path == last {
                failure.take().unwrap()
            } else {
                Ok(originals.get(path).unwrap().clone())
            }
        },
    )
    .err()
    .unwrap();
    assert_eq!(reads, paths);
    error
}

pub(crate) fn versions(version: &str) -> ResolvedVersions {
    ResolvedVersions {
        packages: ["root", "api", "helper"]
            .into_iter()
            .map(|name| (name.to_owned(), version.parse().unwrap()))
            .collect(),
    }
}

pub(crate) fn unique_paths() -> [PathBuf; 4] {
    ["", "api", "helper", "untouched"]
        .map(|member| PathBuf::from("workspace").join(member).join("Cargo.toml"))
}

pub(crate) fn work_tree() -> WorkTree {
    let paths = unique_paths();
    WorkTree {
        manifests: ManifestSnapshot::default(),
        tracked_paths: Vec::new(),
        workspace_root: PathBuf::from("workspace"),
        packages: Vec::new(),
        version_targets: ["root", "api", "helper", "untouched"]
            .into_iter()
            .zip(&paths)
            .map(|(name, path)| VersionTarget {
                name: name.to_owned(),
                version: Version::new(0, 1, 0),
                manifest_path: path.clone(),
                publishable: name != "helper",
            })
            .collect(),
        // Mirror the helper manifest's exact path dependency in manifests() and its path
        // slot in unique_paths(); acquired metadata and editable text must describe one edge.
        exact_dependencies: vec![ExactDependency {
            source: "helper".to_owned(),
            target: "api".to_owned(),
            requirement: "=0.1.0".to_owned(),
            manifest_path: paths[2].clone(),
            location: "dependencies.api".to_owned(),
        }],
        member_manifests: [
            &paths[0], &paths[1], &paths[2], &paths[1], &paths[0], &paths[3],
        ]
        .into_iter()
        .cloned()
        .collect(),
        members_by_dir: ["root", "api", "helper", "untouched"]
            .into_iter()
            .zip(&paths)
            .map(|(name, path)| (path.parent().unwrap().to_path_buf(), name.to_owned()))
            .collect(),
        installation: InstallationGraph::default(),
    }
}

pub(crate) fn manifests() -> BTreeMap<PathBuf, String> {
    unique_paths()
        .into_iter()
        .zip([
            "[package]\nname = 'root'\nversion = '0.1.0' # local\n",
            "[package]\nname = 'api'\nversion = '0.1.0'\n",
            concat!(
                "[package]\nname = 'helper'\nversion = '0.1.0'\npublish = false\n",
                "[dependencies]\napi = { path = '../api', version = '=0.1.0' }\n",
            ),
            "# preserve spacing and quoting\n[package]\nname = 'untouched'\nversion  =  '0.1.0'\n",
        ])
        .map(|(path, text)| (path, text.to_owned()))
        .collect()
}

pub(crate) fn updated_manifests() -> BTreeMap<PathBuf, String> {
    let mut updated = manifests();
    let paths = unique_paths();
    updated.insert(
        paths[0].clone(),
        "[package]\nname = 'root'\nversion = \"0.2.0\" # local\n".to_owned(),
    );
    updated.insert(
        paths[1].clone(),
        "[package]\nname = 'api'\nversion = \"0.2.0\"\n".to_owned(),
    );
    updated.insert(
        paths[2].clone(),
        concat!(
            "[package]\nname = 'helper'\nversion = \"0.2.0\"\npublish = false\n",
            "[dependencies]\napi = { path = '../api', version = \"=0.2.0\" }\n",
        )
        .to_owned(),
    );
    updated
}
