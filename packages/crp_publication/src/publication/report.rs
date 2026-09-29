//! Reconciles attempt-qualified receipts with workflow job results and renders operator handoffs.

#![allow(
    clippy::self_named_module_files,
    reason = "The report subject owns its evidence-validation child module."
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crp_workspace::artifact_path::write_new;
use ohno::AppError;
use serde::Deserialize;

use crate::publication::context::WorkflowRun;
use crate::publication::github::Github;
use crate::publication::manifest::{InvalidManifest, PublicationManifest};
use crate::publication::report::evidence::parse_receipt;
use crate::{ReadFileError, WriteFileError};

/// Workflow job results cannot be replaced by an older successful receipt after a failed rerun.
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

/// Validated phase evidence projected for attempt selection without losing human results.
#[derive(Debug)]
struct Receipt {
    path: PathBuf,
    publication_id: String,
    phase: String,
    complete: bool,
    github: WorkflowRun,
    target: Option<String>,
    batch_id: Option<String>,
    batches: Vec<ExpectedBatch>,
    errors: Vec<String>,
    summaries: Vec<String>,
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

/// Keeps per-file acquisition failures alongside independently usable outcomes.
#[derive(Default)]
struct LoadedEvidence {
    receipts: Vec<Receipt>,
    failures: Vec<(PathBuf, AppError)>,
}

// Filesystem collection and issue delivery are covered by boundary tests; selection/rendering
// use the in-process helpers below.
#[cfg_attr(test, mutants::skip)]
pub fn report(
    repository: &str,
    publication_path: Option<&Path>,
    outcomes: &Path,
    jobs_path: &Path,
    output: &Path,
    no_issue: bool,
    diagnostics: &crate::PublicationOutput,
) -> Result<(bool, String), AppError> {
    let context = WorkflowRun::capture()?.ok_or_else(|| {
        InvalidManifest::new(
            "publication reporting requires GitHub run and attempt context".to_owned(),
        )
    })?;
    let jobs: JobResults = serde_json::from_slice(
        &fs::read(jobs_path).map_err(|error| ReadFileError::caused_by(jobs_path, error))?,
    )
    .map_err(|error| ReceiptReadError::caused_by(jobs_path, error))?;
    let mut evidence_errors = Vec::new();
    let publication = match publication_path.map(PublicationManifest::read).transpose() {
        Ok(publication) => publication,
        Err(error) => {
            diagnostics.line(format_args!("{error}"));
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
    let loaded = load_receipts(outcomes, publication.as_ref());
    let mut assessment = assess_loaded(publication.as_ref(), &loaded, &jobs, context);
    for (_, error) in &loaded.failures {
        diagnostics.line(format_args!("{error}"));
    }
    if !evidence_errors.is_empty() {
        assessment.complete = false;
        evidence_errors.extend(assessment.details);
        assessment.details = evidence_errors;
    }
    let body = render_report(repository, context, &assessment)?;
    write_new(output, |file| {
        file.write_all(body.as_bytes())
            .map_err(|error| WriteFileError::caused_by(output, error).into())
    })?;
    if !assessment.complete && !no_issue {
        Github::new(repository, diagnostics)?.report_failure(context, &body)?;
    }
    Ok((
        assessment.complete,
        format!("Publication report: {}.", output.display()),
    ))
}

fn assess_loaded(
    publication: Option<&PublicationManifest>,
    loaded: &LoadedEvidence,
    jobs: &JobResults,
    context: WorkflowRun,
) -> Assessment {
    let mut assessment = assess(publication, &loaded.receipts, jobs, context);
    if !loaded.failures.is_empty() {
        assessment.complete = false;
        let mut failures: Vec<_> = loaded.failures.iter().map(|(path, _)| {
            format!("Publication outcome {} is invalid or unavailable; inspect the reporter diagnostics.", path.display())
        }).collect();
        failures.extend(assessment.details);
        assessment.details = failures;
    }
    assessment
}

fn render_report(
    repository: &str,
    context: WorkflowRun,
    assessment: &Assessment,
) -> Result<String, AppError> {
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
        body.push_str("\nKeep the original manifest and attempt artifacts. Retry the original failed workflow after correcting the observed blocker; if its artifacts expired, use explicit-source recovery.\n");
    }
    Ok(body)
}

#[cfg_attr(test, mutants::skip)] // Real directory traversal and file reads belong to boundary tests.
fn load_receipts(root: &Path, publication: Option<&PublicationManifest>) -> LoadedEvidence {
    let mut loaded = LoadedEvidence::default();
    match root.try_exists() {
        Ok(false) => return loaded,
        Ok(true) => {}
        Err(error) => {
            loaded.failures.push((
                root.to_path_buf(),
                ReadFileError::caused_by(root, error).into(),
            ));
            return loaded;
        }
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                loaded.failures.push((
                    directory.clone(),
                    ReadFileError::caused_by(&directory, error).into(),
                ));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    loaded.failures.push((
                        directory.clone(),
                        ReadFileError::caused_by(&directory, error).into(),
                    ));
                    continue;
                }
            };
            let path = entry.path();
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    loaded
                        .failures
                        .push((path.clone(), ReadFileError::caused_by(&path, error).into()));
                    continue;
                }
            };
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() && entry.file_name() == "outcome.json" {
                // The shared workflow stages phase outcomes under this filename.
                // Ref: book/src/integration/publication.md, artifact layout.
                let result = fs::read(&path)
                    .map_err(|error| ReadFileError::caused_by(&path, error).into())
                    .and_then(|bytes| parse_receipt(&bytes, publication, &path));
                collect_receipt(&mut loaded, path, result);
            }
        }
    }
    loaded
}

