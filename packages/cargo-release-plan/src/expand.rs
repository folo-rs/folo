// `expand` command: write the explicit package/version plan reviewers apply.
//
// Input plans may name a version group or one member of it instead of every
// package the release decision reaches. This command writes the complete
// explicit package/version set, so reviewers see the plan that `apply` consumes.

use std::fs;
use std::path::Path;

use ohno::AppError;
use serde::Serialize;

use crate::metadata::load_tracked_work_tree;
use crate::plan::{PlanFile, SCHEMA_VERSION, expand_plan};
use crate::text::plural;
use crate::verbose::Verbose;
use crate::{
    CreateOutputDirectoryError, ParsePlanError, ReadFileError, WriteFileError, quote_path,
};

/// On-disk expanded plan body.
///
/// The expanded document is itself a plan that can be applied directly after
/// review. Every entry carries an explicit version because expansion has already
/// resolved the increment against the group's highest declared member version.
#[derive(Serialize)]
struct ExpandedPlanFile {
    schema_version: u32,
    increments: Vec<ExpandedPackageVersion>,
}

/// One package's resolved version in an expanded plan.
#[derive(Serialize)]
struct ExpandedPackageVersion {
    name: String,
    version: String,
}

pub(crate) fn run_expand(
    plan_path: &Path,
    out_path: &Path,
    manifest_path: &Path,
    verbose: Verbose,
) -> Result<String, AppError> {
    let plan = fs::read_to_string(plan_path)
        .map_err(|error| ReadFileError::caused_by(plan_path, error))?;
    let plan: PlanFile =
        serde_json::from_str(&plan).map_err(|error| ParsePlanError::caused_by(plan_path, error))?;

    let (work_tree, _) = load_tracked_work_tree(manifest_path)?;
    // Only Git-tracked publishable members are valid targets, and a group
    // increments from the highest version any of its members declares.
    // Ref: docs/implementation.md, "Plan expansion and application".
    let publishable = work_tree.publishable_versions();
    let expanded = expand_plan(&plan, &work_tree.groups, &publishable, verbose)?;

    verbose.note(|| {
        format!(
            "{} named {} and expands to {}; any difference is group members the plan did not name",
            quote_path(&plan_path.to_string_lossy()),
            plural(plan.increments.len(), "increment"),
            plural(expanded.packages.len(), "package version")
        )
    });

    let document = ExpandedPlanFile {
        schema_version: SCHEMA_VERSION,
        increments: expanded
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
    // A bare filename has an empty parent and therefore needs no directory creation.
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|error| CreateOutputDirectoryError::caused_by(parent, error))?;
    }
    fs::write(out_path, json.as_bytes())
        .map_err(|error| WriteFileError::caused_by(out_path, error))?;

    Ok(format!(
        "Expanded {} to {}",
        plural(expanded.packages.len(), "package version"),
        quote_path(&out_path.to_string_lossy())
    ))
}
