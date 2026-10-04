// Explicit offline preparation and proposal-specific fixed-point resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf, absolute};

use crp_diag::Verbose;
use crp_workspace::artifact_path::{resolve_path, same_path};
use crp_workspace::cache::{Cache, CacheOptions};
use crp_workspace::command::hash_bytes;
use crp_workspace::manifest::requirement_names_version;
use crp_workspace::metadata::{WorkTree, load_tracked_work_tree};
use ohno::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::apply::{ManifestEdit, compute_edits};
use crate::check::{check_classification, releases_breaking_change};
use crate::classify::{
    ChangedItem, PackageClass, PackageStatus, SnapshotCache, classify_with_cache,
};
use crate::groups::{GroupVerdict, Groups};
use crate::plan::{
    PlanFile, PlanIncrement, PlanStage, ResolvedVersions, SCHEMA_VERSION, VersionBump,
    increment_version, resolve_plan,
};
use crate::prospective::Prospective;
use crate::report::write_report;
use crate::resolved::{
    Artifact, Inputs, ResolvedState, StaleInputs, canonical, read_json, write_json,
};
use crate::{CheckFormat, WriteFileError};

/// Post-refresh workspace inputs captured before semantic assessment.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Prepared {
    pub schema_version: u32,
    pub inputs: Inputs,
}

// Preparation owns real clone/resolver lifetimes and publication of captured files. Its
// end-to-end side effects and output belong in integration tests; file admission remains
// unit-tested by validate_preparation_files. See
// packages/cargo-release-plan/docs/implementation.md, "Test boundaries".
/// Prepares evidence while freezing actual history and the anticipated predecessor separately.
#[cfg_attr(test, mutants::skip)] // Real source/resolution acquisition belongs to boundary tests.
pub fn run_prepare_with_target(
    output: &Path,
    release_history: Option<&str>,
    merge_target: Option<&str>,
    manifest: &Path,
    verbose: Verbose<'_>,
) -> Result<String, AppError> {
    run_prepare_with_cache(
        output,
        release_history,
        merge_target,
        manifest,
        verbose,
        Cache::resolve(manifest, &CacheOptions::Default, verbose)?,
    )
}

#[cfg_attr(test, mutants::skip)] // Native source capture, resolution and evidence publication.
pub fn run_prepare_with_cache(
    output: &Path,
    release_history: Option<&str>,
    merge_target: Option<&str>,
    manifest: &Path,
    verbose: Verbose<'_>,
    cache: Cache,
) -> Result<String, AppError> {
    cache.protect(output)?;
    let output = absolute(output).map_err(|error| WriteFileError::caused_by(output, error))?;
    let manifest = canonical(manifest)?;
    let inputs = Inputs::capture_with_target(&manifest, release_history, merge_target)?;
    let prospective = Prospective::new(&output, &inputs)?;
    remove_marker(&output.join("prepared.json"))?;
    prospective.resolve(verbose)?;
    let files = prospective.artifacts(&inputs)?;
    inputs.verify(&manifest, None)?;
    let (work_tree, _) = load_tracked_work_tree(&manifest)?;
    let lockfile = work_tree.workspace_root.join("Cargo.lock");
    validate_preparation_files(inputs.root(), &lockfile, &files)?;
    // Preparation is the explicit mutation boundary. Install only the successfully resolved
    // lockfile before capturing evidence so semantic checks run against this same live state.
    for file in files {
        fs::write(&lockfile, file.contents)
            .map_err(|error| WriteFileError::caused_by(&lockfile, error))?;
    }
    let refreshed = Inputs::capture_with_target(&manifest, release_history, merge_target)?;
    if !inputs.same_history(&refreshed) {
        return Err(StaleInputs::new().into());
    }
    let inputs = refreshed;
    let classification = classify_with_cache(
        &manifest,
        Some(&inputs.release_history),
        inputs.merge_target.as_deref(),
        verbose,
        &mut SnapshotCache::new(cache),
    )?;
    inputs.verify(&manifest, None)?;
    write_report(&output, &classification)?;
    write_json(
        &output.join("prepared.json"),
        &Prepared {
            schema_version: SCHEMA_VERSION,
            inputs,
        },
    )?;
    Ok(format!(
        "Refreshed the workspace lockfile offline and prepared release evidence in {}",
        output.display()
    ))
}

// This adapter coordinates real Git/Cargo workspaces and artifact publication. Integration
// tests verify the command and output; unit-tested cores retain rewriting, convergence,
// consequence expansion and final evidence decisions rather than simulating a second workflow.
#[cfg_attr(test, mutants::skip)]
pub fn run_preview(
    plan: &Path,
    prepared: &Path,
    output: &Path,
    manifest: &Path,
    verbose: Verbose<'_>,
) -> Result<String, AppError> {
    run_preview_with_options(
        plan,
        prepared,
        output,
        manifest,
        verbose,
        &CacheOptions::Default,
    )
}

#[cfg_attr(test, mutants::skip)] // Resolves original-workspace storage after preview admission.
pub fn run_preview_with_options(
    plan: &Path,
    prepared: &Path,
    output: &Path,
    manifest: &Path,
    verbose: Verbose<'_>,
    options: &CacheOptions,
) -> Result<String, AppError> {
    run_preview_acquiring_cache(plan, prepared, output, manifest, verbose, || {
        Cache::resolve(manifest, options, verbose)
    })
}

