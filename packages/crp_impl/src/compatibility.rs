//! Explicit external API evidence, kept separate from offline classification and semantic decisions.

use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{env, fs};

use ohno::AppError;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::inspect_plan::run_inspect_plan;
use crate::plan::{PlanFile, SCHEMA_VERSION};
use crate::preview::Prepared;
use crate::publication::registry::{RegistryClient, write_outcome};
use crate::report::{read_report, run_report};
use crate::resolved::{Inputs, read_json};
use crate::semver_targets::semver_targets;
use crate::verbose::Verbose;

/// Identifies the assessed source without accepting a report detached from its input snapshot.
enum Evidence {
    Source(Inputs),
    Preview(PlanFile),
}

impl Evidence {
    fn root(&self) -> &Path {
        match self {
            Self::Source(inputs) => &inputs.root,
            Self::Preview(plan) => {
                &plan
                    .resolved
                    .as_ref()
                    .expect("preview evidence requires captured resolution")
                    .inputs
                    .root
            }
        }
    }
    fn base(&self) -> &str {
        match self {
            Self::Source(inputs) => &inputs.base,
            Self::Preview(plan) => {
                &plan
                    .resolved
                    .as_ref()
                    .expect("preview evidence requires captured resolution")
                    .inputs
                    .base
            }
        }
    }
    fn verify(&self, manifest: &Path) -> Result<(), AppError> {
        match self {
            Self::Source(inputs) => {
                inputs.verify(manifest, None)?;
            }
            Self::Preview(plan) => plan
                .resolved
                .as_ref()
                .ok_or_else(|| {
                    CompatibilityError::new("preview has no captured resolution".to_owned())
                })?
                .verify_candidate(manifest)?,
        }
        Ok(())
    }
}

/// Completed comparisons and their semantic floors, not an automatic semantic assessment.
#[derive(Debug, Serialize)]
struct CompatibilityOutcome {
    schema_version: u32,
    checker: String,
    report: PathBuf,
    completed: bool,
    findings: bool,
    packages: Vec<Comparison>,
}

#[derive(Debug, Serialize)]
struct Comparison {
    name: String,
    baseline_version: Option<String>,
    required_level: Option<&'static str>,
    compared: bool,
}

