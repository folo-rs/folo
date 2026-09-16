use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZero;
use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::AppError;

use crate::github::WorkflowJob;
use crate::model::{CommitSha, Instance, Repository};
use crate::workflow::receipt::{Receipt, validate_platform};

/// Successful receipts in platform order, with platform coverage independent of report coverage.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Selection {
    pub(crate) receipt_indices: Vec<usize>,
    pub(crate) complete: bool,
}

// Workflow setup and job reconciliation must agree on the collection job namespace.
const COLLECTION_JOB_PREFIX: &str = "cbh-collect";

pub(crate) fn collection_job_prefix(instance: &Instance) -> String {
    format!("{COLLECTION_JOB_PREFIX}:{}", instance.as_str())
}

pub(crate) fn reconcile(
    repository: &Repository,
    instance: &Instance,
    run_id: NonZero<u64>,
    head: &CommitSha,
    expected: &BTreeSet<String>,
    jobs: &[WorkflowJob],
    receipts: &[Receipt],
) -> Result<Selection, AppError> {
    let mut attempts = BTreeMap::new();
    let mut latest = BTreeMap::<&str, &WorkflowJob>::new();
    let mut ids = BTreeSet::new();
    let prefix = format!("{}:", collection_job_prefix(instance));
    for job in jobs {
        if job.run_id != run_id || !ids.insert(job.id) {
            return Err(InvalidCollectionJobs::new("mismatched run or duplicate job ID").into());
        }
        let marker = job.name.rsplit(" / ").next().unwrap_or(&job.name);
        let Some(platform) = marker.strip_prefix(&prefix) else {
            continue;
        };
        validate_platform(platform)?;
        if !expected.contains(platform) {
            return Err(InvalidCollectionJobs::new("unexpected collection platform").into());
        }
        if attempts.insert((platform, job.run_attempt), job).is_some() {
            return Err(
                InvalidCollectionJobs::new("duplicate platform jobs in one attempt").into(),
            );
        }
        if latest
            .get(platform)
            .is_none_or(|old| old.run_attempt < job.run_attempt)
        {
            latest.insert(platform, job);
        }
    }

    let mut by_attempt = BTreeMap::new();
    for (index, receipt) in receipts.iter().enumerate() {
        if receipt.repository != *repository
            || receipt.instance != *instance
            || receipt.run_id != run_id
            || receipt.head != *head
            || !expected.contains(&receipt.platform)
            || !attempts.contains_key(&(receipt.platform.as_str(), receipt.run_attempt))
        {
            return Err(MismatchedReceipt::new(&receipt.platform).into());
        }
        if by_attempt
            .insert((receipt.platform.as_str(), receipt.run_attempt), index)
            .is_some()
        {
            return Err(DuplicateReceipt::new(&receipt.platform).into());
        }
    }

    let mut receipt_indices = Vec::new();
    for platform in expected {
        let job = latest
            .get(platform.as_str())
            .ok_or_else(|| InvalidCollectionJobs::new("expected platform has no collection job"))?;
        if !successful(job)? {
            // A failed retry invalidates older success. A platform not rerun still selects
            // its own latest successful attempt. Ref: docs/design.md, Workflow evidence.
            continue;
        }
        let index = by_attempt
            .get(&(platform.as_str(), job.run_attempt))
            .ok_or_else(|| MismatchedReceipt::new(platform))?;
        receipt_indices.push(*index);
    }
    if receipt_indices.is_empty() {
        return Err(NoSuccessfulCollection::new().into());
    }
    Ok(Selection {
        complete: receipt_indices.len() == expected.len(),
        receipt_indices,
    })
}

fn successful(job: &WorkflowJob) -> Result<bool, AppError> {
    if job.status != "completed" {
        return Err(InvalidCollectionJobs::new("latest collection job is not completed").into());
    }
    match job.conclusion.as_deref() {
        Some("success") => Ok(true),
        Some("failure" | "neutral" | "cancelled" | "skipped" | "timed_out" | "action_required") => {
            Ok(false)
        }
        _ => Err(InvalidCollectionJobs::new("latest collection conclusion is unknown").into()),
    }
}

