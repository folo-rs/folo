// Expanded-plan inspection supplies typed facts to publication and evidence callers.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crp_diag::Verbose;
use crp_workspace::git::GitRepo;
use crp_workspace::metadata::{WorkTree, load_tracked_work_tree};
use ohno::AppError;
use serde::Serialize;

use crate::classify::{AcquiredWorkspace, SnapshotCache};
use crate::groups::Groups;
use crate::plan::{PlanFile, PlanStage, resolve_plan};
use crate::resolved::{ResolutionRequired, ResolvedState, read_json, validate_application};

/// Reads and validates preview evidence without producing publication-target output.
#[cfg_attr(test, mutants::skip)] // Real acquisition is exercised by compatibility integration tests.
pub fn read_resolved_preview(
    path: &Path,
    manifest: &Path,
    verbose: Verbose<'_>,
) -> Result<ResolvedState, AppError> {
    read_resolved_preview_with_cache(path, manifest, verbose, &mut SnapshotCache::default())
        .map(|(state, _)| state)
}

/// Admits a resolved preview and retains its candidate observations for read-only work.
///
/// Assessed source, configuration and history must remain unchanged while using
/// the returned candidate.
#[cfg_attr(test, mutants::skip)] // Native admission; isolation and reuse have integration coverage.
pub fn read_resolved_preview_with_cache(
    path: &Path,
    manifest: &Path,
    verbose: Verbose<'_>,
    cache: &mut SnapshotCache,
) -> Result<(ResolvedState, AcquiredWorkspace), AppError> {
    let plan: PlanFile = read_json(path)?;
    validate_expanded(&plan)?;
    let state = plan.resolved.as_ref().ok_or_else(ResolutionRequired::new)?;
    // Parsed documents can publish entries during admission, before classification.
    // Protect the whole retained repository first, including a nested workspace's siblings.
    cache.protect(path)?;
    cache.protect(
        GitRepo::discover(
            state
                .evidence_manifest_path
                .parent()
                .ok_or_else(ResolutionRequired::new)?,
        )?
        .root(),
    )?;
    drop(validate_application(&plan, manifest, verbose, cache)?);
    resolved_preview(plan, |state| {
        state.acquire_candidate(&state.evidence_manifest_path, verbose, cache)
    })
}

fn resolved_preview<T>(
    plan: PlanFile,
    verify_candidate: impl FnOnce(&ResolvedState) -> Result<T, AppError>,
) -> Result<(ResolvedState, T), AppError> {
    let state = plan.resolved.ok_or_else(ResolutionRequired::new)?;
    let acquired = verify_candidate(&state)?;
    Ok((state, acquired))
}

/// Publication eligibility comes from tracked Cargo members, not package naming.
#[derive(Debug, Serialize)]
pub struct PlanInspection {
    /// Publishable targets selected by the validated expanded plan.
    pub publication_targets: Vec<String>,
    /// Verified retained preview manifest when the plan contains resolved state.
    pub evidence_manifest_path: Option<PathBuf>,
}

// Only the real filesystem/Git adapters are excluded; inspection decisions and serialization
// run in the unit-tested core. See packages/cargo-release-plan/docs/implementation.md, "Test
// boundaries".
#[cfg_attr(test, mutants::skip)]
pub fn run_inspect_plan(
    path: &Path,
    require_resolved: bool,
    manifest: &Path,
    verbose: Verbose<'_>,
) -> Result<String, AppError> {
    let inspection = read_plan_inspection(path, require_resolved, manifest, verbose)?;
    Ok(serde_json::to_string(&inspection)
        .expect("plan inspection contains only JSON-compatible artifact fields"))
}

