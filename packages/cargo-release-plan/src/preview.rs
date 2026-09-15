// Explicit offline preparation and proposal-specific fixed-point resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, absolute};

use ohno::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::WriteFileError;
use crate::apply::compute_edits;
use crate::artifact_path::{resolve_path, same_path};
use crate::check::{CheckFormat, releases_breaking_change, run_check};
use crate::classify::{ChangedItem, PackageClass, PackageStatus, classify};
use crate::command::hash_bytes;
use crate::groups::GroupVerdict;
use crate::manifest::requirement_names_version;
use crate::metadata::{WorkTree, load_tracked_work_tree};
use crate::plan::{
    IncrementLevel, PlanFile, PlanIncrement, PlanStage, ResolvedVersions, SCHEMA_VERSION,
    increment_version, resolve_plan,
};
use crate::prospective::Prospective;
use crate::report::write_report;
use crate::resolved::{Artifact, Inputs, ResolvedState, canonical, read_json, write_json};
use crate::verbose::Verbose;

/// Post-refresh workspace inputs captured before semantic grading.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    schema_version: u32,
    inputs: Inputs,
}

pub(crate) fn run_prepare(
    output: &Path,
    base: Option<&str>,
    manifest: &Path,
    verbose: Verbose,
) -> Result<String, AppError> {
    let output = absolute(output).map_err(|error| WriteFileError::caused_by(output, error))?;
    let manifest = canonical(manifest)?;
    let inputs = Inputs::capture(&manifest, base)?;
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
    let inputs = Inputs::capture(&manifest, base)?;
    let classification = classify(&manifest, Some(&inputs.base), verbose)?;
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

pub(crate) fn run_preview(
    plan: &Path,
    prepared: &Path,
    output: &Path,
    manifest: &Path,
    verbose: Verbose,
) -> Result<String, AppError> {
    let output = absolute(output).map_err(|error| WriteFileError::caused_by(output, error))?;
    let (prepared, plan) = preview_inputs(plan, prepared, &output, manifest, |inputs| {
        inputs.verify(manifest, None).map(|_| ())
    })?;
    let prospective = Prospective::new(&output, &prepared.inputs)?;
    let mut classification = classify(&prospective.manifest, Some(&prepared.inputs.base), verbose)?;
    let resolved = resolve_plan(
        &plan,
        &classification.work_tree.groups,
        &classification.work_tree.target_versions(),
        verbose,
    )?;
    require_semantic_decisions(&classification.packages, &resolved)?;

    let (resolved, files) = resolve_until_stable(resolved, &prospective.root, |resolved| {
        let (work_tree, _) = load_tracked_work_tree(&prospective.manifest)?;
        for edit in compute_edits(&work_tree, resolved, verbose)? {
            if edit.original != edit.updated {
                fs::write(&edit.path, edit.updated)
                    .map_err(|error| WriteFileError::caused_by(&edit.path, error))?;
            }
        }
        prospective.resolve(verbose)?;
        classification = classify(&prospective.manifest, Some(&prepared.inputs.base), verbose)?;
        let files = prospective.artifacts(&prepared.inputs)?;
        let mut expanded = resolved.clone();
        add_consequences(
            &classification.packages,
            &classification.groups,
            &classification.work_tree,
            &mut expanded,
        )?;
        Ok((expanded, files))
    })?;
    let (passed, message, _) = run_check(
        Some(&prepared.inputs.base),
        &prospective.manifest,
        CheckFormat::Text,
        false,
        verbose,
    )?;
    require_complete_preview(passed, message)?;
    prepared.inputs.verify(manifest, None)?;
    let final_digest = prepared.inputs.final_digest(&files)?;
    let evidence_manifest_path = prospective.retain(&output, prepared.inputs.root())?;
    prepared
        .inputs
        .verify_candidate(&evidence_manifest_path, &final_digest)?;
    let mut plan = explicit_plan(&resolved);
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

fn resolve_until_stable(
    mut resolved: ResolvedVersions,
    root: &Path,
    mut resolve: impl FnMut(&ResolvedVersions) -> Result<(ResolvedVersions, Vec<Artifact>), AppError>,
) -> Result<(ResolvedVersions, Vec<Artifact>), AppError> {
    // The callback owns rewriting, offline resolution and recapture; this loop owns convergence.
    // Ref: docs/implementation.md, "Prepared and prospective resolution".
    let mut visited = BTreeSet::new();
    let mut previous_files = Vec::new();
    loop {
        let (expanded, files) = resolve(&resolved)?;
        if expanded == resolved && files == previous_files {
            return Ok((resolved, files));
        }
        // Remember actual states rather than imposing an arbitrary iteration deadline.
        record_state(&mut visited, &expanded, &files, root)?;
        previous_files = files;
        resolved = expanded;
    }
}

fn preview_inputs(
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
    root: &Path,
) -> Result<(), AppError> {
    let state = serde_json::to_vec(&(explicit_plan(resolved), files))
        .expect("version plans and artifacts contain only JSON-compatible data");
    // Keep a bounded digest per iteration, not another retained copy of every resolved file.
    // Hash the complete state so a version, path, or content change cannot look like a cycle.
    if !visited.insert(hash_bytes(&state, root)?) {
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
    work_tree: &WorkTree,
    resolved: &mut ResolvedVersions,
) -> Result<(), AppError> {
    let versions = work_tree.target_versions();
    for (name, group) in groups {
        if !group.is_consistent() {
            raise(work_tree, resolved, name, group.version());
        }
    }
    for package in packages {
        if package.status() == PackageStatus::NeedsIncrement {
            let anchor = package.anchor().expect("needs-increment has an anchor");
            raise(
                work_tree,
                resolved,
                &package.name,
                &increment_version(&anchor.version, IncrementLevel::Patch)?,
            );
        }
        for dependency in &package.dependencies {
            let Some(version) = versions.get(&dependency.name) else {
                continue;
            };
            if !requirement_names_version(&dependency.req, version) {
                // Explicitly retaining the target version also schedules its requirement rewrites.
                raise(work_tree, resolved, &dependency.name, version);
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
                let level = if anchor.version.major == 0 {
                    IncrementLevel::Minor
                } else {
                    IncrementLevel::Major
                };
                raise(
                    work_tree,
                    resolved,
                    &package.name,
                    &increment_version(&anchor.version, level)?,
                );
            }
        }
    }
    for dependency in &work_tree.exact_dependencies {
        if let Some(version) = versions.get(&dependency.target)
            && !requirement_names_version(&dependency.requirement, version)
        {
            raise(work_tree, resolved, &dependency.target, version);
        }
    }
    Ok(())
}

fn raise(work_tree: &WorkTree, resolved: &mut ResolvedVersions, target: &str, minimum: &Version) {
    let groups = &work_tree.groups;
    let group = groups.group_of(target).unwrap_or(target);
    let mut members = groups.members(group).to_vec();
    if members.is_empty() {
        members.push(target.to_owned());
    }
    let versions = work_tree.target_versions();
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
                level: None,
                version: Some(version.to_string()),
            })
            .collect(),
    )
}

