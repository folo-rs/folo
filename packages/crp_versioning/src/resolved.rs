// Captured repository inputs and the exact resolved manifest/lockfile writes.

#![allow(
    clippy::self_named_module_files,
    reason = "The subject module owns captured state; child modules own path logic and test matrices."
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, ErrorKind};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use crp_diag::Verbose;
use crp_workspace::command::{hash_bytes, run_capture};
use crp_workspace::git::GitRepo;
use crp_workspace::manifest::{PathCase, for_each_dependency_table, parse_document};
use crp_workspace::metadata::{WorkTree, load_tracked_work_tree};
use ohno::AppError;
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, TableLike};

use self::paths::PathIdentity;
use crate::groups::Groups;
use crate::history::AssessmentHistory;
use crate::plan::{PlanFile, PlanStage, SCHEMA_VERSION, resolve_plan};
use crate::{ParsePlanError, ReadFileError, UnsupportedPlanSchemaError, WriteFileError};

mod paths;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod fingerprint_tests;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod candidate_tests;

/// Repository facts frozen before any resolver-driven release decisions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Inputs {
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub head: String,
    pub release_history: String,
    pub release_history_revision: String,
    /// A distinct anticipated predecessor; targets already in release history normalize to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_target: Option<String>,
    /// Original target ref, retained even when it initially resolved within release history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_target_revision: Option<String>,
    pub index: String,
    pub paths: BTreeSet<PathBuf>,
    pub digest: String,
}

impl Inputs {
    pub fn capture(manifest: &Path, release_history: Option<&str>) -> Result<Self, AppError> {
        Self::capture_with_target(manifest, release_history, None)
    }

    pub fn capture_with_target(
        manifest: &Path,
        release_history: Option<&str>,
        merge_target: Option<&str>,
    ) -> Result<Self, AppError> {
        Self::capture_workspace(manifest, release_history, merge_target).map(|(inputs, _)| inputs)
    }

    fn capture_workspace(
        manifest: &Path,
        release_history: Option<&str>,
        merge_target: Option<&str>,
    ) -> Result<(Self, WorkTree), AppError> {
        // Cargo preserves the supplied path spelling, including Windows short names.
        // Normalize the entry point before discovering any paths that will be rebased.
        let manifest = canonical(manifest)?;
        let (work_tree, git) = load_tracked_work_tree(&manifest)?;
        let root = canonical(git.root())?;
        let manifest = relative(&root, &manifest)?;
        let history = AssessmentHistory::resolve(
            &git,
            release_history,
            merge_target,
            Verbose::new(false, &crp_diag::Discard),
        )?;
        let mut paths: BTreeSet<PathBuf> =
            work_tree.tracked_paths.iter().map(PathBuf::from).collect();
        paths.insert(relative(
            &root,
            &work_tree.workspace_root.join("Cargo.lock"),
        )?);
        for directory in work_tree
            .workspace_root
            .ancestors()
            .take_while(|directory| directory.starts_with(&root))
        {
            paths.insert(relative(&root, &directory.join(".cargo/config"))?);
            paths.insert(relative(&root, &directory.join(".cargo/config.toml"))?);
        }
        for manifest in &work_tree.member_manifests {
            paths.insert(relative(&root, manifest)?);
            let directory = manifest
                .parent()
                .expect("a manifest has a parent directory");
            collect_sources(&root, &directory.join("src"), &mut paths)?;
            paths.insert(relative(&root, &directory.join("build.rs"))?);
        }
        capture_path_dependencies_documents(
            &root,
            work_tree
                .member_manifests
                .iter()
                .chain([&work_tree.workspace_root.join("Cargo.toml")]),
            &mut paths,
            |path| match work_tree.manifests.documents.get(path) {
                Some(document) => Ok(document.clone()),
                None => {
                    let text = fs::read_to_string(path)
                        .map_err(|error| ReadFileError::caused_by(path, error))?;
                    parse_document(path, &text)
                }
            },
            canonical,
            |directory, paths| collect_sources(&root, directory, paths),
        )?;
        let digest = fingerprint(&root, &paths, &BTreeMap::new())?;
        history.verify(&git)?;
        Ok((
            Self {
                root,
                manifest,
                head: git.head()?,
                release_history: history.release_history,
                release_history_revision: history.release_history_revision,
                merge_target: history.merge_target,
                merge_target_revision: history.merge_target_revision,
                index: run_capture("git", &["ls-files", "--stage", "-z"], git.root())?,
                paths,
                digest,
            },
            work_tree,
        ))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn index(&self) -> &str {
        &self.index
    }

    /// Verifies the same captured input set in a relocated final workspace.
    // Connects captured-input acquisition to the unit-tested verification protocol.
    #[cfg_attr(test, mutants::skip)]
    pub fn verify_candidate(&self, manifest: &Path, final_digest: &str) -> Result<(), AppError> {
        self.verify_candidate_with(
            manifest,
            final_digest,
            Self::capture_with_target,
            |current, digest| self.compare_candidate(current, digest),
        )?;
        self.verify_history()
    }

    /// Rechecks the originally captured history refs without reacquiring workspace content.
    ///
    /// Retained candidates use frozen commits for their content checks, but must still reject
    /// movement of the caller's original release-history or merge-target ref.
    // Native Git verification only; verify_history_with tests the original identity handoff.
    #[cfg_attr(test, mutants::skip)]
    pub fn verify_history(&self) -> Result<(), AppError> {
        self.verify_history_with(AssessmentHistory::verify)
    }

    fn verify_history_with(
        &self,
        verify: impl FnOnce(&AssessmentHistory, &GitRepo) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        self.validate_history_fields()?;
        let history = AssessmentHistory {
            release_history: self.release_history.clone(),
            release_history_revision: self.release_history_revision.clone(),
            merge_target: self.merge_target.clone(),
            merge_target_revision: self.merge_target_revision.clone(),
        };
        verify(
            &history,
            &GitRepo {
                root: self.root.clone(),
                prefix: String::new(),
            },
        )
        .map_err(StaleInputs::caused_by)?;
        Ok(())
    }

    fn verify_candidate_with(
        &self,
        manifest: &Path,
        final_digest: &str,
        capture: impl FnOnce(&Path, Option<&str>, Option<&str>) -> Result<Self, AppError>,
        compare: impl FnOnce(&Self, &str) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        self.validate_history_fields()?;
        let current = capture(
            manifest,
            Some(&self.release_history),
            self.merge_target.as_deref(),
        )
        .map_err(StaleInputs::caused_by)?;
        compare(&current, final_digest)
    }

    // Native path-identity and fingerprint acquisition; the comparison protocol has pure tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn compare_candidate(&self, current: &Self, final_digest: &str) -> Result<(), AppError> {
        self.compare_candidate_with(
            current,
            final_digest,
            &PathIdentity::new(&current.root, &PathCase::probe),
            || fingerprint(&current.root, &self.paths, &BTreeMap::new()),
        )
    }