/// Acquires the same validated inspection used by the CLI without a JSON round trip.
#[cfg_attr(test, mutants::skip)] // Real input acquisition and validation have boundary coverage.
pub fn read_plan_inspection(
    path: &Path,
    require_resolved: bool,
    manifest: &Path,
    verbose: Verbose<'_>,
) -> Result<PlanInspection, AppError> {
    let plan: PlanFile = read_json(path)?;
    inspect_plan(
        plan,
        require_resolved,
        verbose,
        |plan| {
            // Reuse application's captured-state validation without installing any files.
            // The registry probe must see the same target set that application will accept.
            validate_application(plan, manifest, verbose, &mut SnapshotCache::default())
                .map(|(_, acquired)| acquired.work_tree)
        },
        |state| {
            state
                .acquire_candidate(
                    &state.evidence_manifest_path,
                    verbose,
                    &mut SnapshotCache::default(),
                )
                .map(|_| ())
        },
        || load_tracked_work_tree(manifest).map(|(work_tree, _)| work_tree),
    )
}

fn inspect_plan(
    plan: PlanFile,
    require_resolved: bool,
    verbose: Verbose<'_>,
    validate_resolved: impl FnOnce(&PlanFile) -> Result<WorkTree, AppError>,
    verify_candidate: impl FnOnce(&ResolvedState) -> Result<(), AppError>,
    load_work_tree: impl FnOnce() -> Result<WorkTree, AppError>,
) -> Result<PlanInspection, AppError> {
    let work_tree =
        match validate_inputs(&plan, require_resolved, validate_resolved, verify_candidate)? {
            Some(work_tree) => work_tree,
            None => load_work_tree()?,
        };
    let resolved = resolve_plan(
        &plan,
        &Groups::from_workspace(&work_tree),
        &work_tree.target_versions(),
        verbose,
    )?;
    let publication_targets = work_tree
        .version_targets
        .iter()
        .filter(|target| target.publishable && resolved.packages.contains_key(&target.name))
        .map(|target| target.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(PlanInspection {
        publication_targets,
        evidence_manifest_path: plan.resolved.map(|state| state.evidence_manifest_path),
    })
}

fn validate_inputs<T>(
    plan: &PlanFile,
    require_resolved: bool,
    validate_resolved: impl FnOnce(&PlanFile) -> Result<T, AppError>,
    verify_candidate: impl FnOnce(&ResolvedState) -> Result<(), AppError>,
) -> Result<Option<T>, AppError> {
    validate_expanded(plan)?;
    let acquired = (require_resolved || plan.resolved.is_some())
        .then(|| validate_resolved(plan))
        .transpose()?;
    if let Some(state) = &plan.resolved {
        // Both consumers can run compatibility tooling immediately after validation.
        verify_candidate(state)?;
    }
    Ok(acquired)
}

fn validate_expanded(plan: &PlanFile) -> Result<(), AppError> {
    plan.validate_schema()?;
    let mut names = BTreeSet::new();
    if plan.stage() != PlanStage::Expanded
        || plan.increments.iter().any(|increment| {
            increment.version.is_none()
                || increment.bump.is_some()
                || !names.insert(&increment.name)
        })
    {
        return Err(ExpandedPlanRequired::new().into());
    }
    Ok(())
}

/// Inspection requires the complete, uniquely named package/version set.
#[ohno::error]
#[display("inspection requires an expanded plan with one explicit version per package")]
struct ExpandedPlanRequired;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;

    use crp_workspace::lockfile::InstallationGraph;
    use crp_workspace::metadata::{ManifestSnapshot, VersionTarget};
    use semver::Version;
    use serde_json::json;

    use super::*;
    use crate::plan::PlanIncrement;
    use crate::{UnknownPlanTargetError, UnsupportedPlanSchemaError};

    #[test]
    fn resolved_preview_returns_verified_state_without_publication_projection() {
        let plan = plan(&["api"], true);
        let expected = plan.resolved.clone().unwrap();
        let order = Cell::new(0);
        let state = resolved_preview(plan, |state| {
            assert_eq!(order.replace(1), 0);
            assert_eq!(state, &expected);
            Ok("acquired")
        })
        .unwrap();
        assert_eq!(state, (expected, "acquired"));
        assert_eq!(order.get(), 1);
    }

    #[test]
    fn resolved_preview_requires_state_and_preserves_validation_failures() {
        let error = resolved_preview::<()>(plan(&[], false), |_| panic!()).unwrap_err();
        assert!(error.find_source::<ResolutionRequired>().is_some());
        let error =
            resolved_preview::<()>(plan(&[], true), |_| Err(InspectionFailure::new().into()))
                .unwrap_err();
        assert!(error.find_source::<InspectionFailure>().is_some());
    }

    #[test]
    fn resolved_validation_is_required_by_either_the_flag_or_captured_state() {
        for require_resolved in [false, true] {
            for has_state in [false, true] {
                let plan = plan(&[], has_state);
                let expected = plan.clone();
                let validated = Cell::new(false);
                let result = inspect_plan(
                    plan,
                    require_resolved,
                    Verbose::new(false, &crp_diag::Discard),
                    |plan| {
                        assert_eq!(*plan, expected);
                        validated.set(true);
                        Err(InspectionFailure::new().into())
                    },
                    |_| panic!(),
                    || Ok(work_tree(&[])),
                );
                match (require_resolved, has_state) {
                    (false, false) => {
                        assert!(!validated.get());
                        assert_eq!(
                            serde_json::to_value(result.unwrap()).unwrap(),
                            json!({
                                "publication_targets": [],
                                "evidence_manifest_path": null
                            })
                        );
                    }
                    _ => {
                        assert!(validated.get());
                        assert!(
                            result
                                .unwrap_err()
                                .find_source::<InspectionFailure>()
                                .is_some()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn inspection_reuses_admitted_workspace_and_serializes_the_publication_intersection() {
        let plan = plan(&["zeta", "helper", "api"], true);
        let expected = plan.clone();
        let order = Cell::new(0);
        let output = inspect_plan(
            plan,
            false,
            Verbose::new(false, &crp_diag::Discard),
            |plan| {
                assert_eq!(order.replace(1), 0);
                assert_eq!(*plan, expected);
                Ok(work_tree(&[
                    ("zeta", true),
                    ("helper", false),
                    ("unselected-api", true),
                    ("unselected-helper", false),
                    ("api", true),
                    ("api", true),
                ]))
            },
            |state| {
                assert_eq!(order.replace(2), 1);
                assert_eq!(state, expected.resolved.as_ref().unwrap());
                Ok(())
            },
            || panic!("source admission already acquired this workspace"),
        )
        .unwrap();
        assert_eq!(order.get(), 2);
        assert_eq!(
            serde_json::to_value(output).unwrap(),
            json!({
                "publication_targets": ["api", "zeta"],
                "evidence_manifest_path": expected.resolved.unwrap().evidence_manifest_path
            })
        );
    }

    #[test]
    fn stale_candidate_and_workspace_acquisition_errors_propagate() {
        let error = inspect_plan(
            plan(&[], true),
            false,
            Verbose::new(false, &crp_diag::Discard),
            |_| Ok(work_tree(&[])),
            |_| Err(InspectionFailure::new().into()),
            || panic!(),
        )
        .unwrap_err();
        assert!(error.find_source::<InspectionFailure>().is_some());

        let error = inspect_plan(
            plan(&[], false),
            false,
            Verbose::new(false, &crp_diag::Discard),
            |_| panic!(),
            |_| panic!(),
            || Err(InspectionFailure::new().into()),
        )
        .unwrap_err();
        assert!(error.find_source::<InspectionFailure>().is_some());
    }

    #[test]
    fn invalid_artifacts_are_rejected_before_external_validation_or_acquisition() {
        let error = inspect_plan(
            PlanFile::new(PlanStage::Proposed, Vec::new()),
            true,
            Verbose::new(false, &crp_diag::Discard),
            |_| panic!(),
            |_| panic!(),
            || panic!(),
        )
        .unwrap_err();
        assert!(error.find_source::<ExpandedPlanRequired>().is_some());

        let error = inspect_plan(
            PlanFile::with_schema_version(0),
            false,
            Verbose::new(false, &crp_diag::Discard),
            |_| panic!(),
            |_| panic!(),
            || panic!(),
        )
        .unwrap_err();
        assert!(error.find_source::<UnsupportedPlanSchemaError>().is_some());
    }

    #[test]
    fn selected_names_must_resolve_against_tracked_workspace_targets() {
        let error = inspect_plan(
            plan(&["absent"], false),
            false,
            Verbose::new(false, &crp_diag::Discard),
            |_| panic!(),
            |_| panic!(),
            || Ok(work_tree(&[("api", true)])),
        )
        .unwrap_err();
        assert!(error.find_source::<UnknownPlanTargetError>().is_some());
    }

    fn plan(names: &[&str], resolved: bool) -> PlanFile {
        let mut plan = PlanFile::new(
            PlanStage::Expanded,
            names
                .iter()
                .map(|name| PlanIncrement {
                    name: (*name).to_owned(),
                    bump: None,
                    version: Some("1.0.1".to_owned()),
                })
                .collect(),
        );
        if resolved {
            // Capture contents are opaque to orchestration; its injected validators own them.
            plan.resolved = Some(
                serde_json::from_value(json!({
                    "inputs": {
                        "root": "workspace", "manifest": "Cargo.toml",
                        "head": "head", "release_history": "base", "release_history_revision": "main",
                        "index": "", "paths": [], "digest": "inputs"
                    },
                    "files": [], "final_digest": "candidate", "versions": {},
                    "evidence_manifest_path": "candidate \"quoted\"\\Cargo.toml"
                }))
                .unwrap(),
            );
        }
        plan
    }

    fn work_tree(targets: &[(&str, bool)]) -> WorkTree {
        WorkTree {
            manifests: ManifestSnapshot::default(),
            tracked_paths: Vec::new(),
            workspace_root: PathBuf::from("workspace"),
            packages: Vec::new(),
            version_targets: targets
                .iter()
                .map(|(name, publishable)| VersionTarget {
                    name: (*name).to_owned(),
                    version: Version::new(1, 0, 0),
                    manifest_path: PathBuf::from(name).join("Cargo.toml"),
                    publishable: *publishable,
                })
                .collect(),
            exact_dependencies: Vec::new(),
            member_manifests: Vec::new(),
            members_by_dir: BTreeMap::new(),
            installation: InstallationGraph::default(),
        }
    }

    /// Identifies an injected external failure without relying on diagnostic wording.
    #[ohno::error]
    struct InspectionFailure;

    #[test]
    fn only_complete_explicit_unique_expansions_are_inspectable() {
        let increment = PlanIncrement {
            name: "api".to_owned(),
            bump: None,
            version: Some("1.0.1".to_owned()),
        };
        let plan = PlanFile::new(PlanStage::Expanded, vec![increment.clone()]);
        validate_expanded(&plan).unwrap();
        for plan in [
            PlanFile::new(PlanStage::Proposed, vec![increment.clone()]),
            PlanFile::new(
                PlanStage::Expanded,
                vec![increment.clone(), increment.clone()],
            ),
            PlanFile::new(
                PlanStage::Expanded,
                vec![PlanIncrement {
                    bump: Some("patch".to_owned()),
                    ..increment.clone()
                }],
            ),
            PlanFile::new(
                PlanStage::Expanded,
                vec![PlanIncrement {
                    version: None,
                    ..increment
                }],
            ),
        ] {
            assert!(
                validate_expanded(&plan)
                    .unwrap_err()
                    .find_source::<ExpandedPlanRequired>()
                    .is_some()
            );
        }
    }
}
