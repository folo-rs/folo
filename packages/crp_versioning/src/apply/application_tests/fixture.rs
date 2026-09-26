//! Synthetic plans, manifests and injected failures for application behavior tests.

use crp_workspace::lockfile::InstallationGraph;
use crp_workspace::metadata::{ExactDependency, VersionTarget};
use serde_json::json;

use super::super::*;
use crate::plan::PlanIncrement;

/// Distinguishes injected acquisition/application failures from successful empty work.
#[ohno::error]
pub(crate) struct ApplicationFailure;

pub(crate) fn failed_manifest_application(failure: Result<String, AppError>) -> AppError {
    let originals = manifests();
    let mut failure = Some(failure);
    let mut reads = Vec::new();
    let paths = unique_paths();
    let last = paths.last().unwrap();
    let error = apply_plan(
        &plan(PlanStage::Proposed, false),
        false,
        Verbose::new(false, &crp_diag::Discard),
        |_, _| panic!(),
        || Ok(work_tree()),
        |path| {
            reads.push(path.to_path_buf());
            if path == last {
                failure.take().unwrap()
            } else {
                Ok(originals.get(path).unwrap().clone())
            }
        },
        |_| panic!(),
    )
    .unwrap_err();
    assert_eq!(reads, paths);
    error
}

pub(crate) fn plan(stage: PlanStage, captured: bool) -> PlanFile {
    let mut plan = PlanFile::new(
        stage,
        ["root", "api"]
            .into_iter()
            .map(|name| PlanIncrement {
                name: name.to_owned(),
                level: None,
                version: Some("0.2.0".to_owned()),
            })
            .collect(),
    );
    if captured {
        // Dispatch forwards capture contents unchanged; the captured-state validator owns them.
        plan.resolved = Some(
            serde_json::from_value(json!({
                "inputs": {
                    "root": "workspace", "manifest": "Cargo.toml",
                    "head": "head", "base": "base", "base_revision": "main",
                    "index": "", "paths": [], "digest": "inputs"
                },
                "files": [], "final_digest": "candidate", "versions": {},
                "evidence_manifest_path": "candidate/Cargo.toml"
            }))
            .unwrap(),
        );
    }
    plan
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

pub(crate) fn manifest_only_summary(dry_run: bool) -> String {
    if dry_run {
        let mut summary = "Dry run: 3 manifests would change".to_owned();
        for path in &unique_paths()[..3] {
            write!(summary, "\n  {}", quote_path(&path.to_string_lossy())).unwrap();
        }
        summary.push_str("; the workspace lockfile would be left untouched");
        summary
    } else {
        "Updated 3 manifests and left the workspace lockfile untouched; use prepare and preview for a resolved release plan".to_owned()
    }
}
