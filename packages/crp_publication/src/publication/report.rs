//! Reconciles attempt-qualified receipts with platform job results and renders operator handoffs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::Path;

use ohno::AppError;
use serde::Deserialize;
use tempfile::NamedTempFile;

use crate::publication::context::WorkflowRun;
use crate::publication::github::Github;
use crate::publication::manifest::{InvalidManifest, PublicationManifest};
use crate::{ReadFileError, WriteFileError};

/// Platform facts cannot be replaced by an older successful receipt after a failed rerun.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobResults {
    prepare: JobResult,
    registry: JobResult,
    github: JobResult,
    binaries: JobResult,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum JobResult {
    Success,
    Failure,
    Cancelled,
    Skipped,
}

/// Common receipt projection; phase-specific payload remains in the owning artifacts.
#[derive(Debug, Deserialize)]
struct Receipt {
    schema_version: u32,
    publication_id: String,
    phase: String,
    complete: bool,
    github: WorkflowRun,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    batch_id: Option<String>,
    #[serde(default)]
    batches: Vec<ExpectedBatch>,
    #[serde(default)]
    errors: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedBatch {
    target: String,
    batch_id: String,
}

/// The report verdict and details are one result so issue text cannot disagree with exit status.
#[derive(Debug)]
struct Assessment {
    complete: bool,
    details: Vec<String>,
}

pub fn report(
    repository: &str,
    publication_path: Option<&Path>,
    outcomes: &Path,
    jobs_path: &Path,
    output: &Path,
    no_issue: bool,
) -> Result<(bool, String), AppError> {
    let context = WorkflowRun::capture()?.ok_or_else(|| {
        InvalidManifest::new(
            "publication reporting requires GitHub run and attempt context".to_owned(),
        )
    })?;
    let jobs: JobResults = serde_json::from_slice(
        &fs::read(jobs_path).map_err(|error| ReadFileError::caused_by(jobs_path, error))?,
    )?;
    let mut evidence_errors = Vec::new();
    let publication = match publication_path.map(PublicationManifest::read).transpose() {
        Ok(publication) => publication,
        Err(error) => {
            eprintln!("{error}");
            evidence_errors.push("The original publication manifest is unavailable or invalid; restore its artifact before retrying publication.".to_owned());
            None
        }
    };
    if publication
        .as_ref()
        .is_some_and(|manifest| manifest.publication.configuration.repository() != repository)
    {
        return Err(InvalidManifest::new(
            "report repository differs from publication intent".to_owned(),
        )
        .into());
    }
    let result = load_receipts(outcomes)
        .and_then(|receipts| assess(publication.as_ref(), &receipts, &jobs, context));
    let mut assessment = match result {
        Ok(assessment) => assessment,
        Err(error) => {
            eprintln!("{error}");
            let mut details = job_failures(&jobs);
            details.push(
                "Publication evidence is invalid or unavailable; inspect the reporter diagnostics."
                    .to_owned(),
            );
            Assessment {
                complete: false,
                details,
            }
        }
    };
    if !evidence_errors.is_empty() {
        assessment.complete = false;
        assessment.details.extend(evidence_errors);
    }
    let url = format!(
        "https://github.com/{repository}/actions/runs/{}",
        context.run_id
    );
    let mut body = format!(
        "[Copilot speaking]\n\n## Release {}\n\nWorkflow run: {url}\nAttempt: {}\n\n",
        if assessment.complete {
            "complete"
        } else {
            "incomplete"
        },
        context.run_attempt
    );
    for detail in &assessment.details {
        writeln!(body, "- {detail}")?;
    }
    if !assessment.complete {
        body.push_str("\nFor a missing-tag handoff, verify the recorded publication source and create only the missing tag at that exact commit using operator rights. Do not tag the current branch tip or move an existing tag. Then retry the original failed workflow. Keep the original manifest and attempt artifacts; if they expired, use explicit-source recovery.\n");
    }
    if output.try_exists()? {
        return Err(InvalidManifest::new("report output must be new".to_owned()).into());
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = NamedTempFile::new_in(parent)?;
    file.write_all(body.as_bytes())?;
    file.persist_noclobber(output)
        .map_err(|error| WriteFileError::caused_by(output, error))?;
    if !assessment.complete && !no_issue {
        Github::new(repository)?.report_failure(context, &body)?;
    }
    Ok((
        assessment.complete,
        format!("Publication report: {}.", output.display()),
    ))
}

fn load_receipts(root: &Path) -> Result<Vec<Receipt>, AppError> {
    if !root.try_exists()? {
        return Ok(Vec::new());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut receipts = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && entry.file_name() == "outcome.json" {
                let bytes = fs::read(entry.path())?;
                let receipt: Receipt = serde_json::from_slice(&bytes)?;
                if receipt.schema_version != 1
                    || !matches!(receipt.phase.as_str(), "registry" | "github" | "binaries")
                {
                    return Err(InvalidManifest::new(
                        "unsupported publication receipt schema or phase".to_owned(),
                    )
                    .into());
                }
                receipts.push(receipt);
            }
        }
    }
    Ok(receipts)
}

fn job_failures(jobs: &JobResults) -> Vec<String> {
    let mut details = Vec::new();
    for (phase, result) in [
        ("prepare", jobs.prepare),
        ("registry", jobs.registry),
        ("github", jobs.github),
        ("binaries", jobs.binaries),
    ] {
        if matches!(result, JobResult::Failure | JobResult::Cancelled)
            || phase != "binaries" && result == JobResult::Skipped
        {
            details.push(format!(
                "{phase} job did not complete successfully ({result:?})."
            ));
        }
    }
    details
}

fn assess(
    publication: Option<&PublicationManifest>,
    receipts: &[Receipt],
    jobs: &JobResults,
    context: WorkflowRun,
) -> Result<Assessment, AppError> {
    let mut details = job_failures(jobs);
    let Some(publication) = publication else {
        details.push("The original immutable publication manifest is unavailable.".to_owned());
        return Ok(Assessment {
            complete: false,
            details,
        });
    };
    let select = |phase: &str,
                  target: Option<&str>,
                  batch: Option<&str>|
     -> Result<Option<&Receipt>, AppError> {
        let candidates = receipts.iter().filter(|receipt| {
            receipt.publication_id == publication.id
                && receipt.github.run_id == context.run_id
                && receipt.github.run_attempt <= context.run_attempt
                && receipt.phase == phase
                && receipt.target.as_deref() == target
                && receipt.batch_id.as_deref() == batch
        });
        let mut selected: Option<&Receipt> = None;
        let mut attempts = BTreeSet::new();
        for receipt in candidates {
            if !attempts.insert(receipt.github.run_attempt) {
                return Err(InvalidManifest::new(
                    "duplicate outcomes for the same publication execution unit".to_owned(),
                )
                .into());
            }
            if selected
                .is_none_or(|previous| previous.github.run_attempt < receipt.github.run_attempt)
            {
                selected = Some(receipt);
            }
        }
        Ok(selected)
    };
    for phase in ["registry", "github"] {
        match select(phase, None, None)? {
            Some(receipt) if receipt.complete => {}
            Some(receipt) => {
                details.push(format!("{phase} receipt reports incomplete delivery."));
                details.extend(receipt.errors.iter().cloned());
            }
            None => details.push(format!(
                "No {phase} outcome matches this publication and workflow run."
            )),
        }
    }
    if let Some(github) = select("github", None, None)? {
        let mut expected = BTreeMap::new();
        for batch in &github.batches {
            if expected.insert(&batch.target, &batch.batch_id).is_some() {
                return Err(InvalidManifest::new(
                    "GitHub outcome repeats a native target".to_owned(),
                )
                .into());
            }
            match select("binaries",Some(&batch.target),Some(&batch.batch_id))? {
                Some(receipt) if receipt.complete && receipt.github.run_attempt>=github.github.run_attempt=>{},
                _=>details.push(format!("No completed binary receipt satisfies {} batch {} from reconciliation attempt {}.",
                    batch.target,batch.batch_id,github.github.run_attempt)),
            }
        }
        if !github.batches.is_empty() && jobs.binaries != JobResult::Success {
            details.push("Expected binary jobs did not all succeed; older receipts cannot override that result.".to_owned());
        }
    }
    Ok(Assessment {
        complete: details.is_empty(),
        details,
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::*;

    fn publication() -> PublicationManifest {
        PublicationManifest::new(serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0","source":"a".repeat(40),
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/tool","release-branch":"main","targets":[]},
            "packages":[]
        })).unwrap()).unwrap()
    }

