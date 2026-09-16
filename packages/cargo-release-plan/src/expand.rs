// `expand` command: turn a proposed plan into an expanded plan.
//
// A proposed plan may name a version group, or one member of it, instead of
// every package whose version the release decision sets. This command resolves
// it and writes the explicit package/version set without running resolution.
// Preview must capture the remaining effects before an expanded plan can be applied.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use ohno::AppError;
use serde::Serialize;
use tempfile::NamedTempFile;

use crate::artifact_path::same_path;
use crate::metadata::load_tracked_work_tree;
use crate::plan::{PlanFile, SCHEMA_VERSION, resolve_plan};
use crate::resolved::read_json;
use crate::text::plural;
use crate::verbose::Verbose;
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

pub(crate) fn run_expand(
    plan_path: &Path,
    out_path: &Path,
    manifest_path: &Path,
    preserve_input: bool,
    verbose: Verbose,
) -> Result<String, AppError> {
    if preserve_input && same_path(plan_path, out_path)? {
        return Err(ExpansionInputCollision::new().into());
    }
    let plan: PlanFile = read_json(plan_path)?;

    let (work_tree, _) = load_tracked_work_tree(manifest_path)?;
    // Every Git-tracked member is a valid version target, and a group increments
    // from the highest version any of its members declares.
    // Ref: docs/implementation.md, "Plan resolution and application".
    let target_versions = work_tree.target_versions();
    let resolved = resolve_plan(&plan, &work_tree.groups, &target_versions, verbose)?;

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
    write_expansion(out_path, &json, preserve_input)?;

    Ok(format!(
        "Expanded {} to {}",
        plural(resolved.packages.len(), "package version"),
        quote_path(&out_path.to_string_lossy())
    ))
}

fn write_expansion(out_path: &Path, json: &str, preserve_input: bool) -> Result<(), AppError> {
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

fn stage_expansion(out_path: &Path, json: &str) -> Result<NamedTempFile, AppError> {
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
    use tempfile::tempdir;

    use super::*;
    use crate::ParsePlanError;

    #[test]
    #[cfg_attr(miri, ignore = "creates missing output directories")]
    fn expansion_creates_missing_output_parents_in_both_modes() {
        let directory = tempdir().unwrap();
        for preserve_input in [false, true] {
            let output = directory
                .path()
                .join(preserve_input.to_string())
                .join("nested")
                .join("expanded.json");
            write_expansion(&output, "complete", preserve_input).unwrap();
            assert_eq!(fs::read_to_string(output).unwrap(), "complete");
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "stages files beside the requested output")]
    fn staging_is_complete_and_beside_the_untouched_destination() {
        let directory = tempdir().unwrap();
        let output = directory.path().join("expanded.json");
        fs::write(&output, "previous").unwrap();
        let staged = stage_expansion(&output, "complete").unwrap();
        assert_eq!(
            fs::canonicalize(staged.path().parent().unwrap()).unwrap(),
            fs::canonicalize(directory.path()).unwrap()
        );
        assert_ne!(staged.path(), output);
        assert_eq!(fs::read_to_string(staged.path()).unwrap(), "complete");
        assert_eq!(fs::read_to_string(&output).unwrap(), "previous");
        drop(staged);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads proposal files without acquiring a workspace")]
    fn accepted_destinations_reach_plan_validation_in_both_modes() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("plan.json");
        fs::write(&input, "invalid plan, retained").unwrap();
        for (preserve_input, output) in [
            (true, directory.path().join("expanded.json")),
            (false, directory.path().join("expanded.json")),
            (false, input.clone()),
        ] {
            let error = run_expand(
                &input,
                &output,
                Path::new("unused.toml"),
                preserve_input,
                Verbose::new(false),
            )
            .unwrap_err();
            assert!(error.find_source::<ParsePlanError>().is_some());
            assert_eq!(
                fs::read_to_string(&input).unwrap(),
                "invalid plan, retained"
            );
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore = "stages files on the host filesystem")]
    fn protected_writes_promote_complete_output_and_remove_staging_files() {
        let directory = tempdir().unwrap();
        let output = directory.path().join("expanded.json");
        fs::write(&output, "previous").unwrap();
        write_expansion(&output, "complete", true).unwrap();
        assert_eq!(fs::read_to_string(&output).unwrap(), "complete");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore = "exercises a failed filesystem promotion")]
    fn failed_promotion_preserves_the_destination_and_cleans_staging() {
        let directory = tempdir().unwrap();
        let output = directory.path().join("occupied");
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join("input"), "retained").unwrap();
        let error = write_expansion(&output, "complete", true).unwrap_err();
        assert!(error.find_source::<WriteFileError>().is_some());
        assert_eq!(
            fs::read_to_string(output.join("input")).unwrap(),
            "retained"
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "uses an exclusively owned file in the current directory"
    )]
    fn protected_writes_support_bare_filenames() {
        let owned = NamedTempFile::new_in(".").unwrap().into_temp_path();
        let output = Path::new(owned.file_name().unwrap());
        write_expansion(output, "complete", true).unwrap();
        assert_eq!(fs::read_to_string(output).unwrap(), "complete");
    }

    #[test]
    #[cfg_attr(miri, ignore = "protects input paths before writing artifacts")]
    fn protected_expansion_rejects_input_aliases_before_reading() {
        let directory = tempdir().unwrap();
        let input = directory.path().join("plan.json");
        fs::write(&input, "retained").unwrap();
        for output in [
            input.clone(),
            directory
                .path()
                .join("missing")
                .join("..")
                .join("plan.json"),
        ] {
            let error = run_expand(
                &input,
                &output,
                Path::new("unused.toml"),
                true,
                Verbose::new(false),
            )
            .unwrap_err();
            assert!(error.find_source::<ExpansionInputCollision>().is_some());
            assert_eq!(fs::read_to_string(&input).unwrap(), "retained");
        }
    }
}