#[cfg_attr(test, mutants::skip)] // Native prospective lifetime and resolution.
pub fn run_preview_with_cache(
    plan: &Path,
    prepared: &Path,
    output: &Path,
    manifest: &Path,
    verbose: Verbose<'_>,
    cache: Cache,
) -> Result<String, AppError> {
    run_preview_acquiring_cache(plan, prepared, output, manifest, verbose, || Ok(cache))
}

#[cfg_attr(test, mutants::skip)] // Native prospective lifetime and resolution.
fn run_preview_acquiring_cache(
    plan: &Path,
    prepared: &Path,
    output: &Path,
    manifest: &Path,
    verbose: Verbose<'_>,
    acquire_cache: impl FnOnce() -> Result<Cache, AppError>,
) -> Result<String, AppError> {
    let output = absolute(output).map_err(|error| WriteFileError::caused_by(output, error))?;
    let (prepared_input, plan_input) =
        preview_inputs(plan, prepared, &output, manifest, |inputs| {
            inputs.verify(manifest, None).map(|_| ())
        })?;
    // Collision-safe completion invalidation precedes even cache metadata acquisition.
    // The selected original-workspace location is still fixed before prospective creation.
    let cache = acquire_cache()?;
    cache.protect(plan)?;
    cache.protect(prepared)?;
    cache.protect(&output)?;
    let prepared = prepared_input;
    let plan = plan_input;
    let prospective = Prospective::new(&output, &prepared.inputs)?;
    let mut cache = SnapshotCache::new(cache);
    let mut classification = classify_with_cache(
        &prospective.manifest,
        Some(&prepared.inputs.release_history),
        prepared.inputs.merge_target.as_deref(),
        verbose,
        &mut cache,
    )?;
    let resolved = resolve_plan(
        &plan,
        &classification.membership,
        &classification.work_tree.target_versions(),
        verbose,
    )?;
    require_semantic_decisions(&classification.packages, &resolved)?;

    let (resolved, files) = resolve_until_stable(
        resolved,
        |bytes| hash_bytes(bytes, &prospective.root),
        |resolved| {
            let (work_tree, _) = load_tracked_work_tree(&prospective.manifest)?;
            install_preview_edits(compute_edits(&work_tree, resolved, verbose)?, |edit| {
                fs::write(&edit.path, &edit.updated)
                    .map_err(|error| WriteFileError::caused_by(&edit.path, error).into())
            })?;
            prospective.resolve(verbose)?;
            classification = classify_with_cache(
                &prospective.manifest,
                Some(&prepared.inputs.release_history),
                prepared.inputs.merge_target.as_deref(),
                verbose,
                &mut cache,
            )?;
            let files = prospective.artifacts(&prepared.inputs)?;
            let mut expanded = resolved.clone();
            add_consequences(
                &classification.packages,
                &classification.groups,
                &classification.membership,
                &classification.work_tree,
                &mut expanded,
            )?;
            Ok((expanded, files))
        },
    )?;
    // Convergence leaves the candidate unchanged after this classification. Keep the readiness
    // verdict and report on those same observations; source/history verification still follows.
    // Ref: packages/cargo-release-plan/docs/implementation.md, "Prepared and prospective resolution".
    let (passed, message) = check_classification(&classification, CheckFormat::Text);
    require_complete_preview(passed, message)?;
    prepared.inputs.verify(manifest, None)?;
    let final_digest = prepared.inputs.final_digest(&files)?;
    let evidence_manifest_path = prospective.retain(&output, prepared.inputs.root())?;
    prepared
        .inputs
        .verify_candidate(&evidence_manifest_path, &final_digest)?;
    let mut plan = explicit_plan(&resolved);
    plan.release_history = Some(prepared.inputs.release_history.clone());
    plan.merge_target.clone_from(&prepared.inputs.merge_target);
    plan.resolved = Some(ResolvedState {
        final_digest,
        versions: resolved
            .packages
            .iter()
            .map(|(name, version)| (name.clone(), version.to_string()))
            .collect(),
        inputs: prepared.inputs,
        files,
        evidence_manifest_path,
    });
    write_report(&output, &classification)?;
    write_json(&output.join("plan.json"), &plan)?;
    Ok(format!(
        "Wrote complete resolved plan to {}",
        output.join("plan.json").display()
    ))
}

fn install_preview_edits(
    edits: Vec<ManifestEdit>,
    mut write: impl FnMut(&ManifestEdit) -> Result<(), AppError>,
) -> Result<(), AppError> {
    for edit in edits {
        if edit.original != edit.updated {
            write(&edit)?;
        }
    }
    Ok(())
}

fn resolve_until_stable(
    mut resolved: ResolvedVersions,
    mut hash: impl FnMut(&[u8]) -> Result<String, AppError>,
    mut pass: impl FnMut(&ResolvedVersions) -> Result<(ResolvedVersions, Vec<Artifact>), AppError>,
) -> Result<(ResolvedVersions, Vec<Artifact>), AppError> {
    // The callback owns rewriting, offline resolution and recapture; this loop owns convergence.
    // Ref: packages/cargo-release-plan/docs/implementation.md, "Prepared and prospective
    // resolution".
    let mut visited = BTreeSet::new();
    let mut previous_files = Vec::new();
    loop {
        let (expanded, files) = pass(&resolved)?;
        if expanded == resolved && files == previous_files {
            return Ok((resolved, files));
        }
        // Remember actual states rather than imposing an arbitrary iteration deadline.
        record_state(&mut visited, &expanded, &files, &mut hash)?;
        previous_files = files;
        resolved = expanded;
    }
}

