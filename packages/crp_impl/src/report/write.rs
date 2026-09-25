// `report` command: write report.json and per-package diffs.

use std::collections::BTreeMap;
use std::path::Path;

use ohno::AppError;
use serde::{Deserialize, Serialize};

use crate::classify::{
    AnchorJson, ChangedItem, Classification, DiffStat, PackageClass, PackageStatus, classify,
};
use crate::metadata::ReportedDep;
use crate::plan::SCHEMA_VERSION;
use crate::report::output::{FileOutput, ReportOutput};
use crate::text::quote_path;
use crate::verbose::Verbose;

/// On-disk `report.json` body.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ReportFile {
    pub(crate) schema_version: u32,
    pub(crate) head: String,
    pub(crate) packages: Vec<ReportPackage>,
    pub(crate) non_publishable_packages: Vec<ReportVersionTarget>,
    pub(crate) groups: BTreeMap<String, ReportGroup>,
}

/// One publishable package in `report.json`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ReportPackage {
    pub(crate) name: String,
    pub(crate) declared_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) group: Option<String>,
    pub(crate) status: PackageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) anchor: Option<AnchorJson>,
    pub(crate) changed: Vec<ChangedItem>,
    pub(crate) stat: DiffStat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) diff_path: Option<String>,
    pub(crate) dependencies: Vec<ReportedDep>,
    pub(crate) dependents: Vec<String>,
    /// Whether the package's library is what consumers are meant to use.
    ///
    /// False for a package with no library target, and for one declaring
    /// `[package.metadata.release-plan] private-api = true`.
    pub(crate) consumer_contract: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) untracked: Vec<String>,
}

/// One non-publishable version target in `report.json`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ReportVersionTarget {
    pub(crate) name: String,
    pub(crate) declared_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) group: Option<String>,
}

/// Version-group consistency as recorded in `report.json`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ReportGroup {
    pub(crate) members: Vec<String>,
    pub(crate) consistent: bool,
    pub(crate) version: String,
}

// Only the Git/Cargo and filesystem adapter wiring is excluded from library mutations.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn run_report(
    out_dir: &Path,
    base: Option<&str>,
    manifest_path: &Path,
    verbose: Verbose,
) -> Result<String, AppError> {
    create_report(
        out_dir,
        || classify(manifest_path, base, verbose),
        &mut FileOutput { directory: out_dir },
    )
}

// Preview already has a classification; this adapter supplies the real publication operations.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn write_report(
    out_dir: &Path,
    classification: &Classification,
) -> Result<String, AppError> {
    emit_report(
        out_dir,
        classification,
        &mut FileOutput { directory: out_dir },
    )
}

fn create_report(
    out_dir: &Path,
    classify: impl FnOnce() -> Result<Classification, AppError>,
    output: &mut impl ReportOutput,
) -> Result<String, AppError> {
    let classification = classify()?;
    emit_report(out_dir, &classification, output)
}

fn emit_report(
    out_dir: &Path,
    classification: &Classification,
    output: &mut impl ReportOutput,
) -> Result<String, AppError> {
    let diff_names: Vec<Option<String>> =
        classification.packages.iter().map(diff_file_name).collect();
    let packages = classification
        .packages
        .iter()
        .zip(&diff_names)
        .map(|(package, diff_name)| {
            report_package(
                package,
                diff_name.as_ref().map(|name| format!("diffs/{name}")),
            )
        })
        .collect();
    let mut groups = BTreeMap::new();
    for (name, verdict) in &classification.groups {
        groups.insert(
            name.clone(),
            ReportGroup {
                members: verdict.members().to_vec(),
                consistent: verdict.is_consistent(),
                version: verdict.version().to_string(),
            },
        );
    }
    let non_publishable_packages = classification
        .work_tree
        .version_targets
        .iter()
        .filter(|target| !target.publishable)
        .map(|target| ReportVersionTarget {
            name: target.name.clone(),
            declared_version: target.version.to_string(),
            group: classification
                .work_tree
                .groups
                .group_of(&target.name)
                .map(ToOwned::to_owned),
        })
        .collect();
    // The emitted field names are part of the consumer-facing layout documented
    // in the README, so they are compatibility-sensitive rather than incidental.
    let report = ReportFile {
        schema_version: SCHEMA_VERSION,
        head: classification.head.clone(),
        packages,
        non_publishable_packages,
        groups,
    };
    let report = serde_json::to_string_pretty(&report)
        .expect("the report body contains only JSON-serializable fields");

    output.reset()?;
    for (package, diff_name) in classification.packages.iter().zip(&diff_names) {
        if let Some(diff_name) = diff_name {
            output.write_patch(diff_name, package.patch())?;
        }
    }
    output.complete(&report)?;

    let report_path = out_dir.join("report.json");
    let needing_increment = classification
        .packages
        .iter()
        .filter(|package| package.status() == PackageStatus::NeedsIncrement)
        .count();
    Ok(format!(
        "Wrote {} ({} needing an increment)",
        quote_path(&report_path.display().to_string()),
        needing_increment
    ))
}

