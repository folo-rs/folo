use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use ohno::AppError;
use serde::Deserialize;

use crate::action::errors::{InvalidInput, InvalidOutput};
use crate::action::port::Host;
use crate::action::preparation::inputs::package_name;

/// Cargo's workspace members and reverse dependency graph used for benchmark selection.
///
/// All path dependency kinds and target conditions participate: an excluded or non-benchmark
/// package can still affect a benchmarked dependent on another collection platform.
pub(crate) struct Workspace {
    pub(crate) root: PathBuf,
    directories: BTreeMap<PathBuf, String>,
    benchmarks: BTreeSet<String>,
    dependents: BTreeMap<String, BTreeSet<String>>,
}

impl Workspace {
    /// Resolves Cargo-reported member/path identities through the filesystem abstraction.
    pub(crate) fn parse(json: &str, host: &impl Host) -> Result<Self, AppError> {
        let metadata: Metadata = serde_json::from_str(json)
            .map_err(|error| InvalidOutput::caused_by("invalid Cargo metadata", error))?;
        let root = host.directory(&metadata.workspace_root)?;
        let member_ids: BTreeSet<_> = metadata.workspace_members.iter().collect();
        let members: Vec<_> = metadata
            .packages
            .iter()
            .filter(|package| member_ids.contains(&package.id))
            .collect();
        if members.len() != member_ids.len() {
            return Err(InvalidOutput::new("Cargo workspace members lack package metadata").into());
        }
        let mut directories = BTreeMap::new();
        let mut dependents = BTreeMap::new();
        let mut benchmarks = BTreeSet::new();
        for package in &members {
            let directory = package
                .manifest_path
                .parent()
                .ok_or_else(|| InvalidOutput::new("package manifest lacks a parent directory"))?;
            if !package_name(&package.name)
                || dependents
                    .insert(package.name.clone(), BTreeSet::new())
                    .is_some()
                || directories
                    .insert(host.directory(directory)?, package.name.clone())
                    .is_some()
            {
                return Err(InvalidOutput::new("ambiguous Cargo package identity").into());
            }
            if package
                .targets
                .iter()
                .any(|target| target.kind.iter().any(|kind| kind == "bench"))
            {
                benchmarks.insert(package.name.clone());
            }
        }
        for package in members {
            for dependency in &package.dependencies {
                if let Some(path) = &dependency.path
                    && let Some(name) = directories.get(&host.directory(path)?)
                {
                    dependents
                        .get_mut(name)
                        .expect("every member directory has a matching dependency entry")
                        .insert(package.name.clone());
                }
            }
        }
        Ok(Self {
            root,
            directories,
            benchmarks,
            dependents,
        })
    }

    /// Keeps ownership within Cargo's declared members rather than nested fixture workspaces.
    pub(crate) fn package_directory(&self, path: &Path) -> Option<&Path> {
        self.directories
            .keys()
            .filter(|directory| path.starts_with(directory))
            .max_by_key(|directory| directory.components().count())
            .map(PathBuf::as_path)
    }

    /// Expands optional affected owners before benchmark filtering and final exclusions.
    pub(crate) fn select(
        &self,
        affected: Option<BTreeSet<String>>,
        excluded: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, AppError> {
        for name in excluded.iter().chain(affected.iter().flatten()) {
            if !self.dependents.contains_key(name) {
                return Err(InvalidInput::new(
                    "packages/exclude",
                    format!("package {name} is not a Cargo workspace member"),
                )
                .into());
            }
        }
        let mut selected = affected.unwrap_or_else(|| self.dependents.keys().cloned().collect());
        let mut pending: VecDeque<_> = selected.iter().cloned().collect();
        while let Some(name) = pending.pop_front() {
            for dependent in self
                .dependents
                .get(&name)
                .expect("selected packages were checked against workspace membership")
            {
                let previous_count = selected.len();
                if selected.insert(dependent.clone()) {
                    debug_assert!(
                        selected.len() > previous_count,
                        "queued packages must expand the selected scope"
                    );
                    pending.push_back(dependent.clone());
                }
            }
        }
        Ok(selected
            .intersection(&self.benchmarks)
            .filter(|name| !excluded.contains(*name))
            .cloned()
            .collect())
    }
}

/// The subset of machine-readable Cargo metadata needed for workspace scope policy.
#[derive(Deserialize)]
struct Metadata {
    workspace_root: PathBuf,
    workspace_members: Vec<String>,
    packages: Vec<Package>,
}

/// A Cargo member's identity, benchmark targets and path dependency declarations.
#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
    targets: Vec<Target>,
    dependencies: Vec<Dependency>,
}

/// Explicit benchmark targets establish eligibility independently of feature selection.
#[derive(Deserialize)]
struct Target {
    kind: Vec<String>,
}

/// Path identity distinguishes workspace dependencies from similarly named registry packages.
#[derive(Deserialize)]
struct Dependency {
    path: Option<PathBuf>,
}