pub(crate) fn remove_marker(path: &Path) -> Result<(), AppError> {
    if path.exists() {
        fs::remove_file(path).map_err(|error| WriteFileError::caused_by(path, error))?;
    }
    Ok(())
}

fn guard_output_inputs(output: &Path, inputs: &[&Path]) -> Result<(), AppError> {
    let output = resolve_path(output)?;
    for input in inputs {
        for name in ["report.json", "report.json.tmp"] {
            if same_path(input, &output.join(name))? {
                return Err(OutputInputCollision::new().into());
            }
        }
        let input = resolve_path(input)?;
        for directory in ["diffs", "workspace", ".prospective"] {
            if input.starts_with(output.join(directory)) {
                return Err(OutputInputCollision::new().into());
            }
        }
    }
    Ok(())
}

/// Completion artifacts must not replace the invocation's own input documents.
#[ohno::error]
#[display("preview output overlaps an input; choose a separate output location")]
struct OutputInputCollision;

/// Source-level release grading belongs to the caller, not the resolver.
#[ohno::error]
#[display("package {package} needs a semantic release decision in the proposed plan")]
struct SemanticDecisionRequired {
    package: String,
}

/// Prepared bytes must remain the state captured before grading.
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
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::iter;
    use std::path::PathBuf;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::*;
    use crate::ParsePlanError;
    use crate::anchor::Anchor;
    use crate::groups::Groups;
    use crate::lockfile::InstallationGraph;
    use crate::metadata::{DepKind, ExactDependency, ReportedDep, VersionTarget};
    use crate::resolved::StaleInputs;

    #[test]
    #[cfg_attr(
        miri,
        ignore = "captures owned files and hashes convergence states with Git"
    )]
    fn resolution_waits_for_stable_captured_files_without_changing_versions() {
        let directory = tempdir().unwrap();
        let lockfile = directory.path().join("Cargo.lock");
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
        let (resolved, files) =
            resolve_until_stable(initial.clone(), directory.path(), |resolved| {
                assert_eq!(resolved, &initial);
                fs::write(&lockfile, writes.next().unwrap()).unwrap();
                Ok((
                    resolved.clone(),
                    vec![Artifact {
                        path: "Cargo.lock".into(),
                        contents: fs::read_to_string(&lockfile).unwrap(),
                    }],
                ))
            })
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
    #[cfg_attr(miri, ignore = "uses Git to hash convergence states")]
    fn resolution_applies_new_versions_even_when_captured_files_are_stable() {
        let directory = tempdir().unwrap();
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
        let (resolved, captured) =
            resolve_until_stable(initial.clone(), directory.path(), |resolved| {
                let (expected, next) = passes.next().unwrap();
                assert_eq!(resolved, expected);
                Ok((next.clone(), files.clone()))
            })
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
        let (resolved, files) =
            resolve_until_stable(initial.clone(), Path::new("unused"), |resolved| {
                passes.next().unwrap();
                Ok((resolved.clone(), Vec::new()))
            })
            .unwrap();
        assert!(passes.next().is_none());
        assert_eq!(resolved, initial);
        assert!(files.is_empty());
    }

    #[test]
    #[cfg_attr(miri, ignore = "uses Git to hash convergence states")]
    fn resolution_rejects_a_captured_file_cycle_without_accepting_stable_versions() {
        let directory = tempdir().unwrap();
        let initial = ResolvedVersions {
            packages: BTreeMap::new(),
        };
        let mut contents = ["first resolution", "other resolution", "first resolution"].into_iter();
        let error = resolve_until_stable(initial.clone(), directory.path(), |resolved| {
            assert_eq!(resolved, &initial);
            Ok((
                resolved.clone(),
                vec![Artifact {
                    path: "Cargo.lock".into(),
                    contents: contents.next().unwrap().to_owned(),
                }],
            ))
        })
        .unwrap_err();
        assert!(error.find_source::<ResolutionCycle>().is_some());
        assert!(contents.next().is_none());
    }

    #[test]
    #[cfg_attr(miri, ignore = "uses Git to hash convergence states")]
    fn resolution_propagates_a_failed_pass_instead_of_accepting_previous_files() {
        let directory = tempdir().unwrap();
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
        let error = resolve_until_stable(initial.clone(), directory.path(), |resolved| {
            assert_eq!(resolved, &initial);
            passes
                .next()
                .unwrap()
                .map(|files| (resolved.clone(), files))
        })
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
            groups: Groups::default(),
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
        work_tree.groups = Groups::from_edges(
            ["core".to_owned(), "helper".to_owned()],
            [("helper".to_owned(), "core".to_owned())],
        );
        let verdict = GroupVerdict::new(
            work_tree.groups.members("core"),
            &work_tree.target_versions(),
            &HashSet::new(),
        );
        let groups = BTreeMap::from([("core".to_owned(), verdict)]);
        let mut resolved = ResolvedVersions {
            packages: BTreeMap::new(),
        };
        add_consequences(&packages[..1], &groups, &work_tree, &mut resolved).unwrap();
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
        add_consequences(&packages[..1], &groups, &work_tree, &mut resolved).unwrap();
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
        work_tree.groups = Groups::from_edges(
            ["core".to_owned(), "helper".to_owned()],
            [("helper".to_owned(), "core".to_owned())],
        );
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
                work_tree.groups.members("core"),
                &work_tree.target_versions(),
                &HashSet::new(),
            ),
        )]);
        let mut resolved = ResolvedVersions {
            packages: BTreeMap::new(),
        };

        // Unpublished members have no release assessments, but their exact requirements
        // must still be rewritten. Matching requirements must not invent plan entries.
        add_consequences(&[], &groups, &work_tree, &mut resolved).unwrap();
        assert!(resolved.packages.is_empty());

        work_tree
            .exact_dependencies
            .first_mut()
            .unwrap()
            .requirement = "=0.1.0".to_owned();
        add_consequences(&[], &groups, &work_tree, &mut resolved).unwrap();
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

    fn prepared_document() -> String {
        json!({
            "schema_version": SCHEMA_VERSION,
            "inputs": {
                "root": "repository", "manifest": "Cargo.toml", "head": "head", "base": "base",
                "base_revision": "main", "index": "index", "paths": ["Cargo.toml"], "digest": "initial"
            }
        })
        .to_string()
    }

    #[test]
    #[cfg_attr(miri, ignore = "uses owned preview artifact files")]
    fn failed_input_reads_and_verification_invalidate_the_previous_completion_marker() {
        let directory = tempdir().unwrap();
        let output = directory.path();
        let marker = output.join("plan.json");
        let prepared = output.join("prepared.json");
        let proposal = output.join("proposal.json");
        let manifest = output.join("Cargo.toml");
        let verified = Cell::new(false);
        fs::write(&proposal, "{ invalid plan").unwrap();
        fs::write(&prepared, "{ invalid preparation").unwrap();
        fs::write(&marker, "previous completion").unwrap();
        let error = preview_inputs(&proposal, &prepared, output, &manifest, |_| {
            verified.set(true);
            Ok(())
        })
        .err()
        .unwrap();
        assert!(error.find_source::<ParsePlanError>().is_some());
        assert!(!verified.get());
        assert!(!marker.exists());

        fs::write(&prepared, prepared_document()).unwrap();
        fs::write(&marker, "previous completion").unwrap();
        let error = preview_inputs(&proposal, &prepared, output, &manifest, |_| {
            assert!(!marker.exists());
            verified.set(true);
            Err(StaleInputs::new().into())
        })
        .err()
        .unwrap();
        // Staleness wins over the malformed proposal, preserving input acquisition order.
        assert!(error.find_source::<StaleInputs>().is_some());
        assert!(verified.get());
        assert!(!marker.exists());

        fs::write(&marker, "previous completion").unwrap();
        let error = preview_inputs(&proposal, &prepared, output, &manifest, |inputs| {
            assert!(!marker.exists());
            assert_eq!(inputs.head, "head");
            Ok(())
        })
        .err()
        .unwrap();
        assert!(error.find_source::<ParsePlanError>().is_some());
        assert!(!marker.exists());

        fs::write(&proposal, r#"{"schema_version":4,"increments":[]}"#).unwrap();
        let (_, plan) =
            preview_inputs(&proposal, &prepared, output, &manifest, |_| Ok(())).unwrap();
        assert!(plan.increments.is_empty());
        assert!(!marker.exists());
    }

    #[test]
    #[cfg_attr(miri, ignore = "checks owned filesystem output aliases")]
    fn preview_collisions_preserve_inputs_and_never_acquire_repository_state() {
        let directory = tempdir().unwrap();
        let output = directory.path().join("preview");
        fs::create_dir_all(&output).unwrap();
        for relative in [
            "plan.json",
            "report.json",
            "report.json.tmp",
            "diffs/proposal.json",
            "workspace/proposal.json",
            ".prospective/proposal.json",
        ] {
            let input = output.join(relative);
            fs::create_dir_all(input.parent().unwrap()).unwrap();
            fs::write(&input, "input document").unwrap();
            for position in 0..3 {
                let unrelated = directory.path().join("unrelated");
                let mut inputs = [&unrelated, &unrelated, &unrelated];
                *inputs.get_mut(position).unwrap() = &input;
                let error = preview_inputs(inputs[0], inputs[1], &output, inputs[2], |_| {
                    panic!("input collisions must be rejected before repository acquisition")
                })
                .err()
                .unwrap();
                assert!(error.find_source::<OutputInputCollision>().is_some());
                assert_eq!(fs::read_to_string(&input).unwrap(), "input document");
            }
        }
        let input = output.join("plan.json");
        fs::write(&input, "input document").unwrap();
        let alias = directory.path().join("missing/../preview");
        let error = preview_inputs(&input, &input, &alias, &input, |_| {
            panic!("output aliases must be rejected before repository acquisition")
        })
        .err()
        .unwrap();
        assert!(error.find_source::<OutputInputCollision>().is_some());
        assert_eq!(fs::read_to_string(input).unwrap(), "input document");
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads an owned prepared artifact")]
    fn preparation_does_not_accept_alternative_resolution_artifacts() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("prepared.json");
        let mut prepared: Value = serde_json::from_str(&prepared_document()).unwrap();
        prepared.as_object_mut().unwrap().insert(
            "files".to_owned(),
            json!([{"path":"Cargo.lock","contents":"alternative resolution"}]),
        );
        fs::write(&path, prepared.to_string()).unwrap();
        let error = read_json::<Prepared>(&path).err().unwrap();
        assert!(error.find_source::<ParsePlanError>().is_some());
    }

    #[test]
    #[cfg_attr(miri, ignore = "checks an owned preview marker directory")]
    fn occupied_completion_marker_precedes_input_acquisition() {
        let directory = tempdir().unwrap();
        let marker = directory.path().join("plan.json/keep");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, "not a completion file").unwrap();
        let absent = directory.path().join("absent");
        let error = preview_inputs(&absent, &absent, directory.path(), &absent, |_| {
            panic!("an occupied marker must fail before repository acquisition")
        })
        .err()
        .unwrap();
        assert!(error.find_source::<WriteFileError>().is_some());
        assert_eq!(fs::read_to_string(marker).unwrap(), "not a completion file");
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
    #[cfg_attr(miri, ignore = "uses Git to hash convergence states")]
    fn history_detects_repetition_without_retaining_artifact_contents() {
        let directory = tempdir().unwrap();
        let mut visited = BTreeSet::new();
        let mut resolved = ResolvedVersions {
            packages: BTreeMap::from([("tool".to_owned(), Version::new(1, 0, 0))]),
        };
        let mut files = [Artifact {
            path: "Cargo.lock".into(),
            contents: "initial resolution".to_owned(),
        }];
        record_state(&mut visited, &resolved, &files, directory.path()).unwrap();
        let key_length = visited.first().unwrap().len();
        let error = record_state(&mut visited, &resolved, &files, directory.path()).unwrap_err();
        assert!(error.find_source::<ResolutionCycle>().is_some());
        assert_eq!(visited.len(), 1);

        resolved
            .packages
            .insert("tool".to_owned(), Version::new(1, 0, 1));
        record_state(&mut visited, &resolved, &files, directory.path()).unwrap();
        files[0].path = "Cargo.toml".into();
        record_state(&mut visited, &resolved, &files, directory.path()).unwrap();
        // Larger content establishes that retained key size is independent of artifact size.
        files[0].contents = "resolved dependency\n".repeat(1024);
        record_state(&mut visited, &resolved, &files, directory.path()).unwrap();
        assert_eq!(visited.len(), 4);
        assert!(visited.iter().all(|key| key.len() == key_length));
    }
}
