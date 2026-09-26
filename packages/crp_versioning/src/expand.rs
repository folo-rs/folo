// `expand` command: turn a proposed plan into an expanded plan.
//
// A proposed plan may name a version group, or one member of it, instead of
// every package whose version the release decision sets. This command resolves
// it and writes the explicit package/version set without running resolution.
// Preview must capture the remaining effects before an expanded plan can be applied.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use crp_diag::{Verbose, plural};
use crp_workspace::artifact_path::same_path;
use crp_workspace::metadata::{WorkTree, load_tracked_work_tree};
use ohno::AppError;
use serde::Serialize;
use tempfile::NamedTempFile;

use crate::groups::Groups;
use crate::plan::{PlanFile, SCHEMA_VERSION, resolve_plan};
use crate::resolved::read_json;
use crate::{CreateOutputDirectoryError, WriteFileError, quote_path};

/// On-disk body of an expanded plan.
///
/// Every entry carries an explicit
/// version because resolution has already applied the increment to the group's
/// highest declared member version. The `expanded` stamp records the planning
/// stage, which is what holds the document to the package set it names instead
/// of letting the derived group of the day widen it. No resolved artifact is
/// attached here because this operation is deliberately read-only.
#[derive(Serialize)]
struct ExpandedPlanFile {
    schema_version: u32,
    expanded: bool,
    increments: Vec<ExpandedPackageVersion>,
}

/// One package's resolved version within an expanded plan.
#[derive(Serialize)]
struct ExpandedPackageVersion {
    name: String,
    version: String,
}

// This adapter only selects filesystem acquisition; the shared core owns command ordering,
// collision rejection and rendering, which remain in-process unit-test responsibilities.
#[cfg_attr(test, mutants::skip)]
pub fn run_expand(
    plan_path: &Path,
    out_path: &Path,
    manifest_path: &Path,
    preserve_input: bool,
    verbose: Verbose<'_>,
) -> Result<String, AppError> {
    expand(
        plan_path,
        out_path,
        manifest_path,
        preserve_input,
        verbose,
        &mut FileArtifacts,
    )
}

fn expand(
    plan_path: &Path,
    out_path: &Path,
    manifest_path: &Path,
    preserve_input: bool,
    verbose: Verbose<'_>,
    artifacts: &mut impl ExpansionArtifacts,
) -> Result<String, AppError> {
    if preserve_input && artifacts.same_path(plan_path, out_path)? {
        return Err(ExpansionInputCollision::new().into());
    }
    let plan = artifacts.read_plan(plan_path)?;

    let work_tree = artifacts.workspace(manifest_path)?;
    // Every Git-tracked member is a valid version target, and a group increments
    // from the highest version any of its members declares.
    // Ref: docs/implementation.md, "Plan resolution and application".
    let target_versions = work_tree.target_versions();
    let resolved = resolve_plan(
        &plan,
        &Groups::from_workspace(&work_tree),
        &target_versions,
        verbose,
    )?;

    verbose.note(|| {
        format!(
            "{} named {} and expands to {}; any difference is group members the plan did not name",
            quote_path(&plan_path.to_string_lossy()),
            plural(plan.increments.len(), "increment"),
            plural(resolved.packages.len(), "package version")
        )
    });

    let document = ExpandedPlanFile {
        schema_version: SCHEMA_VERSION,
        expanded: true,
        increments: resolved
            .packages
            .iter()
            .map(|(name, version)| ExpandedPackageVersion {
                name: name.clone(),
                version: version.to_string(),
            })
            .collect(),
    };
    let mut json = serde_json::to_string_pretty(&document)
        .expect("an expanded plan holds only strings and a number, which always serialize");
    json.push('\n');
    artifacts.write(out_path, &json, preserve_input)?;

    Ok(format!(
        "Expanded {} to {}",
        plural(resolved.packages.len(), "package version"),
        quote_path(&out_path.to_string_lossy())
    ))
}

/// Acquisition and publication required by expansion, separate from its decision ordering.
trait ExpansionArtifacts {
    fn same_path(&mut self, left: &Path, right: &Path) -> Result<bool, AppError>;
    fn read_plan(&mut self, path: &Path) -> Result<PlanFile, AppError>;
    fn workspace(&mut self, manifest: &Path) -> Result<WorkTree, AppError>;
    fn write(&mut self, path: &Path, json: &str, preserve_input: bool) -> Result<(), AppError>;
}

/// Connects expansion to the real filesystem and tracked Cargo workspace.
struct FileArtifacts;

// Real acquisition is integration-owned; the core above retains every expansion decision.
#[cfg_attr(test, mutants::skip)]
impl ExpansionArtifacts for FileArtifacts {
    fn same_path(&mut self, left: &Path, right: &Path) -> Result<bool, AppError> {
        same_path(left, right)
    }

    fn read_plan(&mut self, path: &Path) -> Result<PlanFile, AppError> {
        read_json(path)
    }

    fn workspace(&mut self, manifest: &Path) -> Result<WorkTree, AppError> {
        load_tracked_work_tree(manifest).map(|(work_tree, _)| work_tree)
    }

    fn write(&mut self, path: &Path, json: &str, preserve_input: bool) -> Result<(), AppError> {
        write_expansion(path, json, preserve_input)
    }
}

