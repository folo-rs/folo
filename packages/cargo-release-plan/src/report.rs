// `report` command: write report.json and per-package diffs.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use ohno::AppError;
use serde::Serialize;

use crate::classify::{
    AnchorJson, ChangedItem, Classification, DiffStat, PackageClass, PackageStatus, classify,
};
use crate::metadata::ReportedDep;
use crate::verbose::Verbose;
use crate::{SCHEMA_VERSION, WriteFileError};

/// On-disk `report.json` body.
#[derive(Serialize)]
struct ReportFile {
    schema_version: u32,
    head: String,
    packages: Vec<ReportPackage>,
    groups: BTreeMap<String, ReportGroup>,
}

/// One publishable package in `report.json`.
#[derive(Serialize)]
struct ReportPackage {
    name: String,
    declared_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    status: PackageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor: Option<AnchorJson>,
    changed: Vec<ChangedItem>,
    stat: DiffStat,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff_path: Option<String>,
    dependencies: Vec<ReportedDep>,
    dependents: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    untracked: Vec<String>,
}

/// Version-group consistency as recorded in `report.json`.
#[derive(Serialize)]
struct ReportGroup {
    members: Vec<String>,
    consistent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

pub(crate) fn run_report(
    out_dir: &Path,
    base: &str,
    manifest_path: &Path,
    verbose: Verbose,
) -> Result<String, AppError> {
    let classification = classify(manifest_path, base, verbose)?;
    write_report(out_dir, &classification)
}

pub(crate) fn write_report(
    out_dir: &Path,
    classification: &Classification,
) -> Result<String, AppError> {
    fs::create_dir_all(out_dir).map_err(|error| WriteFileError::caused_by(out_dir, error))?;
    let diffs_dir = out_dir.join("diffs");
    fs::create_dir_all(&diffs_dir).map_err(|error| WriteFileError::caused_by(&diffs_dir, error))?;

    let mut packages = Vec::new();
    for package in &classification.packages {
        let diff_path = write_diff(&diffs_dir, package)?;
        packages.push(report_package(package, diff_path));
    }

    let mut groups = BTreeMap::new();
    for (name, verdict) in &classification.groups {
        groups.insert(
            name.clone(),
            ReportGroup {
                members: verdict.members.clone(),
                consistent: verdict.consistent,
                version: verdict.version.as_ref().map(ToString::to_string),
            },
        );
    }

    let report = ReportFile {
        schema_version: SCHEMA_VERSION,
        head: classification.head.clone(),
        packages,
        groups,
    };
    let json = serde_json::to_string_pretty(&report).expect(
        "ReportFile is built from strings, maps, and numbers that serde_json can serialize",
    );
    let report_path = out_dir.join("report.json");
    fs::write(&report_path, json.as_bytes())
        .map_err(|error| WriteFileError::caused_by(&report_path, error))?;

    let unreleased = classification
        .packages
        .iter()
        .filter(|package| package.status == PackageStatus::UnreleasedChanges)
        .count();
    Ok(format!(
        "Wrote {} ({} with unreleased changes)",
        report_path.display(),
        unreleased
    ))
}

// Patch files are a dump of `package.patch`; bytes are covered by `naive_patch`.
#[cfg_attr(test, mutants::skip)]
fn write_diff(diffs_dir: &Path, package: &PackageClass) -> Result<Option<String>, AppError> {
    if package.patch.is_empty() {
        return Ok(None);
    }
    let rel = format!("diffs/{}.patch", package.name);
    let path = diffs_dir.join(format!("{}.patch", package.name));
    fs::write(&path, package.patch.as_bytes())
        .map_err(|error| WriteFileError::caused_by(&path, error))?;
    Ok(Some(rel))
}

fn report_package(package: &PackageClass, diff_path: Option<String>) -> ReportPackage {
    ReportPackage {
        name: package.name.clone(),
        declared_version: package.declared_version.to_string(),
        group: package.group.clone(),
        status: package.status,
        anchor: package.anchor.as_ref().map(|anchor| AnchorJson {
            commit: anchor.commit.clone(),
            version: anchor.version.to_string(),
        }),
        changed: package.changed.clone(),
        stat: package.stat.clone(),
        diff_path,
        dependencies: package.dependencies.clone(),
        dependents: package.dependents.clone(),
        untracked: package.untracked.clone(),
    }
}