    fn compare_candidate_with(
        &self,
        current: &Self,
        final_digest: &str,
        identity: &PathIdentity<'_>,
        fingerprint: impl FnOnce() -> Result<String, AppError>,
    ) -> Result<(), AppError> {
        if current.head != self.head || !self.same_history(current) || current.index != self.index {
            return Err(StaleInputs::new().into());
        }
        if current.manifest != self.manifest || current.paths != self.paths {
            if !identity.same(&self.manifest, &current.manifest)
                || !identity.same_set(&self.paths, &current.paths)
                || fingerprint()? != final_digest
            {
                return Err(StaleInputs::new().into());
            }
        } else if current.digest != final_digest {
            return Err(StaleInputs::new().into());
        }
        Ok(())
    }

    /// Accepts only the complete initial state or the complete captured final state.
    // Live acquisition is native; verify_with preserves the captured refs and comparison result.
    #[cfg_attr(test, mutants::skip)]
    pub fn verify(&self, manifest: &Path, final_digest: Option<&str>) -> Result<bool, AppError> {
        self.verify_with(manifest, final_digest, Self::capture_with_target)
    }

    /// Returns observations only after a fresh capture passes source admission.
    #[cfg_attr(test, mutants::skip)] // Native capture; verification policy remains in verify_with.
    pub(crate) fn verify_workspace(
        &self,
        manifest: &Path,
        final_digest: Option<&str>,
    ) -> Result<(bool, WorkTree), AppError> {
        self.verify_observed(manifest, final_digest, Self::capture_workspace)
    }

    fn verify_with(
        &self,
        manifest: &Path,
        final_digest: Option<&str>,
        capture: impl FnOnce(&Path, Option<&str>, Option<&str>) -> Result<Self, AppError>,
    ) -> Result<bool, AppError> {
        self.verify_observed(manifest, final_digest, |manifest, history, target| {
            capture(manifest, history, target).map(|inputs| (inputs, ()))
        })
        .map(|(applied, ())| applied)
    }

    fn verify_observed<T>(
        &self,
        manifest: &Path,
        final_digest: Option<&str>,
        capture: impl FnOnce(&Path, Option<&str>, Option<&str>) -> Result<(Self, T), AppError>,
    ) -> Result<(bool, T), AppError> {
        self.validate_history_fields()?;
        let (current, observations) = capture(
            manifest,
            Some(&self.release_history_revision),
            self.merge_target_revision.as_deref(),
        )
        .map_err(StaleInputs::caused_by)?;
        Ok((self.compare(&current, final_digest)?, observations))
    }

    pub fn compare(&self, current: &Self, final_digest: Option<&str>) -> Result<bool, AppError> {
        if current.root != self.root
            || current.manifest != self.manifest
            || current.head != self.head
            || !self.same_history(current)
            || current.index != self.index
            || current.paths != self.paths
        {
            return Err(StaleInputs::new().into());
        }
        if current.digest == self.digest {
            return Ok(false);
        }
        if final_digest == Some(current.digest.as_str()) {
            return Ok(true);
        }
        Err(StaleInputs::new().into())
    }