pub fn preview_inputs(
    plan: &Path,
    prepared: &Path,
    output: &Path,
    manifest: &Path,
    verify: impl FnOnce(&Inputs) -> Result<(), AppError>,
) -> Result<(Prepared, PlanFile), AppError> {
    let marker = output.join("plan.json");
    let inputs = [plan, prepared, manifest];
    for input in inputs {
        if same_path(input, &marker)? {
            return Err(OutputInputCollision::new().into());
        }
    }
    // The completion marker belongs to this invocation from its first fallible input read.
    // A failed standalone rerun must not leave an earlier resolved plan looking current.
    remove_marker(&marker)?;
    guard_output_inputs(output, &inputs)?;
    let prepared: Prepared = read_json(prepared)?;
    verify(&prepared.inputs)?;
    let plan: PlanFile = read_json(plan)?;
    plan.validate_schema()?;
    plan.validate_history(&prepared.inputs)?;
    Ok((prepared, plan))
}

fn validate_preparation_files(
    root: &Path,
    lockfile: &Path,
    files: &[Artifact],
) -> Result<(), AppError> {
    if files.iter().any(|file| root.join(&file.path) != lockfile) {
        return Err(InvalidPreparation::new().into());
    }
    Ok(())
}

fn require_complete_preview(passed: bool, diagnostics: String) -> Result<(), AppError> {
    if !passed {
        return Err(IncompletePreview::new(diagnostics).into());
    }
    Ok(())
}

fn record_state(
    visited: &mut BTreeSet<String>,
    resolved: &ResolvedVersions,
    files: &[Artifact],
    hash: impl FnOnce(&[u8]) -> Result<String, AppError>,
) -> Result<(), AppError> {
    let state = serde_json::to_vec(&(explicit_plan(resolved), files))
        .expect("version plans and artifacts contain only JSON-compatible data");
    // Keep a bounded digest per iteration, not another retained copy of every resolved file.
    // Hash the complete state so a version, path, or content change cannot look like a cycle.
    if !visited.insert(hash(&state)?) {
        return Err(ResolutionCycle::new().into());
    }
    Ok(())
}

fn require_semantic_decisions(
    packages: &[PackageClass],
    resolved: &ResolvedVersions,
) -> Result<(), AppError> {
    for package in packages {
        if package.status() != PackageStatus::NeedsIncrement
            || package
                .changed()
                .iter()
                .all(|change| matches!(change, ChangedItem::Lockfile { .. }))
        {
            continue;
        }
        if !resolved.packages.get(&package.name).is_some_and(|version| {
            package
                .anchor()
                .is_some_and(|anchor| version > &anchor.version)
        }) {
            return Err(SemanticDecisionRequired::new(&package.name).into());
        }
    }
    Ok(())
}

fn add_consequences(
    packages: &[PackageClass],
    groups: &BTreeMap<String, GroupVerdict>,
    membership: &Groups,
    work_tree: &WorkTree,
    resolved: &mut ResolvedVersions,
) -> Result<(), AppError> {
    let versions = work_tree.target_versions();
    for (name, group) in groups {
        if !group.is_consistent() {
            raise(membership, &versions, resolved, name, group.version());
        }
    }
    for package in packages {
        if package.status() == PackageStatus::NeedsIncrement {
            let anchor = package.anchor().expect("needs-increment has an anchor");
            raise(
                membership,
                &versions,
                resolved,
                &package.name,
                &increment_version(&anchor.version, VersionBump::Patch)?,
            );
        }
        for dependency in &package.dependencies {
            let Some(version) = versions.get(&dependency.name) else {
                continue;
            };
            if !requirement_names_version(&dependency.req, version) {
                // Explicitly retaining the target version also schedules its requirement rewrites.
                raise(membership, &versions, resolved, &dependency.name, version);
            }
            if !dependency.public || releases_breaking_change(package) {
                continue;
            }
            let Some(anchor) = package.anchor() else {
                continue;
            };
            if packages
                .iter()
                .any(|target| target.name == dependency.name && releases_breaking_change(target))
            {
                let bump = if anchor.version.major == 0 {
                    VersionBump::Minor
                } else {
                    VersionBump::Major
                };
                raise(
                    membership,
                    &versions,
                    resolved,
                    &package.name,
                    &increment_version(&anchor.version, bump)?,
                );
            }
        }
    }
    for dependency in &work_tree.exact_dependencies {
        if let Some(version) = versions.get(&dependency.target)
            && !requirement_names_version(&dependency.requirement, version)
        {
            raise(membership, &versions, resolved, &dependency.target, version);
        }
    }
    Ok(())
}

fn raise(
    groups: &Groups,
    versions: &BTreeMap<String, Version>,
    resolved: &mut ResolvedVersions,
    target: &str,
    minimum: &Version,
) {
    let group = groups.group_of(target).unwrap_or(target);
    let mut members = groups.members(group).to_vec();
    if members.is_empty() {
        members.push(target.to_owned());
    }
    let version = members
        .iter()
        .filter_map(|name| resolved.packages.get(name).or_else(|| versions.get(name)))
        .chain([minimum])
        .max()
        .expect("the minimum version always participates")
        .clone();
    for member in members {
        resolved.packages.insert(member, version.clone());
    }
}