fn collect_receipt(
    loaded: &mut LoadedEvidence,
    path: PathBuf,
    result: Result<Option<Receipt>, AppError>,
) {
    match result {
        Ok(Some(receipt)) => loaded.receipts.push(receipt),
        Ok(None) => {}
        Err(error) => loaded.failures.push((
            path.clone(),
            ReceiptReadError::caused_by(path, error).into(),
        )),
    }
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
            // A skipped empty binary matrix is legitimate; required phases may not be skipped.
            || phase != "binaries" && result == JobResult::Skipped
        {
            details.push(format!(
                "{phase} job did not complete successfully ({result:?})."
            ));
        }
    }
    details
}

/// Selects the latest eligible outcome for each publication execution unit.
///
/// Current job failures remain authoritative; binary evidence must follow its reconciliation.
/// Informational partial-success summaries never participate in the failure verdict.
fn assess(
    publication: Option<&PublicationManifest>,
    receipts: &[Receipt],
    jobs: &JobResults,
    context: WorkflowRun,
) -> Assessment {
    let mut details = job_failures(jobs);
    let mut summaries = Vec::new();
    let Some(publication) = publication else {
        details.push("The original immutable publication manifest is unavailable.".to_owned());
        return Assessment {
            complete: false,
            details,
        };
    };
    let mut github_outcome = None;
    for phase in ["registry", "github"] {
        match select_receipt(
            receipts,
            &publication.id,
            context,
            phase,
            None,
            None,
            &mut details,
        ) {
            Some(receipt) => {
                if phase == "github" {
                    github_outcome = Some(receipt);
                }
                summaries.extend(receipt.summaries.iter().cloned());
                if !receipt.complete {
                    details.push(format!("{phase} outcome reports incomplete delivery."));
                    details.extend(receipt.errors.iter().cloned());
                }
            }
            None => details.push(format!(
                "No {phase} outcome matches this publication and workflow run."
            )),
        }
    }
    if let Some(github) = github_outcome {
        let mut expected = BTreeMap::new();
        for batch in &github.batches {
            if expected.insert(&batch.target, &batch.batch_id).is_some() {
                details.push(format!(
                    "GitHub outcome {} repeats target {}.",
                    github.path.display(),
                    batch.target
                ));
                continue;
            }
            match select_receipt(receipts, &publication.id, context, "binaries", Some(&batch.target), Some(&batch.batch_id), &mut details) {
                // An older success cannot override a newer observation of missing assets.
                Some(receipt) if receipt.github.run_attempt >= github.github.run_attempt => {
                    summaries.extend(receipt.summaries.iter().cloned());
                    if !receipt.complete {
                        details.push(format!("Binary delivery is incomplete for {}.", batch.target));
                        details.extend(receipt.errors.iter().cloned());
                    }
                }
                _ => details.push(format!("No completed binary outcome satisfies {} batch {} from reconciliation attempt {}.",
                    batch.target, batch.batch_id, github.github.run_attempt)),
            }
        }
        if !github.batches.is_empty() && jobs.binaries == JobResult::Skipped {
            details.push(
                "Required binary jobs were skipped; older outcomes cannot override that result."
                    .to_owned(),
            );
        }
    }
    let complete = details.is_empty();
    details.extend(summaries);
    Assessment { complete, details }
}

