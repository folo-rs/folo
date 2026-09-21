use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::num::NonZero;

use jiff::Timestamp;
use ohno::AppError;
use reqwest::Method;
use serde::Deserialize;

use crate::errors::InvalidResponseError;
use crate::github::WorkflowJob;
use crate::github::http::Http;
use crate::github::rest::{PaginationError, RestGitHub};
use crate::model::Repository;

impl<H: Http> RestGitHub<H> {
    /// Reads every job attempt needed to decide each platform's latest collection result.
    ///
    /// Stable totals, unique IDs and run identity establish complete discovery before receipts
    /// can authorize analysis. Reused success snapshots retain their original execution attempt.
    /// A PR merge-ref SHA in job metadata does not replace receipt heads.
    pub(crate) async fn list_jobs(
        &self,
        repository: &Repository,
        run_id: NonZero<u64>,
    ) -> Result<Vec<WorkflowJob>, AppError> {
        const FIRST_PAGE: u64 = 1;
        let operation = "listing every workflow job attempt";
        let mut page = FIRST_PAGE;
        let mut total = None;
        let mut jobs = Vec::new();
        let mut seen = HashSet::new();
        loop {
            let mut request = self.request(
                Method::GET,
                repository,
                &format!("actions/runs/{run_id}/jobs"),
            )?;
            request
                .url_mut()
                .query_pairs_mut()
                .append_pair("filter", "all")
                .append_pair("per_page", &self.page_size.to_string())
                .append_pair("page", &page.to_string());
            let response: JobsResponse = self.send_json(operation, request).await?;
            if total.is_some_and(|total| total != response.total_count)
                || response.jobs.len() > self.page_size.get()
                || response
                    .jobs
                    .iter()
                    .any(|snapshot| !seen.insert(snapshot.job.id))
            {
                return Err(PaginationError::new(operation).into());
            }
            if response
                .jobs
                .iter()
                .any(|snapshot| snapshot.job.run_id != run_id)
            {
                return Err(InvalidResponseError::new(operation).into());
            }
            total = Some(response.total_count);
            let last_page = response.jobs.len() < self.page_size.get();
            jobs.extend(response.jobs);
            if jobs.len() > response.total_count
                || (last_page && jobs.len() != response.total_count)
            {
                return Err(PaginationError::new(operation).into());
            }
            if last_page {
                return self.execution_jobs(repository, run_id, jobs).await;
            }
            page = page
                .checked_add(1)
                .ok_or_else(|| PaginationError::new(operation))?;
        }
    }

    /// Removes later-attempt copies only when earlier execution evidence establishes their origin.
    async fn execution_jobs(
        &self,
        repository: &Repository,
        run_id: NonZero<u64>,
        mut snapshots: Vec<JobSnapshot>,
    ) -> Result<Vec<WorkflowJob>, AppError> {
        let operation = "resolving workflow job execution attempts";
        snapshots.sort_unstable_by_key(|snapshot| snapshot.job.run_attempt);
        let mut starts = BTreeMap::new();
        let mut executions = BTreeSet::new();
        let mut jobs = Vec::new();
        for snapshot in snapshots {
            let Some((started, completed)) = snapshot.successful_interval()? else {
                // Unfinished and unsuccessful jobs retain their reported attempt so a failed
                // or pending retry cannot recover coverage from an older successful receipt.
                jobs.push(snapshot.job);
                continue;
            };
            let execution = (snapshot.job.name.clone(), started, completed);
            let attempt = snapshot.job.run_attempt;
            if attempt > NonZero::<u64>::MIN {
                let boundary = if let Some(boundary) = starts.get(&attempt) {
                    *boundary
                } else {
                    let boundary = self.attempt_start(repository, run_id, attempt).await?;
                    starts.insert(attempt, boundary);
                    boundary
                };
                // GitHub copies successful jobs into a rerun with new IDs and run_attempt,
                // but retains their execution times. A completion before this attempt began
                // cannot be a new execution. Ref: docs/implementation.md, "Workflow job history".
                if completed < boundary {
                    if !executions.contains(&execution) {
                        return Err(InvalidResponseError::new(operation).into());
                    }
                    continue;
                }
                if started < boundary {
                    return Err(InvalidResponseError::new(operation).into());
                }
            }
            executions.insert(execution);
            jobs.push(snapshot.job);
        }
        Ok(jobs)
    }

    /// Reads the selected attempt's boundary, not the workflow run's first creation time.
    async fn attempt_start(
        &self,
        repository: &Repository,
        run_id: NonZero<u64>,
        attempt: NonZero<u64>,
    ) -> Result<Timestamp, AppError> {
        let operation = "reading a workflow attempt start";
        let request = self.request(
            Method::GET,
            repository,
            &format!("actions/runs/{run_id}/attempts/{attempt}"),
        )?;
        let response: AttemptResponse = self.send_json(operation, request).await?;
        if response.id != run_id || response.run_attempt != attempt {
            return Err(InvalidResponseError::new(operation).into());
        }
        response
            .run_started_at
            .parse()
            .map_err(|error| InvalidResponseError::caused_by(operation, error).into())
    }
}

/// GitHub wraps job pages in an envelope, unlike issue and comment pages.
///
/// Ref: <https://docs.github.com/en/rest/actions/workflow-jobs#list-jobs-for-a-workflow-run>.
#[derive(Deserialize)]
struct JobsResponse {
    total_count: usize,
    jobs: Vec<JobSnapshot>,
}

/// API snapshots can copy completed executions into later workflow attempts.
///
/// Timing remains nullable for queued or skipped jobs. Only successful executions need
/// a complete interval to establish that an old receipt belongs to reused work.
#[derive(Deserialize)]
struct JobSnapshot {
    #[serde(flatten)]
    job: WorkflowJob,
    started_at: Option<String>,
    completed_at: Option<String>,
}

impl JobSnapshot {
    fn successful_interval(&self) -> Result<Option<(Timestamp, Timestamp)>, AppError> {
        if self.job.status != "completed" || self.job.conclusion.as_deref() != Some("success") {
            return Ok(None);
        }
        let operation = "validating successful workflow job timing";
        let parse = |value: &Option<String>| -> Result<Timestamp, AppError> {
            value
                .as_deref()
                .ok_or_else(|| InvalidResponseError::new(operation))?
                .parse()
                .map_err(|error| InvalidResponseError::caused_by(operation, error).into())
        };
        let started = parse(&self.started_at)?;
        let completed = parse(&self.completed_at)?;
        if completed < started {
            return Err(InvalidResponseError::new(operation).into());
        }
        Ok(Some((started, completed)))
    }
}

/// An attempt-specific boundary distinguishes new jobs from copied historical results.
#[derive(Deserialize)]
struct AttemptResponse {
    id: NonZero<u64>,
    run_attempt: NonZero<u64>,
    run_started_at: String,
}