fn explicit_plan(resolved: &ResolvedVersions) -> PlanFile {
    PlanFile::new(
        PlanStage::Expanded,
        resolved
            .packages
            .iter()
            .map(|(name, version)| PlanIncrement {
                name: name.clone(),
                bump: None,
                version: Some(version.to_string()),
            })
            .collect(),
    )
}

// Native marker observation/removal; the injected operation tests invalidation and write errors.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn remove_marker(path: &Path) -> Result<(), AppError> {
    remove_marker_with(path, path.exists(), |path| fs::remove_file(path))
}

fn remove_marker_with(
    path: &Path,
    exists: bool,
    remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), AppError> {
    if exists {
        remove(path).map_err(|error| WriteFileError::caused_by(path, error))?;
    }
    Ok(())
}

// Native alias acquisition only; collision checks consume resolved observations in process.
#[cfg_attr(test, mutants::skip)]
fn guard_output_inputs(output: &Path, inputs: &[&Path]) -> Result<(), AppError> {
    guard_output_inputs_with(output, inputs, resolve_path)
}

fn guard_output_inputs_with(
    output: &Path,
    inputs: &[&Path],
    mut resolve: impl FnMut(&Path) -> Result<PathBuf, AppError>,
) -> Result<(), AppError> {
    let output = resolve(output)?;
    let files = ["report.json", "report.json.tmp"].map(|name| resolve(&output.join(name)));
    let [report, temporary] = files;
    let files = [report?, temporary?];
    for input in inputs {
        let input = resolve(input)?;
        validate_output_input(&output, &input, &files)?;
    }
    Ok(())
}

fn validate_output_input(output: &Path, input: &Path, files: &[PathBuf]) -> Result<(), AppError> {
    if files.iter().any(|file| file == input)
        || ["diffs", "workspace", ".prospective"]
            .iter()
            .any(|directory| input.starts_with(output.join(directory)))
    {
        return Err(OutputInputCollision::new().into());
    }
    Ok(())
}

/// Completion artifacts must not replace the invocation's own input documents.
#[ohno::error]
#[display("preview output overlaps an input; choose a separate output location")]
pub(crate) struct OutputInputCollision;

/// Source-level semantic decisions belong to the skill, not the resolver.
#[ohno::error]
#[display("package {package} needs a semantic release decision in the proposed plan")]
struct SemanticDecisionRequired {
    package: String,
}

/// Prepared bytes must remain the state captured before semantic assessment.
#[ohno::error]
#[display("prepared resolution artifacts changed; prepare and assess the report again")]
struct InvalidPreparation;

/// Predictable expansion must satisfy the same final gate as the live workspace.
#[ohno::error]
#[display("resolved preview does not pass the release gate: {diagnostics}")]
struct IncompletePreview {
    diagnostics: String,
}

/// An offline resolver must converge before producing an applicable artifact.
#[ohno::error]
#[display("offline release preview repeated a non-final state")]
struct ResolutionCycle;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {

    use std::collections::HashSet;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::iter;
    use std::path::PathBuf;

    use crp_workspace::lockfile::InstallationGraph;
    use crp_workspace::metadata::{DepKind, ExactDependency, ReportedDep, VersionTarget};
    use serde_json::Value;

    use super::*;
    use crate::anchor::Anchor;
    use crate::groups::Groups;
    use crate::resolved::StaleInputs;

    #[test]
    fn prospective_writes_skip_unchanged_manifests_and_stop_on_failure() {
        for fail in [false, true] {
            let edits = [("same", "same"), ("old", "new"), ("later", "updated")]
                .into_iter()
                .enumerate()
                .map(|(index, (original, updated))| ManifestEdit {
                    path: PathBuf::from(format!("member{index}/Cargo.toml")),
                    original: original.to_owned(),
                    updated: updated.to_owned(),
                })
                .collect();
            let mut writes = Vec::new();
            let result = install_preview_edits(edits, |edit| {
                writes.push((edit.path.clone(), edit.updated.clone()));
                if fail {
                    Err(WriteFileError::caused_by(
                        &edit.path,
                        std::io::Error::other("write failure"),
                    )
                    .into())
                } else {
                    Ok(())
                }
            });
            assert_eq!(
                writes,
                if fail {
                    vec![(PathBuf::from("member1/Cargo.toml"), "new".to_owned())]
                } else {
                    vec![
                        (PathBuf::from("member1/Cargo.toml"), "new".to_owned()),
                        (PathBuf::from("member2/Cargo.toml"), "updated".to_owned()),
                    ]
                }
            );
            assert_eq!(result.is_err(), fail);
            if let Err(error) = result {
                assert!(error.find_source::<WriteFileError>().is_some());
                assert!(error.find_source::<std::io::Error>().is_some());
            }
        }
    }

    #[test]
    fn resolved_output_paths_reject_each_owned_input_location() {
        let output = Path::new("output");
        let files = ["report.json", "report.json.tmp"].map(|name| output.join(name));
        for path in [
            "report.json",
            "report.json.tmp",
            "diffs/plan",
            "workspace/plan",
            ".prospective/plan",
        ] {
            let error = validate_output_input(output, &output.join(path), &files).unwrap_err();
            assert!(error.find_source::<OutputInputCollision>().is_some());
        }
        validate_output_input(output, Path::new("other/plan"), &files).unwrap();
    }