    pub(crate) fn same_history(&self, other: &Self) -> bool {
        self.release_history == other.release_history && self.merge_target == other.merge_target
    }

    fn validate_history_fields(&self) -> Result<(), AppError> {
        if self.merge_target.is_some() && self.merge_target_revision.is_none() {
            return Err(StaleInputs::new().into());
        }
        Ok(())
    }

    // Native identity/fingerprint adapter; final_digest_with tests the exact replacement handoff.
    #[cfg_attr(test, mutants::skip)]
    pub fn final_digest(&self, files: &[Artifact]) -> Result<String, AppError> {
        let identity = PathIdentity::new(&self.root, &PathCase::probe);
        self.final_digest_with(files, &identity, |replacements| {
            fingerprint(&self.root, &self.paths, replacements)
        })
    }

    fn final_digest_with(
        &self,
        files: &[Artifact],
        identity: &PathIdentity<'_>,
        fingerprint: impl FnOnce(&BTreeMap<PathBuf, Vec<u8>>) -> Result<String, AppError>,
    ) -> Result<String, AppError> {
        let replacements = self.artifact_replacements(files, identity)?;
        fingerprint(&replacements)
    }

    fn artifact_replacements(
        &self,
        files: &[Artifact],
        identity: &PathIdentity<'_>,
    ) -> Result<BTreeMap<PathBuf, Vec<u8>>, AppError> {
        let mut seen = BTreeSet::new();
        for file in files {
            if !identity.supports_artifact(&file.path)
                || !identity.contains(&self.paths, &file.path)
                || identity.contains(&seen, &file.path)
            {
                return Err(ResolutionRequired::new().into());
            }
            seen.insert(file.path.clone());
        }
        Ok(files
            .iter()
            .map(|file| (file.path.clone(), file.contents.as_bytes().to_vec()))
            .collect())
    }
}

// Native file/identity/source acquisition; the graph traversal is tested with captured observations.
#[cfg_attr(test, mutants::skip)]
pub fn capture_path_dependencies<'a>(
    root: &Path,
    manifests: impl IntoIterator<Item = &'a PathBuf>,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), AppError> {
    capture_path_dependencies_with(
        root,
        manifests,
        paths,
        |path| {
            fs::read_to_string(path).map_err(|error| ReadFileError::caused_by(path, error).into())
        },
        canonical,
        |directory, paths| collect_sources(root, directory, paths),
    )
}

fn capture_path_dependencies_with<'a>(
    root: &Path,
    manifests: impl IntoIterator<Item = &'a PathBuf>,
    paths: &mut BTreeSet<PathBuf>,
    mut read: impl FnMut(&Path) -> Result<String, AppError>,
    canonicalize: impl FnMut(&Path) -> Result<PathBuf, AppError>,
    collect: impl FnMut(&Path, &mut BTreeSet<PathBuf>) -> Result<(), AppError>,
) -> Result<(), AppError> {
    capture_path_dependencies_documents(
        root,
        manifests,
        paths,
        |path| parse_document(path, &read(path)?),
        canonicalize,
        collect,
    )
}

fn capture_path_dependencies_documents<'a>(
    root: &Path,
    manifests: impl IntoIterator<Item = &'a PathBuf>,
    paths: &mut BTreeSet<PathBuf>,
    mut document: impl FnMut(&Path) -> Result<DocumentMut, AppError>,
    mut canonicalize: impl FnMut(&Path) -> Result<PathBuf, AppError>,
    mut collect: impl FnMut(&Path, &mut BTreeSet<PathBuf>) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let mut pending: BTreeSet<PathBuf> = manifests.into_iter().cloned().collect();
    let mut visited = BTreeSet::new();
    while let Some(manifest) = pending.pop_first() {
        if !visited.insert(manifest.clone()) {
            continue;
        }
        paths.insert(relative(root, &manifest)?);
        let document = document(&manifest)?;
        let mut dependencies = Vec::new();
        for_each_dependency_table(document.as_table(), &mut |_, table| {
            dependency_paths(table, &mut dependencies);
        });
        if let Some(workspace) = document.get("workspace").and_then(Item::as_table_like) {
            for_each_dependency_table(workspace, &mut |_, table| {
                dependency_paths(table, &mut dependencies);
            });
        }
        if let Some(patches) = document.get("patch").and_then(Item::as_table_like) {
            for (_, patch) in patches.iter() {
                if let Some(table) = patch.as_table_like() {
                    dependency_paths(table, &mut dependencies);
                }
            }
        }
        if let Some(replacements) = document.get("replace").and_then(Item::as_table_like) {
            dependency_paths(replacements, &mut dependencies);
        }
        for dependency in dependencies {
            let path = Path::new(&dependency);
            // Absolute paths would keep pointing into the live workspace from a disposable clone.
            if path.is_absolute() {
                return Err(UnsupportedInput::new(path).into());
            }
            let directory = manifest
                .parent()
                .expect("a manifest has a parent")
                .join(path);
            let directory = canonicalize(&directory)?;
            relative(root, &directory)?;
            collect(&directory.join("src"), paths)?;
            pending.insert(directory.join("Cargo.toml"));
        }
    }
    Ok(())
}

