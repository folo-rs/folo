// Consumer-contract selection is an artifact-only operation shared by CI and planners.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crp_diag::Verbose;
use ohno::AppError;

use crate::classify::PackageStatus;
use crate::quote_path;
use crate::report::{ReportFile, read_report};

// Only the filesystem adapter is excluded; artifact-command integration tests cover it.
#[cfg_attr(test, mutants::skip)]
pub fn run_semver_targets(path: &Path, verbose: Verbose<'_>) -> Result<String, AppError> {
    targets_output(|| read_report(path), verbose)
}

fn targets_output(
    read_report: impl FnOnce() -> Result<ReportFile, AppError>,
    verbose: Verbose<'_>,
) -> Result<String, AppError> {
    let report = read_report()?;
    Ok(serde_json::to_string(&semver_targets(&report, verbose))
        .expect("package names are JSON-compatible strings"))
}

pub fn semver_targets(report: &ReportFile, verbose: Verbose<'_>) -> BTreeSet<String> {
    let packages: BTreeMap<_, _> = report
        .packages
        .iter()
        .map(|package| (package.name.as_str(), package))
        .collect();
    let groups = report.version_groups();
    let mut selected = BTreeSet::new();
    for package in &report.packages {
        if package.status == PackageStatus::Unchanged || package.changed.is_empty() {
            verbose.note(|| {
                format!(
                    "{} has no changed released content requiring comparison, so it contributes no \
                 compatibility target",
                    quote_path(&package.name)
                )
            });
            continue;
        }
        let candidates = groups.closure(&package.name);
        for candidate in candidates {
            if packages
                .get(candidate.as_str())
                .is_some_and(|package| package.consumer_contract)
            {
                verbose.note(|| {
                    format!(
                        "{} has changed released content; {} is a consumer contract in its version \
                     group closure, so its public API is selected for comparison",
                        quote_path(&package.name),
                        quote_path(&candidate)
                    )
                });
                _ = selected.insert(candidate);
            } else {
                verbose.note(|| {
                    format!(
                        "{} has changed released content, but group-closure member {} declares no \
                     consumer contract, so it is not compared directly",
                        quote_path(&package.name),
                        quote_path(&candidate)
                    )
                });
            }
        }
    }
    selected
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::UnsupportedPlanSchemaError;
    use crate::report::fixture::{package, report};

    #[test]
    fn selects_changed_consumer_contracts_in_ordinal_order() {
        let mut data = report(vec![
            package("z", "needs-increment", true),
            package("A", "pending-release", true),
            package("a", "needs-increment", true),
            package("private", "needs-increment", false),
            package("unchanged", "unchanged", true),
        ]);
        let mut no_content = package("already-moved", "pending-release", true);
        *no_content.get_mut("changed").unwrap() = json!([]);
        data.packages
            .push(serde_json::from_value(no_content).unwrap());
        assert_eq!(
            semver_targets(&data, Verbose::new(true, &crp_diag::Discard)),
            ["A", "a", "z"].map(str::to_owned).into_iter().collect()
        );
    }

    #[test]
    fn maps_private_changes_to_every_public_group_member_without_duplicates() {
        let mut data = report(vec![
            package("api", "unchanged", true),
            package("api_other", "needs-increment", true),
            package("implementation", "needs-increment", false),
        ]);
        for package in &mut data.packages {
            package.group = Some("api".to_owned());
        }
        data.non_publishable_packages.push(
            serde_json::from_value(json!({
                "name": "helper", "declared_version": "1.0.0", "group": "api"
            }))
            .unwrap(),
        );
        data.groups = serde_json::from_value(json!({
            "api": {
                "members": ["api", "api_other", "helper", "implementation"],
                "consistent": true, "version": "1.0.0"
            }
        }))
        .unwrap();
        data.validate().unwrap();
        assert_eq!(
            semver_targets(&data, Verbose::new(false, &crp_diag::Discard)),
            ["api", "api_other"]
                .map(str::to_owned)
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn command_acquires_validated_report_and_serializes_package_arrays() {
        for (packages, expected) in [
            (vec![], json!([])),
            (
                vec![package("private", "needs-increment", false)],
                json!([]),
            ),
            (
                vec![package("api", "needs-increment", true)],
                json!(["api"]),
            ),
        ] {
            let data = report(packages);
            let output = targets_output(
                || {
                    data.validate()?;
                    Ok(data)
                },
                Verbose::new(false, &crp_diag::Discard),
            )
            .unwrap();
            assert_eq!(serde_json::from_str::<Value>(&output).unwrap(), expected);
        }
    }

    #[test]
    fn command_propagates_report_validation_failure() {
        let mut data = report(vec![]);
        data.schema_version = 0;
        let error = targets_output(
            || {
                data.validate()?;
                Ok(data)
            },
            Verbose::new(false, &crp_diag::Discard),
        )
        .unwrap_err();
        assert!(error.find_source::<UnsupportedPlanSchemaError>().is_some());
    }
}