    #[test]
    fn acquired_output_aliases_cannot_replace_inputs() {
        let output = Path::new("output");
        for owned in [
            "report.json",
            "report.json.tmp",
            "workspace/plan",
            "diffs/plan",
            ".prospective/plan",
        ] {
            let error = guard_output_inputs_with(output, &[Path::new("alias")], |path| {
                Ok(if path == Path::new("alias") {
                    output.join(owned)
                } else {
                    path.into()
                })
            })
            .unwrap_err();
            assert!(error.find_source::<OutputInputCollision>().is_some());
        }
        guard_output_inputs_with(output, &[Path::new("independent")], |path| Ok(path.into()))
            .unwrap();
        assert!(
            guard_output_inputs_with(output, &[Path::new("input")], |_| Err(
                std::io::Error::other("identity").into()
            ))
            .is_err()
        );
    }

    #[test]
    fn marker_invalidation_removes_only_present_markers_and_propagates_failure() {
        for exists in [false, true] {
            let path = Path::new("plan.json");
            let mut called = false;
            let result = remove_marker_with(path, exists, |actual| {
                called = true;
                assert_eq!(actual, path);
                Err(std::io::ErrorKind::PermissionDenied.into())
            });
            assert_eq!(called, exists);
            if exists {
                assert!(
                    result
                        .unwrap_err()
                        .find_source::<WriteFileError>()
                        .is_some()
                );
            } else {
                result.unwrap();
            }
        }
        remove_marker_with(Path::new("plan.json"), true, |_| Ok(())).unwrap();
    }

    #[test]
    fn resolution_waits_for_stable_captured_files_without_changing_versions() {
        let initial = ResolvedVersions {
            packages: BTreeMap::from([("tool".to_owned(), Version::new(1, 0, 1))]),
        };
        // Model successive offline resolver writes, including a lockfile-only change.
        // Exhausting these expected passes fails immediately rather than timing out.
        let mut writes = [
            "first resolution",
            "reselected dependency",
            "reselected dependency",
        ]
        .into_iter();
        let (resolved, files) = resolve_until_stable(
            initial.clone(),
            |bytes| Ok(digest(bytes)),
            |resolved| {
                assert_eq!(resolved, &initial);
                Ok((
                    resolved.clone(),
                    vec![Artifact {
                        path: "Cargo.lock".into(),
                        contents: writes.next().unwrap().to_owned(),
                    }],
                ))
            },
        )
        .unwrap();
        assert!(writes.next().is_none());
        assert_eq!(resolved, initial);
        assert_eq!(
            files,
            [Artifact {
                path: "Cargo.lock".into(),
                contents: "reselected dependency".to_owned(),
            }]
        );
    }

    #[test]
    fn resolution_applies_new_versions_even_when_captured_files_are_stable() {
        let initial = ResolvedVersions {
            packages: BTreeMap::new(),
        };
        let expanded = ResolvedVersions {
            packages: BTreeMap::from([("tool".to_owned(), Version::new(1, 0, 1))]),
        };
        let files = vec![Artifact {
            path: "Cargo.lock".into(),
            contents: "resolved dependency".to_owned(),
        }];
        let mut passes = [
            (&initial, &initial),
            (&initial, &expanded),
            (&expanded, &expanded),
        ]
        .into_iter();
        let (resolved, captured) = resolve_until_stable(
            initial.clone(),
            |bytes| Ok(digest(bytes)),
            |resolved| {
                let (expected, next) = passes.next().unwrap();
                assert_eq!(resolved, expected);
                Ok((next.clone(), files.clone()))
            },
        )
        .unwrap();
        assert!(passes.next().is_none());
        assert_eq!(resolved, expanded);
        assert_eq!(captured, files);
    }

    #[test]
    fn resolution_accepts_an_unchanged_empty_artifact_set() {
        let initial = ResolvedVersions {
            packages: BTreeMap::new(),
        };
        let mut passes = iter::once(());
        let (resolved, files) = resolve_until_stable(
            initial.clone(),
            |_| panic!("an unchanged state needs no hash"),
            |resolved| {
                passes.next().unwrap();
                Ok((resolved.clone(), Vec::new()))
            },
        )
        .unwrap();
        assert!(passes.next().is_none());
        assert_eq!(resolved, initial);
        assert!(files.is_empty());
    }

    #[test]
    fn resolution_rejects_a_captured_file_cycle_without_accepting_stable_versions() {
        let initial = ResolvedVersions {
            packages: BTreeMap::new(),
        };
        let mut contents = ["first resolution", "other resolution", "first resolution"].into_iter();
        let error = resolve_until_stable(
            initial.clone(),
            |bytes| Ok(digest(bytes)),
            |resolved| {
                assert_eq!(resolved, &initial);
                Ok((
                    resolved.clone(),
                    vec![Artifact {
                        path: "Cargo.lock".into(),
                        contents: contents.next().unwrap().to_owned(),
                    }],
                ))
            },
        )
        .unwrap_err();
        assert!(error.find_source::<ResolutionCycle>().is_some());
        assert!(contents.next().is_none());
    }