fn dependency_paths(table: &dyn TableLike, paths: &mut Vec<String>) {
    for (_, dependency) in table.iter() {
        if let Some(path) = dependency
            .as_table_like()
            .and_then(|dependency| dependency.get("path"))
            .and_then(Item::as_str)
        {
            paths.push(path.to_owned());
        }
    }
}

/// The complete resolved state embedded in the explicit plan for application.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolvedState {
    pub inputs: Inputs,
    pub files: Vec<Artifact>,
    pub final_digest: String,
    pub versions: BTreeMap<String, String>,
    pub evidence_manifest_path: PathBuf,
}

impl ResolvedState {
    // Native identity/captured-input acquisition; validate_candidate_location owns isolation policy.
    #[cfg_attr(test, mutants::skip)]
    pub fn verify_candidate(&self, manifest: &Path) -> Result<(), AppError> {
        let manifest = canonical(manifest)?;
        Self::validate_candidate_location(
            &manifest,
            &canonical(&self.evidence_manifest_path)?,
            &canonical(&self.inputs.root.join(&self.inputs.manifest))?,
        )?;
        self.inputs.verify_candidate(&manifest, &self.final_digest)
    }

    fn validate_candidate_location(
        actual: &Path,
        evidence: &Path,
        live: &Path,
    ) -> Result<(), AppError> {
        if actual != evidence || actual == live {
            return Err(WrongEvidenceWorkspace::new().into());
        }
        Ok(())
    }

    // Native identity/fingerprint acquisition; the injected validator retains every admission check.
    #[cfg_attr(test, mutants::skip)]
    pub fn validate_artifacts(
        &self,
        versions: &BTreeMap<String, String>,
        allowed: &BTreeSet<PathBuf>,
    ) -> Result<(), AppError> {
        self.validate_artifacts_with(
            versions,
            allowed,
            &PathIdentity::new(self.inputs.root(), &PathCase::probe),
            || self.inputs.final_digest(&self.files),
        )
    }

    fn validate_artifacts_with(
        &self,
        versions: &BTreeMap<String, String>,
        allowed: &BTreeSet<PathBuf>,
        identity: &PathIdentity<'_>,
        final_digest: impl FnOnce() -> Result<String, AppError>,
    ) -> Result<(), AppError> {
        if *versions != self.versions {
            return Err(ResolutionRequired::new().into());
        }
        for file in &self.files {
            if !identity.supports_artifact(&file.path) || !identity.contains(allowed, &file.path) {
                return Err(ResolutionRequired::new().into());
            }
        }
        // final_digest owns captured-path membership and uniqueness; this layer additionally
        // restricts writes to the current workspace's member manifests and lockfile.
        if final_digest()? != self.final_digest {
            return Err(ResolutionRequired::new().into());
        }
        Ok(())
    }
}

/// Exact UTF-8 bytes of one resolved manifest or workspace lockfile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Artifact {
    pub path: PathBuf,
    pub contents: String,
}

/// The protocol header is checked before parsing a version-specific artifact body.
#[derive(Deserialize)]
struct ArtifactSchema {
    schema_version: u32,
}

// Filesystem acquisition and verification adapters are covered by the retained-workspace
// integration tests; the command's ordering, errors and output are tested in process below.
#[cfg_attr(test, mutants::skip)]
pub fn run_verify_preview(
    plan_path: &Path,
    manifest: &Path,
    verbose: Verbose<'_>,
) -> Result<String, AppError> {
    let plan: PlanFile = read_json(plan_path)?;
    verify_preview(
        &plan,
        |state| {
            apply_resolved(
                &plan,
                &state.inputs.root.join(&state.inputs.manifest),
                true,
                verbose,
            )
            .map(|_| ())
        },
        |state| state.verify_candidate(manifest),
    )
}

fn verify_preview(
    plan: &PlanFile,
    validate_live: impl FnOnce(&ResolvedState) -> Result<(), AppError>,
    verify_candidate: impl FnOnce(&ResolvedState) -> Result<(), AppError>,
) -> Result<String, AppError> {
    let state = plan.resolved.as_ref().ok_or_else(ResolutionRequired::new)?;
    validate_live(state)?;
    verify_candidate(state)?;
    Ok("Compatibility workspace matches the captured source, versions, and lockfile.".to_owned())
}