fn select_receipt<'a>(
    receipts: &'a [Receipt],
    publication_id: &str,
    context: WorkflowRun,
    phase: &str,
    target: Option<&str>,
    batch: Option<&str>,
    failures: &mut Vec<String>,
) -> Option<&'a Receipt> {
    let mut selected: Option<&Receipt> = None;
    let mut attempts = BTreeMap::new();
    let mut ambiguous = false;
    for receipt in receipts.iter().filter(|receipt| {
        receipt.publication_id == publication_id
            && receipt.github.run_id == context.run_id
            && receipt.github.run_attempt <= context.run_attempt
            && receipt.phase == phase
            && receipt.target.as_deref() == target
            && receipt.batch_id.as_deref() == batch
    }) {
        if let Some(previous) = attempts.insert(receipt.github.run_attempt, &receipt.path) {
            ambiguous = true;
            failures.push(format!(
                "Duplicate {phase} outcomes for publication {publication_id}, run {}, attempt {}, target {target:?}, batch {batch:?}: {} and {}.",
                context.run_id, receipt.github.run_attempt, previous.display(), receipt.path.display(),
            ));
        }
        if selected.is_none_or(|previous| previous.github.run_attempt < receipt.github.run_attempt)
        {
            selected = Some(receipt);
        }
    }
    if ambiguous { None } else { selected }
}

/// Retains the particular jobs/receipt file when decoding or validation fails.
#[ohno::error]
#[display("cannot read publication evidence {}", path.display())]
struct ReceiptReadError {
    path: PathBuf,
}