    #[test]
    fn resolution_propagates_a_failed_pass_instead_of_accepting_previous_files() {
        let initial = ResolvedVersions {
            packages: BTreeMap::new(),
        };
        let mut passes = [
            Ok(vec![Artifact {
                path: "Cargo.lock".into(),
                contents: "incomplete resolution".to_owned(),
            }]),
            Err(StaleInputs::new().into()),
        ]
        .into_iter();
        let error = resolve_until_stable(
            initial.clone(),
            |bytes| Ok(digest(bytes)),
            |resolved| {
                assert_eq!(resolved, &initial);
                passes
                    .next()
                    .unwrap()
                    .map(|files| (resolved.clone(), files))
            },
        )
        .unwrap_err();
        assert!(error.find_source::<StaleInputs>().is_some());
        assert!(passes.next().is_none());
    }

    fn work_tree(packages: &[PackageClass]) -> WorkTree {
        WorkTree {
            workspace_root: PathBuf::new(),
            packages: Vec::new(),
            version_targets: packages
                .iter()
                .map(|package| VersionTarget {
                    name: package.name.clone(),
                    version: package.declared_version.clone(),
                    manifest_path: package.manifest_path.clone(),
                    publishable: true,
                })
                .collect(),
            exact_dependencies: Vec::new(),
            member_manifests: Vec::new(),
            members_by_dir: BTreeMap::new(),
            installation: InstallationGraph::default(),
        }
    }

    fn package(name: &str, previous: Option<&str>, current: &str) -> PackageClass {
        let version = Version::parse(current).unwrap();
        match previous {
            Some(previous) => {
                let anchor = Anchor {
                    commit: "release".to_owned(),
                    version: Version::parse(previous).unwrap(),
                };
                if anchor.version == version {
                    PackageClass::unchanged(name, version, anchor, PathBuf::new())
                } else {
                    PackageClass::pending_release(name, version, anchor, PathBuf::new())
                }
            }
            None => PackageClass::new_package(name, version, PathBuf::new()),
        }
    }

    #[test]
    fn public_dependency_consequences_respect_anchors_and_sufficient_existing_versions() {
        for (old_core, core, old_facade, facade, public, expected) in [
            (
                "0.1.0",
                "0.2.0",
                Some("0.1.0"),
                "0.1.0",
                true,
                Some("0.2.0"),
            ),
            (
                "1.0.0",
                "2.0.0",
                Some("1.0.0"),
                "1.0.0",
                true,
                Some("2.0.0"),
            ),
            ("0.1.0", "0.2.0", Some("0.1.0"), "0.3.0", true, None),
            ("1.0.0", "2.0.0", None, "1.0.0", true, None),
            ("0.1.0", "0.2.0", Some("0.1.0"), "0.1.0", false, None),
            ("0.1.0", "0.1.1", Some("0.1.0"), "0.1.0", true, None),
        ] {
            let core_package = package("core", Some(old_core), core);
            let mut facade_package = package("facade", old_facade, facade);
            facade_package.dependencies.push(ReportedDep {
                name: "core".to_owned(),
                req: core.to_owned(),
                exact_pin: false,
                kind: DepKind::Normal,
                public,
            });
            let packages = [core_package, facade_package];
            let mut resolved = ResolvedVersions {
                packages: BTreeMap::from([("core".to_owned(), Version::parse(core).unwrap())]),
            };
            add_consequences(
                &packages,
                &BTreeMap::new(),
                &Groups::from_workspace(&work_tree(&packages)),
                &work_tree(&packages),
                &mut resolved,
            )
            .unwrap();
            assert_eq!(
                resolved.packages.get("facade"),
                expected
                    .map(|version| Version::parse(version).unwrap())
                    .as_ref()
            );
            assert_eq!(
                resolved.packages.get("core").unwrap(),
                &Version::parse(core).unwrap()
            );
        }
    }

    #[test]
    fn group_consequences_align_unpublished_members_without_lowering_planned_versions() {
        let packages = [
            package("core", Some("0.1.0"), "0.2.0"),
            package("helper", Some("0.1.0"), "0.1.0"),
        ];
        let mut work_tree = work_tree(&packages);
        work_tree.version_targets.get_mut(1).unwrap().publishable = false;
        work_tree.exact_dependencies.push(ExactDependency {
            source: "helper".to_owned(),
            target: "core".to_owned(),
            requirement: "=0.2.0".to_owned(),
            manifest_path: PathBuf::from("packages/helper/Cargo.toml"),
            location: "dependencies.core".to_owned(),
        });
        let verdict = GroupVerdict::new(
            Groups::from_workspace(&work_tree).members("core"),
            &work_tree.target_versions(),
            &HashSet::new(),
        );
        let groups = BTreeMap::from([("core".to_owned(), verdict)]);
        let mut resolved = ResolvedVersions {
            packages: BTreeMap::new(),
        };
        add_consequences(
            &packages[..1],
            &groups,
            &Groups::from_workspace(&work_tree),
            &work_tree,
            &mut resolved,
        )
        .unwrap();
        assert_eq!(
            resolved.packages,
            BTreeMap::from([
                ("core".to_owned(), Version::new(0, 2, 0)),
                ("helper".to_owned(), Version::new(0, 2, 0)),
            ])
        );
        resolved
            .packages
            .insert("helper".to_owned(), Version::new(0, 3, 0));
        add_consequences(
            &packages[..1],
            &groups,
            &Groups::from_workspace(&work_tree),
            &work_tree,
            &mut resolved,
        )
        .unwrap();
        assert!(
            resolved
                .packages
                .values()
                .all(|version| *version == Version::new(0, 3, 0))
        );
    }