/// Job ambiguity or missing completion facts cannot establish collection success.
#[ohno::error]
#[display("Collection job evidence is inconsistent: {reason}")]
pub(crate) struct InvalidCollectionJobs {
    reason: String,
}

/// A successful job requires the receipt for precisely its run, attempt and frozen head.
#[ohno::error]
#[display("Missing or mismatched collection receipt for platform '{platform}'")]
pub(crate) struct MismatchedReceipt {
    platform: String,
}

/// Multiple artifacts cannot claim the same platform attempt.
#[ohno::error]
#[display("Duplicate collection receipts for platform '{platform}' and attempt")]
pub(crate) struct DuplicateReceipt {
    platform: String,
}

/// An analyzer must never run with fabricated empty evidence after total collection failure.
#[ohno::error]
#[display("No expected platform completed collection successfully")]
pub(crate) struct NoSuccessfulCollection;

impl UnwindSafe for InvalidCollectionJobs {}
impl RefUnwindSafe for InvalidCollectionJobs {}
impl UnwindSafe for MismatchedReceipt {}
impl RefUnwindSafe for MismatchedReceipt {}
impl UnwindSafe for DuplicateReceipt {}
impl RefUnwindSafe for DuplicateReceipt {}
impl UnwindSafe for NoSuccessfulCollection {}
impl RefUnwindSafe for NoSuccessfulCollection {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::workflow::receipt::tests::receipt;

    fn job(platform: &str, attempt: u64, success: bool) -> WorkflowJob {
        let receipt = receipt(platform, attempt);
        WorkflowJob {
            id: NonZero::new(
                attempt
                    .checked_mul(10)
                    .unwrap()
                    .checked_add(u64::from(platform == "linux"))
                    .unwrap(),
            )
            .unwrap(),
            run_id: receipt.run_id,
            run_attempt: receipt.run_attempt,
            name: format!("cbh-collect:folo:{platform}"),
            status: "completed".to_owned(),
            conclusion: Some(if success { "success" } else { "failure" }.to_owned()),
        }
    }

    fn select(jobs: &[WorkflowJob], receipts: &[Receipt]) -> Result<Selection, AppError> {
        let identity = receipt("linux", 1);
        reconcile(
            &identity.repository,
            &identity.instance,
            identity.run_id,
            &identity.head,
            &["linux".to_owned(), "windows".to_owned()].into(),
            jobs,
            receipts,
        )
    }

    #[test]
    fn failed_retry_invalidates_only_its_platform_regardless_of_api_order() {
        let jobs = [
            job("windows", 2, false),
            job("linux", 1, true),
            job("windows", 1, true),
        ];
        let receipts = [receipt("windows", 1), receipt("linux", 1)];
        for jobs in [jobs.to_vec(), jobs.into_iter().rev().collect()] {
            assert_eq!(
                select(&jobs, &receipts).unwrap(),
                Selection {
                    receipt_indices: vec![1],
                    complete: false
                }
            );
        }
    }

    #[test]
    fn successful_retry_combines_with_platform_not_rerun() {
        let mut windows = job("windows", 2, true);
        windows.name = format!("outer / reusable / {}", windows.name);
        let selected = select(
            &[job("windows", 1, false), windows, job("linux", 1, true)],
            &[receipt("windows", 2), receipt("linux", 1)],
        )
        .unwrap();
        assert_eq!(selected.receipt_indices, [1, 0]);
        assert!(selected.complete);
    }

    #[test]
    fn successful_retry_cannot_use_an_older_receipt() {
        let error = select(
            &[
                job("linux", 1, true),
                job("linux", 2, true),
                job("windows", 1, false),
            ],
            &[receipt("linux", 1)],
        )
        .unwrap_err();
        assert!(error.find_source::<MismatchedReceipt>().is_some());
    }

    #[test]
    fn historical_receipts_are_unambiguous_when_their_attempts_differ() {
        let selected = select(
            &[
                job("linux", 2, true),
                job("windows", 1, false),
                job("linux", 1, true),
            ],
            &[receipt("linux", 1), receipt("linux", 2)],
        )
        .unwrap();
        assert_eq!(selected.receipt_indices, [1]);
        assert!(!selected.complete);
    }