pub(crate) fn apply_resolved(
    plan: &PlanFile,
    manifest: &Path,
    dry_run: bool,
    verbose: Verbose<'_>,
) -> Result<String, AppError> {
    let state = plan.resolved.as_ref().ok_or_else(ResolutionRequired::new)?;
    if plan.schema_version != SCHEMA_VERSION || plan.stage() != PlanStage::Expanded {
        return Err(ResolutionRequired::new().into());
    }
    plan.validate_history(&state.inputs)?;
    let manifest = canonical(manifest)?;
    let (already_applied, work_tree) = state
        .inputs
        .verify_workspace(&manifest, Some(&state.final_digest))?;
    let resolved = resolve_plan(
        plan,
        &Groups::from_workspace(&work_tree),
        &work_tree.target_versions(),
        verbose,
    )?;
    let versions: BTreeMap<String, String> = resolved
        .packages
        .iter()
        .map(|(name, version)| (name.clone(), version.to_string()))
        .collect();
    let allowed: BTreeSet<PathBuf> = work_tree
        .member_manifests
        .iter()
        .chain([&work_tree.workspace_root.join("Cargo.toml")])
        .chain([&work_tree.workspace_root.join("Cargo.lock")])
        .map(|path| relative(state.inputs.root(), path))
        .collect::<Result<_, _>>()?;
    state.validate_artifacts(&versions, &allowed)?;
    if already_applied {
        return Ok("Resolved state is already applied; no files changed.".to_owned());
    }
    if dry_run {
        return Ok(format!(
            "Dry run: would install {} captured files; no Cargo resolution is performed.",
            state.files.len()
        ));
    }
    for file in &state.files {
        let path = state.inputs.root().join(&file.path);
        fs::write(&path, &file.contents)
            .map_err(|error| WriteFileError::caused_by(&path, error))?;
    }
    state.inputs.verify(&manifest, Some(&state.final_digest))?;
    Ok(format!(
        "Installed {} captured files without Cargo resolution.",
        state.files.len()
    ))
}

pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, AppError> {
    let text = fs::read_to_string(path).map_err(|error| ReadFileError::caused_by(path, error))?;
    parse_artifact(path, &text)
}

fn parse_artifact<T: for<'de> Deserialize<'de>>(path: &Path, text: &str) -> Result<T, AppError> {
    // A UTF-8 byte-order mark is encoding metadata, not part of the JSON document.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let schema: ArtifactSchema =
        serde_json::from_str(text).map_err(|error| ParsePlanError::caused_by(path, error))?;
    if schema.schema_version != SCHEMA_VERSION {
        return Err(UnsupportedPlanSchemaError::new(schema.schema_version).into());
    }
    serde_json::from_str(text).map_err(|error| ParsePlanError::caused_by(path, error).into())
}

// Native file write only; serialization, emitted bytes and write errors are tested with a sink.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), AppError> {
    write_json_with(path, value, |path, bytes| fs::write(path, bytes))
}

fn write_json_with(
    path: &Path,
    value: &impl Serialize,
    write: impl FnOnce(&Path, &[u8]) -> io::Result<()>,
) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(value)
        .expect("release artifacts contain only JSON-compatible data");
    write(path, format!("{json}\n").as_bytes())
        .map_err(|error| WriteFileError::caused_by(path, error).into())
}

pub fn relative(root: &Path, path: &Path) -> Result<PathBuf, AppError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| UnsupportedInput::caused_by(path, error))?;
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(UnsupportedInput::new(path).into());
    }
    Ok(relative.to_path_buf())
}

// Native canonicalization is integration-only; Windows spelling normalization has pure tests.
#[cfg_attr(test, mutants::skip)]
pub fn canonical(path: &Path) -> Result<PathBuf, AppError> {
    let canonical =
        fs::canonicalize(path).map_err(|error| ReadFileError::caused_by(path, error))?;
    // Git and Cargo report ordinary Windows paths, not canonicalize's verbatim spelling.
    #[cfg(windows)]
    let canonical = ordinary_windows_path(&canonical);
    Ok(canonical)
}

// String conversion is platform-independent; compiling it on every test host avoids
// uncompiled Windows-only mutants without making a filesystem case-sensitivity assumption.
#[cfg(any(windows, test))]
fn ordinary_windows_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(path) => PathBuf::from(format!(r"\\{path}")),
        None => PathBuf::from(text.trim_start_matches(r"\\?\")),
    }
}

// Directory entries require native acquisition; collect_sources_with tests recursive selection.
#[cfg_attr(test, mutants::skip)]
pub fn collect_sources(
    root: &Path,
    directory: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), AppError> {
    collect_sources_with(root, directory, paths, &mut read_source_directory)
}

// Filesystem existence and entry/type observations are covered by source-capture integration tests.
#[cfg_attr(test, mutants::skip)]
fn read_source_directory(directory: &Path) -> Result<Option<Vec<(PathBuf, bool)>>, AppError> {
    if !directory.exists() {
        return Ok(None);
    }
    let mut entries = Vec::new();
    for entry in
        fs::read_dir(directory).map_err(|error| ReadFileError::caused_by(directory, error))?
    {
        let entry = entry.map_err(|error| ReadFileError::caused_by(directory, error))?;
        let kind = entry
            .file_type()
            .map_err(|error| ReadFileError::caused_by(entry.path(), error))?;
        entries.push((entry.path(), kind.is_dir()));
    }
    Ok(Some(entries))
}

