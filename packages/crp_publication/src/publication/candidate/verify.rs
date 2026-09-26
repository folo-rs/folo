use std::ffi::OsStr;
use std::path::Path;

use crp_diag::Verbose;
use crp_versioning::{CheckFormat, CheckOutcome, CheckRequest};
use ohno::AppError;

use crate::publication::candidate::repository::{VerificationError, validate_around};
use crate::publication::candidate::verification_repository::VerificationRepository;
use crate::publication::candidate::{CandidateRequest, Metadata, Repository};

// Only real-system wiring is excluded. The same sequence below runs with in-memory evidence;
// integration tests cover discovery, subprocesses and the executable connection.
#[cfg_attr(test, mutants::skip)]
pub fn verify(cli: &CandidateRequest, notes: Verbose<'_>) -> Result<String, AppError> {
    verify_using(cli, Repository::discover, notes, |message| {
        Verbose::new(true, notes.sink()).note(|| message.to_owned());
    })
}

pub(crate) fn verify_using<R: VerificationRepository>(
    cli: &CandidateRequest,
    discover: impl FnOnce(&Path, &str) -> Result<R, AppError>,
    notes: Verbose<'_>,
    mut diagnostic: impl FnMut(&str),
) -> Result<String, AppError> {
    let repository = discover(&cli.manifest_path, &cli.commit)?;
    repository.ensure_clean_head()?;
    repository.ensure_first_parent(&cli.release_line)?;
    let manifest = repository.require_tracked(&cli.manifest_path)?;
    if cli.verbose {
        diagnostic(&format!(
            "Candidate {} equals clean HEAD and belongs to the first-parent \
             history of supplied main tip {}; side-branch ancestry alone is not sufficient",
            cli.commit, cli.release_line
        ));
    }
    let metadata = validate_around(
        || repository.ensure_clean_head(),
        || {
            repository.capture(
                "cargo",
                &[
                    OsStr::new("metadata"),
                    OsStr::new("--format-version"),
                    OsStr::new("1"),
                    OsStr::new("--locked"),
                    OsStr::new("--offline"),
                    OsStr::new("--no-deps"),
                    OsStr::new("--manifest-path"),
                    manifest.as_os_str(),
                ],
            )
        },
    )?;
    let metadata = Metadata::parse(&metadata)?;
    repository.validate_inputs(&metadata, &manifest)?;
    metadata.validate_packages(&cli.packages, cli.verbose, &mut diagnostic)?;
    if cli.verbose {
        diagnostic(&format!(
            "Release invariants use candidate {} as their baseline, not \
             later main tip {}; each version retains its original first-parent anchor, including \
             inherited values and locked binary closures",
            cli.commit, cli.release_line
        ));
    }
    let outcome = validate_around(
        || repository.ensure_clean_head(),
        || {
            repository.check(
                &CheckRequest {
                    base: Some(&cli.commit),
                    manifest_path: &manifest,
                    format: CheckFormat::Text,
                    verify_packaging: false,
                },
                notes,
            )
        },
    )?;
    finish_check(outcome, cli.verbose, diagnostic)?;
    let packages = cli
        .packages
        .iter()
        .map(|(name, version)| format!("{name}@{version}"))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "Verified release target {}: {packages}.",
        cli.commit
    ))
}

/// Interprets the checker's operation-specific verdict and forwards its diagnostics.
fn finish_check(
    outcome: CheckOutcome,
    verbose: bool,
    mut diagnostic: impl FnMut(&str),
) -> Result<(), AppError> {
    if !outcome.warnings.is_empty() {
        diagnostic(&outcome.warnings);
    }
    if !outcome.passed {
        return Err(VerificationError::new(outcome.message).into());
    }
    if verbose {
        diagnostic(&format!(
            "{} HEAD and cleanliness still match the \
             candidate after metadata and release verification",
            outcome.message,
        ));
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn preserves_verdicts_and_diagnostic_order() {
        for passed in [false, true] {
            for warnings in ["", "warning canary\n"] {
                for verbose in [false, true] {
                    let mut diagnostics = Vec::new();
                    let result = finish_check(
                        CheckOutcome {
                            passed,
                            message: "verdict canary".into(),
                            warnings: warnings.into(),
                        },
                        verbose,
                        |message| diagnostics.push(message.to_owned()),
                    );
                    assert_eq!(result.is_ok(), passed);
                    if let Err(error) = result {
                        assert!(error.find_source::<VerificationError>().is_some());
                    }

                    assert_eq!(
                        diagnostics.len(),
                        usize::from(!warnings.is_empty()) + usize::from(passed && verbose)
                    );
                    if !warnings.is_empty() {
                        assert_eq!(diagnostics.first().unwrap(), warnings);
                    }
                    if passed && verbose {
                        assert!(diagnostics.last().unwrap().contains("verdict canary"));
                    }
                }
            }
        }
    }
}