mod evidence;

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
            path: PathBuf::from(format!("{phase}-{attempt}/outcome.json")),
            publication_id: publication.id.clone(),
            phase: phase.to_owned(),
            complete,
            github: context(attempt),
            target: None,
            batch_id: None,
            batches: Vec::new(),
            errors: Vec::new(),
            summaries: Vec::new(),
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
            .complete
        );
        assert!(
            !assess(
                Some(&publication),
                &receipts,
                &jobs(JobResult::Cancelled),
                context(3)
            )
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
                .complete
            );
        }
    }

    #[test]
    fn requires_manifest_and_phase_receipts_even_when_workflow_jobs_succeed() {
        let publication = publication();
        let jobs = jobs(JobResult::Skipped);
        assert!(!assess(None, &[], &jobs, context(1)).complete);
        assert!(!assess(Some(&publication), &[], &jobs, context(1)).complete);
        let receipts = vec![
            receipt(&publication, "registry", 1, true),
            receipt(&publication, "github", 1, true),
        ];
        assert!(assess(Some(&publication), &receipts, &jobs, context(1)).complete);
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
            .complete
        );
        receipts.push(receipt(&publication, "registry", 2, true));
        assert!(
            !assess(
                Some(&publication),
                &receipts,
                &jobs(JobResult::Skipped),
                context(2),
            )
            .complete
        );
    }

    #[test]
    fn incomplete_tag_reconciliation_retains_exact_manual_recovery() {
        let publication = publication();
        let mut github = receipt(&publication, "github", 1, false);
        github
            .errors
            .push("Create tool-v1.0.0 at immutable-source then retry original run".to_owned());
        let mut workflow = jobs(JobResult::Skipped);
        workflow.github = JobResult::Failure;
        let report = assess(
            Some(&publication),
            &[receipt(&publication, "registry", 1, true), github],
            &workflow,
            context(1),
        );
        assert!(!report.complete);
        assert!(
            report
                .details
                .iter()
                .any(|detail| detail.contains("tool-v1.0.0"))
        );
    }

    #[test]
    fn informational_success_summaries_do_not_make_delivery_incomplete() {
        let publication = publication();
        let mut registry = receipt(&publication, "registry", 1, true);
        registry
            .summaries
            .push("An independently completed package.".to_owned());
        let report = assess(
            Some(&publication),
            &[registry, receipt(&publication, "github", 1, true)],
            &jobs(JobResult::Skipped),
            context(1),
        );
        assert!(report.complete);
        assert_eq!(report.details, ["An independently completed package."]);
    }

    #[test]
    fn duplicate_outcomes_name_both_paths_without_discarding_independent_results() {
        let publication = publication();
        let first = receipt(&publication, "registry", 1, true);
        let mut duplicate = receipt(&publication, "registry", 1, true);
        duplicate.path = PathBuf::from("copied/outcome.json");
        let mut github = receipt(&publication, "github", 1, true);
        github.summaries.push("independent tag result".to_owned());
        let result = assess(
            Some(&publication),
            &[first, duplicate, github],
            &jobs(JobResult::Skipped),
            context(1),
        );
        assert!(!result.complete);
        assert!(
            result
                .details
                .iter()
                .any(|line| line.contains("registry-1") && line.contains("copied"))
        );
        assert!(
            result
                .details
                .iter()
                .any(|line| line == "independent tag result")
        );
    }

    #[test]
    fn a_bad_file_does_not_discard_a_valid_collected_outcome() {
        let publication = publication();
        let mut loaded = LoadedEvidence::default();
        collect_receipt(
            &mut loaded,
            PathBuf::from("good/outcome.json"),
            Ok(Some(receipt(&publication, "registry", 1, true))),
        );
        collect_receipt(
            &mut loaded,
            PathBuf::from("bad/outcome.json"),
            Err(InvalidManifest::new("fixture".to_owned()).into()),
        );
        assert_eq!(loaded.receipts.len(), 1);
        assert_eq!(loaded.failures.len(), 1);
        assert_eq!(
            loaded.failures.first().unwrap().0,
            Path::new("bad/outcome.json")
        );
        assert!(
            loaded
                .failures
                .first()
                .unwrap()
                .1
                .find_source::<InvalidManifest>()
                .is_some()
        );
        let mut github = receipt(&publication, "github", 1, true);
        github.summaries.push("independent result".to_owned());
        loaded.receipts.push(github);
        let report = assess_loaded(
            Some(&publication),
            &loaded,
            &jobs(JobResult::Skipped),
            context(1),
        );
        assert!(!report.complete);
        assert!(report.details.first().unwrap().contains("bad/outcome.json"));
        assert!(
            report
                .details
                .iter()
                .any(|detail| detail == "independent result")
        );
    }

    #[test]
    fn report_rendering_preserves_each_detail_and_the_run_identity() {
        let report = render_report(
            "example/tools",
            context(2),
            &Assessment {
                complete: false,
                details: vec![
                    "package failure".to_owned(),
                    "independent success".to_owned(),
                ],
            },
        )
        .unwrap();
        assert!(report.contains("example/tools/actions/runs/123"));
        assert!(report.contains("package failure"));
        assert!(report.contains("independent success"));
    }
}