    #[test]
    fn unrelated_jobs_and_other_instances_do_not_contribute() {
        let mut other = job("linux", 2, true);
        other.name = "caller / cbh-collect:another:linux".to_owned();
        let mut analysis = job("linux", 3, true);
        analysis.name = "analysis".to_owned();
        analysis.status = "in_progress".to_owned();
        analysis.conclusion = None;
        assert_eq!(
            select(
                &[
                    job("linux", 1, true),
                    job("windows", 1, false),
                    other,
                    analysis
                ],
                &[receipt("linux", 1)],
            )
            .unwrap()
            .receipt_indices,
            [0]
        );
    }

    #[test]
    fn missing_job_and_unknown_job_state_are_explicit_errors() {
        let jobs = [job("linux", 1, true), job("windows", 1, false)];
        assert!(
            select(&jobs[..1], &[receipt("linux", 1)])
                .unwrap_err()
                .find_source::<InvalidCollectionJobs>()
                .is_some()
        );
        for (status, conclusion) in [
            ("in_progress", Some("success")),
            ("unknown", Some("success")),
            ("completed", None),
            ("completed", Some("unknown")),
        ] {
            let mut jobs = jobs.clone();
            jobs[1].status = status.to_owned();
            jobs[1].conclusion = conclusion.map(str::to_owned);
            assert!(
                select(&jobs, &[receipt("linux", 1)])
                    .unwrap_err()
                    .find_source::<InvalidCollectionJobs>()
                    .is_some()
            );
        }
    }

    #[test]
    fn total_failure_does_not_produce_an_empty_selection() {
        let error = select(
            &[job("linux", 1, false), job("windows", 1, false)],
            &[receipt("linux", 1)],
        )
        .unwrap_err();
        assert!(error.find_source::<NoSuccessfulCollection>().is_some());
    }

    #[test]
    fn duplicate_jobs_and_receipts_are_ambiguous_even_when_bytes_match() {
        let jobs = [job("linux", 1, true), job("windows", 1, false)];
        let receipt = receipt("linux", 1);
        assert!(
            select(&jobs, &[receipt.clone(), receipt.clone()])
                .unwrap_err()
                .find_source::<DuplicateReceipt>()
                .is_some()
        );
        let mut duplicate = jobs[0].clone();
        duplicate.id = NonZero::new(99).unwrap();
        assert!(
            select(&[jobs[0].clone(), jobs[1].clone(), duplicate], &[receipt])
                .unwrap_err()
                .find_source::<InvalidCollectionJobs>()
                .is_some()
        );
    }

    #[test]
    fn receipts_bind_every_identity_field_including_failed_platforms() {
        let jobs = [job("linux", 1, true), job("windows", 1, false)];
        let original = receipt("windows", 1);
        let mut wrong_repository = original.clone();
        wrong_repository.repository = "other/repo".parse().unwrap();
        let mut wrong_instance = original.clone();
        wrong_instance.instance = "other".parse().unwrap();
        let mut wrong_run = original.clone();
        wrong_run.run_id = NonZero::new(43).unwrap();
        let mut wrong_head = original.clone();
        wrong_head.head = "b".repeat(40).parse().unwrap();
        let mut wrong_platform = original.clone();
        wrong_platform.platform = "unknown".to_owned();
        let mut wrong_attempt = original;
        wrong_attempt.run_attempt = NonZero::new(2).unwrap();
        for invalid in [
            wrong_repository,
            wrong_instance,
            wrong_run,
            wrong_head,
            wrong_platform,
            wrong_attempt,
        ] {
            assert!(
                select(&jobs, &[receipt("linux", 1), invalid])
                    .unwrap_err()
                    .find_source::<MismatchedReceipt>()
                    .is_some()
            );
        }
    }

    #[test]
    fn unknown_platform_jobs_are_not_ignored() {
        let mut jobs = [job("linux", 1, true), job("windows", 1, false)];
        jobs[1].name = "cbh-collect:folo:unknown".to_owned();
        assert!(
            select(&jobs, &[receipt("linux", 1)])
                .unwrap_err()
                .find_source::<InvalidCollectionJobs>()
                .is_some()
        );
    }
}