    #[test]
    fn unpublished_exact_dependencies_schedule_only_needed_requirement_rewrites() {
        let mut work_tree = work_tree(&[
            package("core", None, "0.2.0"),
            package("helper", None, "0.2.0"),
        ]);
        for target in &mut work_tree.version_targets {
            target.publishable = false;
        }
        work_tree.exact_dependencies.push(ExactDependency {
            source: "helper".to_owned(),
            target: "core".to_owned(),
            requirement: "=0.2.0".to_owned(),
            manifest_path: PathBuf::from("packages/helper/Cargo.toml"),
            location: "dependencies.core".to_owned(),
        });
        let groups = BTreeMap::from([(
            "core".to_owned(),
            GroupVerdict::new(
                Groups::from_workspace(&work_tree).members("core"),
                &work_tree.target_versions(),
                &HashSet::new(),
            ),
        )]);
        let mut resolved = ResolvedVersions {
            packages: BTreeMap::new(),
        };

        // Unpublished members have no release assessments, but their exact requirements
        // must still be rewritten. Matching requirements must not invent plan entries.
        add_consequences(
            &[],
            &groups,
            &Groups::from_workspace(&work_tree),
            &work_tree,
            &mut resolved,
        )
        .unwrap();
        assert!(resolved.packages.is_empty());

        work_tree
            .exact_dependencies
            .first_mut()
            .unwrap()
            .requirement = "=0.1.0".to_owned();
        add_consequences(
            &[],
            &groups,
            &Groups::from_workspace(&work_tree),
            &work_tree,
            &mut resolved,
        )
        .unwrap();
        assert_eq!(
            resolved.packages,
            BTreeMap::from([
                ("core".to_owned(), Version::new(0, 2, 0)),
                ("helper".to_owned(), Version::new(0, 2, 0)),
            ])
        );
    }

    #[test]
    fn semantic_decisions_cover_source_and_inherited_changes_but_not_only_lockfile_changes() {
        let lockfile = ChangedItem::Lockfile {
            dependency: "third-party".to_owned(),
            change: "changed".to_owned(),
        };
        for changed in [
            vec![ChangedItem::Package {
                path: "src/lib.rs".to_owned(),
                change: "modified".to_owned(),
            }],
            vec![ChangedItem::Inherited {
                field: "edition".to_owned(),
            }],
            vec![
                lockfile.clone(),
                ChangedItem::Package {
                    path: "src/lib.rs".to_owned(),
                    change: "modified".to_owned(),
                },
            ],
        ] {
            let package = PackageClass::needs_increment(
                "demo",
                Version::new(0, 1, 0),
                Anchor {
                    commit: "release".to_owned(),
                    version: Version::new(0, 1, 0),
                },
                changed,
                PathBuf::new(),
            );
            let packages = [package];
            for version in [None, Some(Version::new(0, 1, 0))] {
                let resolved = ResolvedVersions {
                    packages: version
                        .map(|version| ("demo".to_owned(), version))
                        .into_iter()
                        .collect(),
                };
                let error = require_semantic_decisions(&packages, &resolved).unwrap_err();
                assert!(error.find_source::<SemanticDecisionRequired>().is_some());
            }
            let resolved = ResolvedVersions {
                packages: BTreeMap::from([("demo".to_owned(), Version::new(0, 1, 1))]),
            };
            require_semantic_decisions(&packages, &resolved).unwrap();
        }
        let packages = [
            PackageClass::needs_increment(
                "binary",
                Version::new(0, 1, 0),
                Anchor {
                    commit: "release".to_owned(),
                    version: Version::new(0, 1, 0),
                },
                vec![lockfile],
                PathBuf::new(),
            ),
            package("new", None, "1.0.0"),
            package("unchanged", Some("1.0.0"), "1.0.0"),
            package("released", Some("1.0.0"), "1.0.1"),
        ];
        require_semantic_decisions(
            &packages,
            &ResolvedVersions {
                packages: BTreeMap::new(),
            },
        )
        .unwrap();
    }

    #[test]
    fn requirement_rewrites_schedule_only_the_dependent_for_a_release() {
        let core = package("core", Some("0.1.0"), "0.1.0");
        let mut facade = package("facade", Some("0.1.0"), "0.1.0");
        facade.dependencies.push(ReportedDep {
            name: "core".to_owned(),
            req: "0.1".to_owned(),
            exact_pin: false,
            kind: DepKind::Normal,
            public: false,
        });
        let initial = [core.clone(), facade];
        let mut resolved = ResolvedVersions {
            packages: BTreeMap::new(),
        };
        require_semantic_decisions(&initial, &resolved).unwrap();
        add_consequences(
            &initial,
            &BTreeMap::new(),
            &Groups::from_workspace(&work_tree(&initial)),
            &work_tree(&initial),
            &mut resolved,
        )
        .unwrap();
        assert_eq!(
            resolved.packages,
            BTreeMap::from([("core".to_owned(), Version::new(0, 1, 0))])
        );

        // The next classification observes the requirement edit as released manifest content.
        let facade = PackageClass::needs_increment(
            "facade",
            Version::new(0, 1, 0),
            Anchor {
                commit: "release".to_owned(),
                version: Version::new(0, 1, 0),
            },
            vec![ChangedItem::Package {
                path: "Cargo.toml".to_owned(),
                change: "modified".to_owned(),
            }],
            PathBuf::new(),
        );
        let after_rewrite = [core, facade];
        add_consequences(
            &after_rewrite,
            &BTreeMap::new(),
            &Groups::from_workspace(&work_tree(&after_rewrite)),
            &work_tree(&after_rewrite),
            &mut resolved,
        )
        .unwrap();
        assert_eq!(
            resolved.packages,
            BTreeMap::from([
                ("core".to_owned(), Version::new(0, 1, 0)),
                ("facade".to_owned(), Version::new(0, 1, 1)),
            ])
        );
    }