fn collect_sources_with(
    root: &Path,
    directory: &Path,
    paths: &mut BTreeSet<PathBuf>,
    read: &mut impl FnMut(&Path) -> Result<Option<Vec<(PathBuf, bool)>>, AppError>,
) -> Result<(), AppError> {
    let Some(entries) = read(directory)? else {
        return Ok(());
    };
    for (path, directory) in entries {
        if directory {
            collect_sources_with(root, &path, paths, read)?;
        } else {
            paths.insert(relative(root, &path)?);
        }
    }
    Ok(())
}

// Real metadata, byte reads and Git hashing are integration boundaries. The encoder below
// owns missing/error discrimination, file admission, replacement precedence and byte framing.
#[cfg_attr(test, mutants::skip)]
pub fn fingerprint(
    root: &Path,
    paths: &BTreeSet<PathBuf>,
    replacements: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<String, AppError> {
    let identity = PathIdentity::new(root, &PathCase::probe);
    let bytes = fingerprint_bytes(
        root,
        paths,
        replacements,
        &identity,
        |path| {
            fs::symlink_metadata(path).map(|metadata| InputMetadata {
                regular: metadata.is_file(),
                symlink: metadata.file_type().is_symlink(),
                #[cfg(unix)]
                executable: executable_mode(metadata.permissions().mode()),
            })
        },
        |path| fs::read(path),
    )?;
    hash_bytes(&bytes, root)
}

/// Filesystem observations needed to admit and encode one captured input.
struct InputMetadata {
    regular: bool,
    symlink: bool,
    #[cfg(unix)]
    executable: bool,
}

// The bit interpretation is portable even though only Unix metadata supplies it.
#[cfg(any(unix, test))]
fn executable_mode(mode: u32) -> bool {
    // Git records whether any execute bit is set, not the other permission bits.
    mode & 0o111 != 0
}

fn fingerprint_bytes(
    root: &Path,
    paths: &BTreeSet<PathBuf>,
    replacements: &BTreeMap<PathBuf, Vec<u8>>,
    identity: &PathIdentity<'_>,
    mut metadata: impl FnMut(&Path) -> io::Result<InputMetadata>,
    mut read: impl FnMut(&Path) -> io::Result<Vec<u8>>,
) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::new();
    for relative in paths {
        let path = root.join(relative);
        let name = relative.to_string_lossy();
        append_field(&mut bytes, name.as_bytes());
        let metadata = match metadata(&path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(ReadFileError::caused_by(&path, error).into()),
        };
        if metadata
            .as_ref()
            .is_some_and(|metadata| !metadata.regular || metadata.symlink)
        {
            return Err(UnsupportedInput::new(&path).into());
        }
        #[cfg(unix)]
        bytes.push(u8::from(
            metadata
                .as_ref()
                .is_some_and(|metadata| metadata.executable),
        ));
        let contents = if let Some(replacement) = identity.replacement(relative, replacements) {
            Some(replacement.to_owned())
        } else if metadata.is_some() {
            Some(read(&path).map_err(|error| ReadFileError::caused_by(&path, error))?)
        } else {
            None
        };
        bytes.push(u8::from(contents.is_some()));
        if let Some(contents) = contents {
            append_field(&mut bytes, &contents);
        }
    }
    Ok(bytes)
}

fn append_field(bytes: &mut Vec<u8>, field: &[u8]) {
    // Artifact fingerprints use a fixed-width length, independent of the executing binary's
    // pointer width. Length-prefixing also distinguishes adjacent fields with the same bytes.
    let length = u64::try_from(field.len())
        .expect("a slice on a supported Rust target cannot exceed u64::MAX bytes");
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(field);
}

/// A stale source tree must return to preparation rather than widen a captured plan.
#[ohno::error]
#[display("prepared release inputs are stale; regenerate with prepare and preview")]
pub(crate) struct StaleInputs;

/// Application requires the complete source and file effects captured by preview.
#[ohno::error]
#[display("apply requires the unchanged resolved plan produced by preview")]
pub(crate) struct ResolutionRequired;

/// Prospective work must remain inside the captured repository.
#[ohno::error]
#[display("cannot capture release input {}; use ordinary files inside the repository", path.display())]
pub(crate) struct UnsupportedInput {
    path: PathBuf,
}

/// Compatibility evidence must come from the recorded final candidate, not the live tree.
#[ohno::error]
#[display("verification requires the evidence_manifest_path recorded in the resolved plan")]
pub(crate) struct WrongEvidenceWorkspace;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {

    use serde_json::json;

    use super::*;

    #[test]
    fn candidate_location_requires_recorded_evidence_and_excludes_live_source() {
        let evidence = Path::new("candidate/Cargo.toml");
        let live = Path::new("source/Cargo.toml");
        ResolvedState::validate_candidate_location(evidence, evidence, live).unwrap();
        for (actual, recorded) in [(Path::new("other/Cargo.toml"), evidence), (live, live)] {
            let error =
                ResolvedState::validate_candidate_location(actual, recorded, live).unwrap_err();
            assert!(error.find_source::<WrongEvidenceWorkspace>().is_some());
        }
    }

    // A real empty file supports Git for Windows on ARM64, unlike the NUL device.
    // Keep it outside fixtures so it cannot enter their captured or committed inputs.