    fn context(attempt: u32) -> WorkflowRun {
        serde_json::from_value(json!({"run_id":123,"run_attempt":attempt})).unwrap()
    }

    fn jobs(binaries: JobResult) -> JobResults {
        JobResults {
            prepare: JobResult::Success,
            registry: JobResult::Success,
            github: JobResult::Success,
            binaries,
        }
    }

    fn receipt(
        publication: &PublicationManifest,
        phase: &str,
        attempt: u32,
        complete: bool,
    ) -> Receipt {
        Receipt {
            schema_version: 1,
            publication_id: publication.id.clone(),
            phase: phase.to_owned(),
            complete,
            github: context(attempt),
            target: None,
            batch_id: None,
            batches: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn binary(
        publication: &PublicationManifest,
        target: &str,
        batch: &str,
        attempt: u32,
        complete: bool,
    ) -> Receipt {
        let mut receipt = receipt(publication, "binaries", attempt, complete);
        receipt.target = Some(target.to_owned());
        receipt.batch_id = Some(batch.to_owned());
        receipt
    }

    #[test]
    fn failed_rerun_cannot_be_hidden_by_historical_receipts() {
        let publication = publication();
        let mut github = receipt(&publication, "github", 1, true);
        github.batches = vec![
            ExpectedBatch {
                target: "a".to_owned(),
                batch_id: "a-id".to_owned(),
            },
            ExpectedBatch {
                target: "b".to_owned(),
                batch_id: "b-id".to_owned(),
            },
        ];
        let mut receipts = vec![
            receipt(&publication, "registry", 1, true),
            github,
            binary(&publication, "a", "a-id", 1, true),
            binary(&publication, "b", "b-id", 1, false),
        ];
        assert!(
            !assess(
                Some(&publication),
                &receipts,
                &jobs(JobResult::Failure),
                context(2)
            )
            .unwrap()
            .complete
        );
        receipts.push(binary(&publication, "b", "b-id", 3, true));
        assert!(
            assess(
                Some(&publication),
                &receipts,
                &jobs(JobResult::Success),
                context(3)
            )
            .unwrap()
            .complete
        );
        assert!(
            !assess(
                Some(&publication),
                &receipts,
                &jobs(JobResult::Cancelled),
                context(3)
            )
            .unwrap()
            .complete
        );
    }

    #[test]
    fn changed_or_reobserved_batches_require_new_binary_evidence() {
        let publication = publication();
        for id in ["same", "different"] {
            let mut github = receipt(&publication, "github", 2, true);
            github.batches = vec![ExpectedBatch {
                target: "native".to_owned(),
                batch_id: id.to_owned(),
            }];
            let receipts = vec![
                receipt(&publication, "registry", 1, true),
                github,
                binary(&publication, "native", "same", 1, true),
            ];
            assert!(
                !assess(
                    Some(&publication),
                    &receipts,
                    &jobs(JobResult::Success),
                    context(2)
                )
                .unwrap()
                .complete
            );
        }
    }

    #[test]
    fn requires_manifest_and_phase_receipts_even_when_platform_jobs_succeed() {
        let publication = publication();
        let jobs = jobs(JobResult::Skipped);
        assert!(!assess(None, &[], &jobs, context(1)).unwrap().complete);
        assert!(
            !assess(Some(&publication), &[], &jobs, context(1))
                .unwrap()
                .complete
        );
        let receipts = vec![
            receipt(&publication, "registry", 1, true),
            receipt(&publication, "github", 1, true),
        ];
        assert!(
            assess(Some(&publication), &receipts, &jobs, context(1))
                .unwrap()
                .complete
        );
    }

    #[test]
    fn unavailable_or_ambiguous_receipts_never_establish_completion() {
        let publication = publication();
        let mut registry = receipt(&publication, "registry", 1, true);
        registry.github.run_id = 456.try_into().unwrap();
        let mut receipts = vec![registry, receipt(&publication, "github", 1, true)];
        assert!(
            !assess(
                Some(&publication),
                &receipts,
                &jobs(JobResult::Skipped),
                context(1)
            )
            .unwrap()
            .complete
        );
        receipts.push(receipt(&publication, "registry", 2, true));
        assert!(
            !assess(
                Some(&publication),
                &receipts,
                &jobs(JobResult::Skipped),
                context(1)
            )
            .unwrap()
            .complete
        );
        receipts.push(receipt(&publication, "registry", 2, true));
        assess(
            Some(&publication),
            &receipts,
            &jobs(JobResult::Skipped),
            context(2),
        )
        .unwrap_err();
    }

    #[test]
    fn incomplete_tag_reconciliation_retains_exact_manual_recovery() {
        let publication = publication();
        let mut github = receipt(&publication, "github", 1, false);
        github
            .errors
            .push("Create tool-v1.0.0 at immutable-source then retry original run".to_owned());
        let mut platform = jobs(JobResult::Skipped);
        platform.github = JobResult::Failure;
        let report = assess(
            Some(&publication),
            &[receipt(&publication, "registry", 1, true), github],
            &platform,
            context(1),
        )
        .unwrap();
        assert!(!report.complete);
        assert!(
            report
                .details
                .iter()
                .any(|detail| detail.contains("tool-v1.0.0"))
        );
    }
}