    #[test]
    fn sufficient_existing_increments_leave_an_empty_plan_unchanged() {
        let core = package("core", Some("0.1.0"), "0.1.1");
        let mut dependent = package("dependent", Some("0.1.0"), "0.1.1");
        dependent.dependencies.push(ReportedDep {
            name: "core".to_owned(),
            req: "0.1.1".to_owned(),
            exact_pin: false,
            kind: DepKind::Normal,
            public: true,
        });
        let packages = [core, dependent];
        let mut resolved = ResolvedVersions {
            packages: BTreeMap::new(),
        };
        require_semantic_decisions(&packages, &resolved).unwrap();
        add_consequences(
            &packages,
            &BTreeMap::new(),
            &Groups::from_workspace(&work_tree(&packages)),
            &work_tree(&packages),
            &mut resolved,
        )
        .unwrap();
        assert!(resolved.packages.is_empty());
    }

    #[test]
    fn preparation_accepts_only_the_workspace_lockfile() {
        let root = Path::new("repository");
        let lockfile = root.join("workspace/Cargo.lock");
        validate_preparation_files(root, &lockfile, &[]).unwrap();
        let mut files = [Artifact {
            path: "workspace/Cargo.lock".into(),
            contents: String::new(),
        }];
        validate_preparation_files(root, &lockfile, &files).unwrap();
        files[0].path = "workspace/Cargo.toml".into();
        let error = validate_preparation_files(root, &lockfile, &files).unwrap_err();
        assert!(error.find_source::<InvalidPreparation>().is_some());
    }

    #[test]
    fn incomplete_preview_preserves_the_release_gate_diagnostics() {
        require_complete_preview(true, String::new()).unwrap();
        let diagnostics = "package requires an increment";
        let error = require_complete_preview(false, diagnostics.to_owned()).unwrap_err();
        assert_eq!(
            error
                .find_source::<IncompletePreview>()
                .unwrap()
                .diagnostics,
            diagnostics
        );
    }

    #[test]
    fn history_detects_repetition_without_retaining_artifact_contents() {
        let mut visited = BTreeSet::new();
        let mut resolved = ResolvedVersions {
            packages: BTreeMap::from([("tool".to_owned(), Version::new(1, 0, 0))]),
        };
        let mut files = [Artifact {
            path: "Cargo.lock".into(),
            contents: "initial resolution".to_owned(),
        }];
        record_state(&mut visited, &resolved, &files, |bytes| Ok(digest(bytes))).unwrap();
        let error =
            record_state(&mut visited, &resolved, &files, |bytes| Ok(digest(bytes))).unwrap_err();
        assert!(error.find_source::<ResolutionCycle>().is_some());
        assert_eq!(visited.len(), 1);

        resolved
            .packages
            .insert("tool".to_owned(), Version::new(1, 0, 1));
        record_state(&mut visited, &resolved, &files, |bytes| Ok(digest(bytes))).unwrap();
        files[0].path = "Cargo.toml".into();
        record_state(&mut visited, &resolved, &files, |bytes| Ok(digest(bytes))).unwrap();
        files[0].contents = "resolved dependency\n".repeat(1024);
        let token = "digest supplied by the acquisition boundary";
        record_state(&mut visited, &resolved, &files, |bytes| {
            let state: Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(state.pointer("/0/increments/0/name").unwrap(), "tool");
            assert_eq!(state.pointer("/0/increments/0/version").unwrap(), "1.0.1");
            assert_eq!(state.pointer("/1/0/path").unwrap(), "Cargo.toml");
            assert_eq!(state.pointer("/1/0/contents").unwrap(), &files[0].contents);
            Ok(token.to_owned())
        })
        .unwrap();
        assert_eq!(visited.len(), 4);
        assert!(visited.contains(token));
    }

    #[test]
    fn resolution_propagates_hash_failure_before_another_pass() {
        let mut passes = iter::once(());
        let error = resolve_until_stable(
            ResolvedVersions {
                packages: BTreeMap::new(),
            },
            |_| Err(StaleInputs::new().into()),
            |resolved| {
                passes.next().unwrap();
                Ok((
                    resolved.clone(),
                    vec![Artifact {
                        path: "Cargo.lock".into(),
                        contents: "resolved".to_owned(),
                    }],
                ))
            },
        )
        .unwrap_err();
        assert!(error.find_source::<StaleInputs>().is_some());
        assert!(passes.next().is_none());
    }

    fn digest(bytes: &[u8]) -> String {
        // Convergence depends on state identity, not Git's digest encoding. The real
        // hash adapter runs in preview integration tests, never in this decision suite.
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}
