// `expand` command: resolve a plan's version groups into per-package versions.
//
// `apply` expands groups internally, which leaves a planner unable to show which
// packages a plan actually moves until after it has run. This command exposes
// that same expansion as a document: it names every package the plan reaches,
// including group members the plan did not mention, at the version each will
// carry. The result is itself a plan, so the reviewed document is the one that
// gets applied rather than a rendering of it.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use ohno::AppError;
use semver::Version;
use serde::Serialize;

use crate::metadata::load_tracked_work_tree;
use crate::plan::{PlanFile, SCHEMA_VERSION, expand_plan};
use crate::text::plural;
use crate::verbose::Verbose;
use crate::{ParsePlanError, ReadFileError, WriteFileError, quote_path};

/// On-disk expanded plan body.
///
/// Deliberately the same shape `apply` reads, so the expanded document can be
/// applied directly. Every entry carries an explicit version because expansion
/// has already resolved the increment against the group's highest declared
/// member version.
#[derive(Serialize)]
struct ExpandedPlanFile {
    schema_version: u32,
    increments: Vec<ExpandedIncrement>,
}

/// One package's resolved version in an expanded plan.
#[derive(Serialize)]
struct ExpandedIncrement {
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
    // Same target and increment-base rules as `apply`: only Git-tracked
    // publishable members are valid targets, and a group increments from the
    // highest version any of its members declares.
    // Ref: docs/implementation.md, "Plan application".
    let publishable: BTreeMap<String, Version> = work_tree
        .packages
        .iter()
        .map(|package| {
            (
                package.manifest.name.clone(),
                package.manifest.version.clone(),
            )
        })
        .collect();
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
            .map(|(name, version)| ExpandedIncrement {
                name: name.clone(),
                version: version.to_string(),
            })
            .collect(),
    };
    let json = serde_json::to_string_pretty(&document)
        .expect("an expanded plan holds only strings and a number, which always serialize");
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| WriteFileError::caused_by(parent, error))?;
    }
    fs::write(out_path, format!("{json}\n"))
        .map_err(|error| WriteFileError::caused_by(out_path, error))?;

    Ok(format!(
        "Expanded {} to {}",
        plural(expanded.packages.len(), "package version"),
        quote_path(&out_path.to_string_lossy())
    ))
}