    fn inputs() -> Inputs {
        Inputs {
            root: PathBuf::from("repository"),
            manifest: PathBuf::from("Cargo.toml"),
            head: "head".to_owned(),
            release_history: "base".to_owned(),
            release_history_revision: "main".to_owned(),
            merge_target: None,
            merge_target_revision: None,
            index: "index".to_owned(),
            paths: ["Cargo.toml", "Cargo.lock", "src/lib.rs"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            digest: "initial".to_owned(),
        }
    }

    #[test]
    fn captured_identity_fields_are_required_for_initial_and_final_states() {
        let inputs = Inputs {
            merge_target: Some("parent-final".to_owned()),
            merge_target_revision: Some("parent".to_owned()),
            ..inputs()
        };
        let original = serde_json::to_value(&inputs).unwrap();
        for (field, value) in [
            ("root", json!("another-repository")),
            ("manifest", json!("member/Cargo.toml")),
            ("head", json!("another-head")),
            ("release_history", json!("another-base")),
            ("merge_target", json!("another-parent")),
            ("index", json!("another-index")),
            ("paths", json!(["Cargo.toml"])),
        ] {
            let mut changed = original.clone();
            *changed.get_mut(field).unwrap() = value;
            let current: Inputs = serde_json::from_value(changed).unwrap();
            let error = inputs.compare(&current, Some("final")).unwrap_err();
            assert!(error.find_source::<StaleInputs>().is_some());
            if field != "root" {
                let identity = PathIdentity::new(&current.root, &|_| PathCase::Sensitive);
                let error = inputs
                    .compare_candidate_with(&current, "initial", &identity, || {
                        panic!("different membership must fail before fingerprint acquisition")
                    })
                    .unwrap_err();
                assert!(error.find_source::<StaleInputs>().is_some());
            }
        }
    }

    #[test]
    fn captured_target_cannot_disappear_or_lose_its_revision() {
        let inputs = Inputs {
            merge_target: Some("parent-final".to_owned()),
            merge_target_revision: Some("parent".to_owned()),
            ..inputs()
        };
        let mut altered = inputs.clone();
        altered.merge_target = None;
        let error = inputs.compare(&altered, None).unwrap_err();
        assert!(error.find_source::<StaleInputs>().is_some());
        altered = inputs;
        altered.merge_target_revision = None;
        let error = altered.validate_history_fields().unwrap_err();
        assert!(error.find_source::<StaleInputs>().is_some());
    }

    #[test]
    fn live_inputs_accept_only_initial_or_complete_final_bytes() {
        let inputs = inputs();
        assert!(!inputs.compare(&inputs, None).unwrap());
        assert!(!inputs.compare(&inputs, Some("initial")).unwrap());
        let mut current = inputs.clone();
        current.digest = "final".to_owned();
        assert!(inputs.compare(&current, Some("final")).unwrap());
        for final_digest in [None, Some("different-final")] {
            let error = inputs.compare(&current, final_digest).unwrap_err();
            assert!(error.find_source::<StaleInputs>().is_some());
        }
        // The revision spelling is acquisition input; only its resolved commit is identity.
        current.release_history_revision = "another-ref-to-the-same-commit".to_owned();
        assert!(inputs.compare(&current, Some("final")).unwrap());
        // Final bytes do not bypass the identity guard exercised field-by-field above.
        current.head = "another-head".to_owned();
        let error = inputs.compare(&current, Some("final")).unwrap_err();
        assert!(error.find_source::<StaleInputs>().is_some());
    }

    #[test]
    fn relocated_candidates_require_final_bytes_but_not_live_root_or_revision_spelling() {
        let inputs = inputs();
        let mut candidate = inputs.clone();
        candidate.root = PathBuf::from("retained-workspace");
        candidate
            .release_history_revision
            .clone_from(&inputs.release_history);
        let identity = PathIdentity::new(&candidate.root, &|_| panic!("exact paths need no probe"));
        let error = inputs
            .compare_candidate_with(&candidate, "final", &identity, || panic!())
            .unwrap_err();
        assert!(error.find_source::<StaleInputs>().is_some());
        candidate.digest = "final".to_owned();
        inputs
            .compare_candidate_with(&candidate, "final", &identity, || panic!())
            .unwrap();
    }

    #[test]
    fn resolved_metadata_requires_the_expanded_current_schema_before_acquisition() {
        let state = ResolvedState {
            inputs: inputs(),
            files: Vec::new(),
            final_digest: "final".to_owned(),
            versions: BTreeMap::new(),
            evidence_manifest_path: "candidate/Cargo.toml".into(),
        };
        for (stage, schema) in [
            (PlanStage::Proposed, SCHEMA_VERSION),
            (PlanStage::Expanded, SCHEMA_VERSION + 1),
        ] {
            let mut plan = PlanFile::new(stage, Vec::new());
            plan.schema_version = schema;
            plan.resolved = Some(state.clone());
            let error = apply_resolved(
                &plan,
                Path::new("absent/Cargo.toml"),
                false,
                Verbose::new(false, &crp_diag::Discard),
            )
            .unwrap_err();
            assert!(error.find_source::<ResolutionRequired>().is_some());
        }
    }

    #[test]
    fn uncaptured_plans_are_rejected_before_acquisition_in_both_application_modes() {
        for stage in [PlanStage::Proposed, PlanStage::Expanded] {
            let plan = PlanFile::new(stage, Vec::new());
            for dry_run in [false, true] {
                let error = apply_resolved(
                    &plan,
                    Path::new("absent/Cargo.toml"),
                    dry_run,
                    Verbose::new(false, &crp_diag::Discard),
                )
                .unwrap_err();
                assert!(error.find_source::<ResolutionRequired>().is_some());
            }
        }
    }

    #[test]
    fn fields_use_fixed_width_little_endian_lengths() {
        let mut bytes = Vec::new();
        append_field(&mut bytes, b"abc");
        append_field(&mut bytes, b"");
        assert_eq!(
            bytes,
            [
                3, 0, 0, 0, 0, 0, 0, 0, b'a', b'b', b'c', 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn field_boundaries_participate_in_fingerprints() {
        let mut left = Vec::new();
        append_field(&mut left, b"ab");
        append_field(&mut left, b"c");
        let mut right = Vec::new();
        append_field(&mut right, b"a");
        append_field(&mut right, b"bc");
        assert_ne!(left, right);
    }

    #[test]
    fn canonical_windows_paths_match_git_and_cargo_spellings() {
        for (canonical, ordinary) in [
            (
                r"\\?\UNC\server\share\workspace",
                r"\\server\share\workspace",
            ),
            (r"\\?\C:\workspace", r"C:\workspace"),
            (r"C:\workspace", r"C:\workspace"),
        ] {
            assert_eq!(
                ordinary_windows_path(Path::new(canonical)),
                Path::new(ordinary)
            );
        }
    }

    #[test]
    fn artifact_selection_rejects_uncaptured_non_cargo_and_duplicate_paths() {
        let inputs = Inputs {
            root: PathBuf::from("not-accessed"),
            manifest: PathBuf::from("Cargo.toml"),
            head: String::new(),
            release_history: String::new(),
            release_history_revision: String::new(),
            merge_target: None,
            merge_target_revision: None,
            index: String::new(),
            paths: ["Cargo.toml", "src/lib.rs"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            digest: String::new(),
        };
        for paths in [
            vec!["uncaptured/Cargo.toml"],
            vec!["src/lib.rs"],
            vec!["Cargo.toml", "Cargo.toml"],
        ] {
            let artifacts: Vec<_> = paths
                .into_iter()
                .map(|path| Artifact {
                    path: path.into(),
                    contents: String::new(),
                })
                .collect();
            let identity = PathIdentity::new(&inputs.root, &|_| PathCase::Sensitive);
            let error = inputs
                .artifact_replacements(&artifacts, &identity)
                .unwrap_err();
            assert!(error.find_source::<ResolutionRequired>().is_some());
        }
    }

    #[test]
    fn artifact_replacements_preserve_captured_paths_and_exact_bytes() {
        let inputs = inputs();
        let identity = PathIdentity::new(&inputs.root, &|_| panic!("exact paths need no probe"));
        let files = [
            Artifact {
                path: "Cargo.toml".into(),
                contents: "manifest\n".to_owned(),
            },
            Artifact {
                path: "Cargo.lock".into(),
                contents: String::new(),
            },
        ];
        assert_eq!(
            inputs.artifact_replacements(&files, &identity).unwrap(),
            BTreeMap::from([
                (PathBuf::from("Cargo.toml"), b"manifest\n".to_vec()),
                (PathBuf::from("Cargo.lock"), Vec::new()),
            ])
        );
        assert!(
            inputs
                .artifact_replacements(&[], &identity)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn relative_paths_cannot_escape_the_captured_root() {
        let root = Path::new("repository");
        assert_eq!(
            relative(root, &root.join("member/Cargo.toml")).unwrap(),
            Path::new("member/Cargo.toml")
        );
        for path in [
            PathBuf::from("elsewhere/Cargo.toml"),
            root.join("../Cargo.toml"),
        ] {
            let error = relative(root, &path).unwrap_err();
            assert!(error.find_source::<UnsupportedInput>().is_some());
        }
    }

    #[test]
    fn unsupported_artifact_schema_precedes_body_validation() {
        for schema_version in [SCHEMA_VERSION - 1, SCHEMA_VERSION + 1] {
            let artifact = serde_json::json!({"schema_version": schema_version});
            let error =
                parse_artifact::<PlanFile>(Path::new("prepared.json"), &artifact.to_string())
                    .unwrap_err();
            assert!(error.find_source::<UnsupportedPlanSchemaError>().is_some());
        }
        let current = serde_json::json!({"schema_version": SCHEMA_VERSION});
        let error = parse_artifact::<PlanFile>(Path::new("prepared.json"), &current.to_string())
            .unwrap_err();
        assert!(error.find_source::<ParsePlanError>().is_some());
    }
}