pub fn write_expansion(out_path: &Path, json: &str, preserve_input: bool) -> Result<(), AppError> {
    // A bare filename has an empty parent and therefore needs no directory creation.
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|error| CreateOutputDirectoryError::caused_by(parent, error))?;
    }
    if preserve_input {
        // Exclusive temporary-file creation cannot overwrite another input or another run's
        // staging file. Promotion is the only operation that replaces the requested destination.
        let staged = stage_expansion(out_path, json)?;
        _ = staged
            .persist(out_path)
            .map_err(|error| WriteFileError::caused_by(out_path, error.error))?;
    } else {
        // The general command retains its existing in-place and symlink-following behavior.
        fs::write(out_path, json.as_bytes())
            .map_err(|error| WriteFileError::caused_by(out_path, error))?;
    }
    Ok(())
}

pub fn stage_expansion(out_path: &Path, json: &str) -> Result<NamedTempFile, AppError> {
    let parent = out_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut staged = NamedTempFile::new_in(parent)
        .map_err(|error| WriteFileError::caused_by(out_path, error))?;
    staged
        .write_all(json.as_bytes())
        .map_err(|error| WriteFileError::caused_by(out_path, error))?;
    Ok(staged)
}

/// Protected expansion may not replace the artifact it reads.
#[ohno::error]
#[display("expansion output overlaps its input; choose a separate output location")]
struct ExpansionInputCollision;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::classify::PackageStatus;
    use crate::classify::fixture::{classification, package};
    use crate::plan::{PlanIncrement, PlanStage};

    #[test]
    fn expansion_protects_inputs_before_acquisition_only_when_requested() {
        let mut artifacts = ArtifactObservations {
            collision: true,
            ..ArtifactObservations::default()
        };
        let error = expand(
            Path::new("proposal.json"),
            Path::new("expanded.json"),
            Path::new("Cargo.toml"),
            true,
            Verbose::new(false, &crp_diag::Discard),
            &mut artifacts,
        )
        .unwrap_err();
        assert!(error.find_source::<ExpansionInputCollision>().is_some());
        assert_eq!(artifacts.calls, ["compare"]);
    }

    #[test]
    fn expansion_acquires_resolves_renders_and_publishes_in_order() {
        for preserve in [false, true] {
            let expected = if preserve {
                vec!["compare", "read", "workspace", "write"]
            } else {
                vec!["read", "workspace", "write"]
            };
            for failure in std::iter::once(None).chain((0..expected.len()).map(Some)) {
                let mut artifacts = ArtifactObservations {
                    failure,
                    preserve,
                    ..ArtifactObservations::default()
                };
                let result = expand(
                    Path::new("proposal.json"),
                    Path::new("expanded.json"),
                    Path::new("Cargo.toml"),
                    preserve,
                    Verbose::new(false, &crp_diag::Discard),
                    &mut artifacts,
                );
                if let Some(index) = failure {
                    assert!(
                        result
                            .unwrap_err()
                            .find_source::<ArtifactFailure>()
                            .is_some()
                    );
                    assert_eq!(
                        artifacts.calls,
                        expected.iter().take(index + 1).copied().collect::<Vec<_>>()
                    );
                } else {
                    let message = result.unwrap();
                    assert!(message.contains("1 package version"));
                    assert!(message.contains("expanded.json"));
                    assert_eq!(artifacts.calls, expected);
                }
            }
        }
    }

    /// Records expansion observations and failures without acquiring external resources.
    #[derive(Default)]
    struct ArtifactObservations {
        calls: Vec<&'static str>,
        failure: Option<usize>,
        collision: bool,
        preserve: bool,
    }

    impl ArtifactObservations {
        fn visit(&mut self, operation: &'static str) -> Result<(), AppError> {
            let index = self.calls.len();
            self.calls.push(operation);
            if self.failure == Some(index) {
                return Err(ArtifactFailure::new().into());
            }
            Ok(())
        }
    }

    impl ExpansionArtifacts for ArtifactObservations {
        fn same_path(&mut self, left: &Path, right: &Path) -> Result<bool, AppError> {
            assert_eq!(left, Path::new("proposal.json"));
            assert_eq!(right, Path::new("expanded.json"));
            self.visit("compare")?;
            Ok(self.collision)
        }

        fn read_plan(&mut self, path: &Path) -> Result<PlanFile, AppError> {
            assert_eq!(path, Path::new("proposal.json"));
            self.visit("read")?;
            Ok(PlanFile::new(
                PlanStage::Proposed,
                vec![PlanIncrement {
                    name: "library".to_owned(),
                    level: Some("patch".to_owned()),
                    version: None,
                }],
            ))
        }

        fn workspace(&mut self, manifest: &Path) -> Result<WorkTree, AppError> {
            assert_eq!(manifest, Path::new("Cargo.toml"));
            self.visit("workspace")?;
            Ok(classification(vec![package("library", PackageStatus::Unchanged, "")]).work_tree)
        }

        fn write(&mut self, path: &Path, json: &str, preserve_input: bool) -> Result<(), AppError> {
            assert_eq!(path, Path::new("expanded.json"));
            assert_eq!(preserve_input, self.preserve);
            assert_eq!(
                serde_json::from_str::<Value>(json).unwrap(),
                json!({
                    "schema_version": SCHEMA_VERSION, "expanded": true,
                    "increments": [{"name": "library", "version": "1.0.1"}]
                })
            );
            assert!(json.ends_with('\n'));
            self.visit("write")
        }
    }

    /// An injected expansion operation failure, without platform-dependent permissions.
    #[ohno::error]
    struct ArtifactFailure;
}
