use std::collections::HashSet;
use std::num::NonZero;

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
    /// can authorize analysis. A PR merge-ref SHA in job metadata does not replace receipt heads.
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
                || response.jobs.iter().any(|job| !seen.insert(job.id))
            {
                return Err(PaginationError::new(operation).into());
            }
            if response.jobs.iter().any(|job| job.run_id != run_id) {
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
                return Ok(jobs);
            }
            page = page
                .checked_add(1)
                .ok_or_else(|| PaginationError::new(operation))?;
        }
    }
}

/// GitHub wraps job pages in an envelope, unlike issue and comment pages.
///
/// Ref: <https://docs.github.com/en/rest/actions/workflow-jobs#list-jobs-for-a-workflow-run>.
#[derive(Deserialize)]
struct JobsResponse {
    total_count: usize,
    jobs: Vec<WorkflowJob>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::future::{Future, ready};
    use std::slice;
    use std::time::Duration;

    use futures::executor::block_on;
    use reqwest::header::HeaderMap;
    use reqwest::{Request, StatusCode};
    use serde_json::{Value, json};

    use super::*;
    use crate::github::GitHub;
    use crate::github::http::{HttpResponse, TransportError};
    use crate::github::rest::SecretToken;
    use crate::workflow::receipt::tests::receipt;
    use crate::workflow::reconcile::reconcile;

    /// A finite exchange script asserts page progression before returning each response.
    struct JobHttp {
        responses: RefCell<VecDeque<(StatusCode, Value)>>,
        page: RefCell<u64>,
    }

    impl Http for JobHttp {
        fn send(
            &self,
            request: Request,
        ) -> impl Future<Output = Result<HttpResponse, TransportError>> {
            let mut page = self.page.borrow_mut();
            *page = page.checked_add(1).unwrap();
            assert_eq!(request.method(), Method::GET);
            assert_eq!(
                request.url().as_str(),
                format!(
                    "https://api.github.com/repos/folo-rs/folo/actions/runs/42/jobs?filter=all&per_page=2&page={page}"
                )
            );
            let (status, body) = self.responses.borrow_mut().pop_front().unwrap();
            ready(Ok(HttpResponse {
                status,
                headers: HeaderMap::new(),
                body: serde_json::to_vec(&body).unwrap(),
            }))
        }

        async fn sleep(&self, _delay: Duration) {
            panic!("these fixtures never permit a retry");
        }
    }

    fn github(responses: Vec<(StatusCode, Value)>) -> RestGitHub<JobHttp> {
        RestGitHub::new(
            JobHttp {
                responses: RefCell::new(responses.into()),
                page: RefCell::new(0),
            },
            SecretToken::select(Some("job-test-credential".to_owned()), None).unwrap(),
            NonZero::new(2).unwrap(),
        )
    }

    fn job(id: u64, platform: &str, attempt: u64, conclusion: &str) -> Value {
        json!({
            "id": id, "run_id": 42, "run_attempt": attempt,
            "name": format!("caller / cbh-collect:folo:{platform}"),
            "status": "completed", "conclusion": conclusion,
            "head_sha": "b".repeat(40),
            "steps": [], "runner_name": "runner"
        })
    }

    #[test]
    fn all_attempt_pages_drive_selection_without_using_merge_ref_sha() {
        let github = github(vec![
            (
                StatusCode::OK,
                json!({"total_count": 3, "jobs": [
                    job(3, "windows", 2, "failure"), job(1, "linux", 1, "success")
                ]}),
            ),
            (
                StatusCode::OK,
                json!({"total_count": 3, "jobs": [
                    job(2, "windows", 1, "success")
                ]}),
            ),
        ]);
        let receipt = receipt("linux", 1);
        let jobs = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap();
        let selected = reconcile(
            &receipt.repository,
            &receipt.instance,
            receipt.run_id,
            &receipt.head,
            &["linux".to_owned(), "windows".to_owned()].into(),
            &jobs,
            slice::from_ref(&receipt),
        )
        .unwrap();
        assert_eq!(selected.receipt_indices, [0]);
        assert!(!selected.complete);
        assert!(github.http.responses.borrow().is_empty());
    }

    #[test]
    fn missing_attempt_is_not_assumed_to_be_the_current_attempt() {
        let mut value = job(1, "linux", 1, "success");
        value.as_object_mut().unwrap().remove("run_attempt");
        let github = github(vec![(
            StatusCode::OK,
            json!({"total_count": 1, "jobs": [value]}),
        )]);
        let receipt = receipt("linux", 1);
        let error =
            block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
        assert!(error.find_source::<InvalidResponseError>().is_some());
    }

    #[test]
    fn an_exactly_full_page_requires_the_final_empty_page() {
        let github = github(vec![
            (
                StatusCode::OK,
                json!({"total_count": 2, "jobs": [
                    job(1, "linux", 1, "success"), job(2, "windows", 1, "success")
                ]}),
            ),
            (StatusCode::OK, json!({"total_count": 2, "jobs": []})),
        ]);
        let receipt = receipt("linux", 1);
        let jobs = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap();
        assert_eq!(jobs.len(), 2);
        assert!(github.http.responses.borrow().is_empty());
    }

    #[test]
    fn incomplete_pages_fail_instead_of_returning_partial_jobs() {
        for second in [
            json!({"total_count": 3, "jobs": []}),
            json!({"total_count": 3, "jobs": [job(1, "linux", 1, "success")]}),
            json!({"total_count": 4, "jobs": [job(3, "linux", 2, "success")]}),
        ] {
            let github = github(vec![
                (
                    StatusCode::OK,
                    json!({"total_count": 3, "jobs": [
                        job(1, "linux", 1, "success"), job(2, "windows", 1, "success")
                    ]}),
                ),
                (StatusCode::OK, second),
            ]);
            let receipt = receipt("linux", 1);
            let error =
                block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
            assert!(error.find_source::<PaginationError>().is_some());
        }
    }

    #[test]
    fn later_page_http_failure_is_not_a_successful_partial_list() {
        let github = github(vec![
            (
                StatusCode::OK,
                json!({"total_count": 3, "jobs": [
                    job(1, "linux", 1, "success"), job(2, "windows", 1, "success")
                ]}),
            ),
            (StatusCode::FORBIDDEN, json!({"message": "denied"})),
        ]);
        let receipt = receipt("linux", 1);
        block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
    }
}