pub(crate) fn check(
    manifest: &Path,
    prepared: Option<&Path>,
    plan: Option<&Path>,
    base: Option<&str>,
    output: &Path,
    deny_findings: bool,
    verbose: Verbose,
) -> Result<(bool, String), AppError> {
    if output.try_exists()? {
        return Err(CompatibilityError::new(
            "compatibility output directory must be new".to_owned(),
        )
        .into());
    }
    fs::create_dir_all(output)?;
    let (evidence, manifest) = if let Some(path) = prepared {
        let prepared: Prepared = read_json(path)?;
        if prepared.schema_version != SCHEMA_VERSION {
            return Err(
                CompatibilityError::new("unsupported preparation schema".to_owned()).into(),
            );
        }
        let manifest = manifest.canonicalize()?;
        (Evidence::Source(prepared.inputs), manifest)
    } else if let Some(path) = plan {
        run_inspect_plan(path, true, manifest, verbose)?;
        let plan: PlanFile = read_json(path)?;
        let manifest = plan
            .resolved
            .as_ref()
            .ok_or_else(|| {
                CompatibilityError::new("compatibility requires a resolved preview".to_owned())
            })?
            .evidence_manifest_path
            .clone();
        (Evidence::Preview(plan), manifest)
    } else {
        let manifest = manifest.canonicalize()?;
        let inputs = Inputs::capture(&manifest, base)?;
        (Evidence::Source(inputs), manifest)
    };
    evidence.verify(&manifest)?;
    // Derive target selection from the bound source rather than trusting an adjacent report
    // that could have been replaced independently of the prepared/preview artifact.
    run_report(output, Some(evidence.base()), &manifest, verbose)?;
    evidence.verify(&manifest)?;
    let report = output.join("report.json");
    let targets = semver_targets(&read_report(&report)?, verbose);
    let cache = cache_path(&env::temp_dir(), evidence.root())?;
    fs::create_dir_all(&cache)?;
    let log = output.join("semver-checks.log");
    let mut log = fs::File::create(log)?;
    let mut outcome = CompatibilityOutcome {
        schema_version: 1,
        checker: "not invoked: no consumer contracts selected".to_owned(),
        report,
        completed: false,
        findings: false,
        packages: Vec::new(),
    };
    let result = (|| {
        if !targets.is_empty() {
            let checker = invoke(&manifest, &cache, &["--version"])?;
            if !checker.status.success() {
                return Err(CompatibilityError::new(
                    "cargo-semver-checks could not identify itself".to_owned(),
                )
                .into());
            }
            String::from_utf8_lossy(&checker.stdout)
                .trim()
                .clone_into(&mut outcome.checker);
            canary(&cache, &mut log)?;
        }
        let registry = RegistryClient::new()?;
        for name in targets {
            let baseline = registry.latest(&name)?;
            let Some(baseline) = baseline else {
                if registry.exists(&name)? {
                    return Err(CompatibilityError::new(format!(
                        "{name} has no usable published comparison version"
                    ))
                    .into());
                }
                verbose.note(||format!("{name} has no available published comparison version; no compatibility conclusion is inferred."));
                outcome.packages.push(Comparison {
                    name,
                    baseline_version: None,
                    required_level: None,
                    compared: false,
                });
                continue;
            };
            verbose.note(||format!("Comparing {name} against published {baseline} with all features from the captured source workspace."));
            let result = invoke(
                &manifest,
                &cache,
                &[
                    "--all-features",
                    "--manifest-path",
                    &manifest.to_string_lossy(),
                    "--baseline-version",
                    &baseline.to_string(),
                    "-p",
                    &name,
                ],
            )?;
            let text = record(&result, &mut log)?;
            let floor = interpret(result.status.code(), &text)?;
            outcome.findings |= result.status.code() == Some(100);
            outcome.packages.push(Comparison {
                name,
                baseline_version: Some(baseline.to_string()),
                required_level: floor,
                compared: true,
            });
        }
        outcome.completed = true;
        Ok::<_, AppError>(())
    })();
    let unchanged = evidence.verify(&manifest);
    if let Err(error) = &unchanged {
        eprintln!("{error}");
        outcome.completed = false;
    }
    write_outcome(&output.join("compatibility.json"), &outcome)?;
    unchanged?;
    result?;
    Ok((
        !deny_findings || !outcome.findings,
        format!(
            "{}Compatibility evidence: {}.",
            if deny_findings && outcome.findings {
                format!(
                    "Insufficient version increments: {}. ",
                    outcome
                        .packages
                        .iter()
                        .filter_map(|package| package
                            .required_level
                            .map(|level| format!("{} requires {level}", package.name)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                String::new()
            },
            output.join("compatibility.json").display(),
        ),
    ))
}

fn cache_path(temporary: &Path, root: &Path) -> Result<PathBuf, AppError> {
    // A stable short per-workspace root shares compiler artifacts across prepare/preview/verify
    // without exposing cargo-semver-checks' long generated paths to the Windows path limit.
    const ID_BYTES: usize = 8;
    let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
    let mut id = String::new();
    for byte in digest.iter().take(ID_BYTES) {
        write!(id, "{byte:02x}")?;
    }
    Ok(temporary.join(format!("crp-semver-{id}")))
}

fn invoke(manifest: &Path, cache: &Path, args: &[&str]) -> Result<Output, AppError> {
    let mut command = Command::new("cargo");
    command
        .arg("semver-checks")
        .args(args)
        .current_dir(manifest.parent().unwrap_or_else(|| Path::new(".")))
        .env("CARGO_TARGET_DIR", cache)
        .env("CARGO_TERM_COLOR", "never");
    for name in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "CARGO_REGISTRY_TOKEN",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    ] {
        command.env_remove(name);
    }
    Ok(command.output()?)
}

fn canary(cache: &Path, log: &mut fs::File) -> Result<(), AppError> {
    let fixture = tempfile::Builder::new()
        .prefix("crp-semver-canary-")
        .tempdir()?;
    fs::write(
        fixture.path().join("Cargo.toml"),
        "[package]\nname='release-plan-canary'\nversion='1.0.0'\nedition='2024'\n[lib]\npath='lib.rs'\n[workspace]\n",
    )?;
    fs::write(fixture.path().join("lib.rs"), "pub fn canary() {}\n")?;
    let manifest = fixture.path().join("Cargo.toml");
    let result = invoke(
        &manifest,
        cache,
        &[
            "--manifest-path",
            &manifest.to_string_lossy(),
            "--baseline-root",
            &fixture.path().to_string_lossy(),
            "--all-features",
        ],
    )?;
    record(&result, log)?;
    if !result.status.success() {
        return Err(CompatibilityError::new(
            "compatibility checker failed its identical-source canary".to_owned(),
        )
        .into());
    }
    Ok(())
}

fn record(result: &Output, log: &mut fs::File) -> Result<String, AppError> {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    log.write_all(text.as_bytes())?;
    eprint!("{text}");
    Ok(text)
}

fn interpret(code: Option<i32>, text: &str) -> Result<Option<&'static str>, AppError> {
    if !matches!(code, Some(0 | 100)) {
        return Err(CompatibilityError::new(format!(
            "cargo-semver-checks execution failed with status {code:?}"
        ))
        .into());
    }
    if text
        .lines()
        .any(|line| line.contains("Summary semver requires new major version"))
    {
        return Ok(Some("breaking"));
    }
    if text
        .lines()
        .any(|line| line.contains("Summary semver requires new minor version"))
    {
        return Ok(Some("nonbreaking"));
    }
    if code == Some(0)
        && text
            .lines()
            .any(|line| line.contains("Summary no semver update required"))
    {
        return Ok(None);
    }
    Err(CompatibilityError::new(
        "compatibility checker supplied no recognized complete summary".to_owned(),
    )
    .into())
}

#[ohno::error]
#[display("{reason}")]
struct CompatibilityError {
    reason: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn compiler_cache_is_stable_per_original_workspace() {
        let root = Path::new("workspace");
        let temporary = Path::new("temporary");
        assert_eq!(
            cache_path(temporary, root).unwrap(),
            cache_path(temporary, root).unwrap()
        );
        assert_ne!(
            cache_path(temporary, root).unwrap(),
            cache_path(temporary, Path::new("another")).unwrap()
        );
    }

    #[test]
    fn findings_and_execution_failure_are_distinct() {
        assert_eq!(
            interpret(Some(0), " Summary no semver update required").unwrap(),
            None
        );
        assert_eq!(
            interpret(Some(100), " Summary semver requires new minor version: 1").unwrap(),
            Some("nonbreaking")
        );
        assert_eq!(
            interpret(Some(100), " Summary semver requires new major version: 1").unwrap(),
            Some("breaking")
        );
        for code in [None, Some(1), Some(2)] {
            interpret(code, " Summary no semver update required").unwrap_err();
        }
        interpret(Some(0), "checker did not compare").unwrap_err();
        interpret(Some(100), " Summary no semver update required").unwrap_err();
    }
}