fn diff_file_name(package: &PackageClass) -> Option<String> {
    // Patches accompany a file difference; inherited-only and unchanged packages have none.
    if package.patch().is_empty() {
        return None;
    }
    Some(format!("{}.patch", package.name))
}

fn report_package(package: &PackageClass, diff_path: Option<String>) -> ReportPackage {
    ReportPackage {
        name: package.name.clone(),
        declared_version: package.declared_version.to_string(),
        group: package.group.clone(),
        status: package.status(),
        anchor: package.anchor().map(|anchor| AnchorJson {
            commit: anchor.commit.clone(),
            version: anchor.version.to_string(),
        }),
        changed: package.changed().to_vec(),
        stat: package.stat.clone(),
        diff_path,
        dependencies: package.dependencies.clone(),
        dependents: package.dependents.clone(),
        consumer_contract: package.consumer_contract,
        untracked: package.untracked.clone(),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use semver::Version;
    use serde_json::{Value, json};

    use super::*;
    use crate::classify::fixture::{classification, package};
    use crate::groups::Groups;
    use crate::metadata::{DepKind, ReportedDep, VersionTarget};

    #[test]
    fn command_publishes_current_patches_and_complete_report_metadata() {
        let mut api = package("api", PackageStatus::NeedsIncrement, "-old\n+new\n");
        api.group = Some("api".to_owned());
        api.consumer_contract = true;
        api.untracked = vec!["untracked.rs".to_owned()];
        api.dependencies = vec![ReportedDep {
            name: "pending".to_owned(),
            req: "1.0.1".to_owned(),
            exact_pin: false,
            kind: DepKind::Normal,
            public: true,
        }];
        let mut pending = package(
            "pending",
            PackageStatus::PendingRelease,
            "-before\n+after\n",
        );
        pending.dependents = vec!["api".to_owned()];
        let mut data = classification(vec![
            api,
            package("inherited", PackageStatus::NeedsIncrement, ""),
            pending,
        ]);
        data.work_tree.version_targets.push(VersionTarget {
            name: "helper".to_owned(),
            version: Version::new(1, 0, 0),
            manifest_path: PathBuf::from("helper/Cargo.toml"),
            publishable: false,
        });
        data.work_tree.groups = Groups::from_edges(
            data.work_tree
                .version_targets
                .iter()
                .map(|target| target.name.clone()),
            [("api".to_owned(), "helper".to_owned())],
        );
        data.groups = data
            .work_tree
            .groups
            .verdicts(&data.work_tree.target_versions(), &HashSet::new());
        let mut output = MemoryOutput {
            report: Some("old completion".to_owned()),
            patches: BTreeMap::from([("stale.patch".to_owned(), "stale".to_owned())]),
            ..MemoryOutput::default()
        };
        let directory = Path::new("report output");
        let message = create_report(directory, || Ok(data), &mut output).unwrap();
        assert_eq!(
            output.operations,
            [
                Operation::Reset,
                Operation::Patch,
                Operation::Patch,
                Operation::Complete
            ]
        );
        assert_eq!(
            output.patches,
            BTreeMap::from([
                ("api.patch".to_owned(), "-old\n+new\n".to_owned()),
                ("pending.patch".to_owned(), "-before\n+after\n".to_owned()),
            ])
        );
        let json: Value = serde_json::from_str(output.report.as_ref().unwrap()).unwrap();
        assert_eq!(json.get("schema_version"), Some(&json!(SCHEMA_VERSION)));
        assert_eq!(json.get("head"), Some(&json!("classified-head")));
        assert_eq!(json.get("packages").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(
            json.pointer("/packages/0").unwrap(),
            &json!({
                "name": "api", "declared_version": "1.0.0", "group": "api",
                "status": "needs-increment",
                "anchor": {"commit": "package-anchor", "version": "1.0.0"},
                "changed": [{"source": "package", "path": "src/lib.rs", "change": "modified"}],
                "stat": {"files": 1, "insertions": 1, "deletions": 1},
                "diff_path": "diffs/api.patch",
                "dependencies": [{"name": "pending", "req": "1.0.1", "exact_pin": false, "public": true}],
                "dependents": [], "consumer_contract": true, "untracked": ["untracked.rs"]
            })
        );
        assert_eq!(
            json.pointer("/packages/2/diff_path"),
            Some(&json!("diffs/pending.patch"))
        );
        assert_eq!(
            json.pointer("/packages/2/dependents"),
            Some(&json!(["api"]))
        );
        assert!(json.pointer("/packages/1/diff_path").is_none());
        assert_eq!(
            json.get("non_publishable_packages").unwrap(),
            &json!([
                {"name": "helper", "declared_version": "1.0.0", "group": "api"}
            ])
        );
        assert_eq!(
            json.get("groups").unwrap(),
            &json!({
                "api": {"members": ["api", "helper"], "consistent": true, "version": "1.0.0"}
            })
        );
        assert_eq!(
            message,
            format!(
                "Wrote {} (2 needing an increment)",
                quote_path(&directory.join("report.json").display().to_string()),
            )
        );
    }

    #[test]
    fn unchanged_and_new_packages_have_no_patch_name() {
        for data in [
            package("unchanged", PackageStatus::Unchanged, ""),
            PackageClass::new_package(
                "new",
                Version::new(1, 0, 0),
                PathBuf::from("new/Cargo.toml"),
            ),
        ] {
            assert!(diff_file_name(&data).is_none());
        }
    }

    #[test]
    fn classification_failure_does_not_start_publication() {
        let mut output = MemoryOutput::default();
        let error = create_report(
            Path::new("out"),
            || Err(PublicationFailure::new().into()),
            &mut output,
        )
        .unwrap_err();
        assert!(error.find_source::<PublicationFailure>().is_some());
        assert!(output.operations.is_empty());
    }

    #[test]
    fn publication_errors_propagate_without_completing_a_partial_report() {
        for (fail, expected) in [
            (Operation::Reset, vec![Operation::Reset]),
            (Operation::Patch, vec![Operation::Reset, Operation::Patch]),
            (
                Operation::Complete,
                vec![Operation::Reset, Operation::Patch, Operation::Complete],
            ),
        ] {
            let data = classification(vec![package("api", PackageStatus::NeedsIncrement, "patch")]);
            let mut output = MemoryOutput {
                fail: Some(fail),
                ..MemoryOutput::default()
            };
            let error = emit_report(Path::new("out"), &data, &mut output).unwrap_err();
            assert!(error.find_source::<PublicationFailure>().is_some());
            assert_eq!(output.operations, expected);
            assert!(output.report.is_none());
        }
    }

    #[test]
    fn empty_report_completes_with_no_patches_or_pending_increment() {
        let mut output = MemoryOutput::default();
        let message = emit_report(Path::new("out"), &classification(vec![]), &mut output).unwrap();
        assert_eq!(output.operations, [Operation::Reset, Operation::Complete]);
        assert!(output.patches.is_empty());
        let report: Value = serde_json::from_str(output.report.as_ref().unwrap()).unwrap();
        assert_eq!(report.get("packages"), Some(&json!([])));
        assert!(message.contains("(0 needing an increment)"));
    }

    /// Records publication effects without consulting a filesystem or rebuilding classification.
    #[derive(Default)]
    struct MemoryOutput {
        operations: Vec<Operation>,
        patches: BTreeMap<String, String>,
        report: Option<String>,
        fail: Option<Operation>,
    }

    impl MemoryOutput {
        fn record(&mut self, operation: Operation) -> Result<(), AppError> {
            self.operations.push(operation);
            if self.fail == Some(operation) {
                return Err(PublicationFailure::new().into());
            }
            Ok(())
        }
    }

    impl ReportOutput for MemoryOutput {
        fn reset(&mut self) -> Result<(), AppError> {
            self.record(Operation::Reset)?;
            self.patches.clear();
            self.report = None;
            Ok(())
        }

        fn write_patch(&mut self, name: &str, patch: &str) -> Result<(), AppError> {
            self.record(Operation::Patch)?;
            assert!(self.report.is_none());
            assert!(
                self.patches
                    .insert(name.to_owned(), patch.to_owned())
                    .is_none()
            );
            Ok(())
        }

        fn complete(&mut self, report: &str) -> Result<(), AppError> {
            self.record(Operation::Complete)?;
            assert!(self.report.replace(report.to_owned()).is_none());
            Ok(())
        }
    }

    /// Publication milestones whose ordering makes a completed artifact usable.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Reset,
        Patch,
        Complete,
    }

    /// Identifies a publication/acquisition failure without asserting its wording.
    #[ohno::error]
    struct PublicationFailure;
}
