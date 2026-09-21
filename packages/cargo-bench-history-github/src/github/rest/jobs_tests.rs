#![allow(
    clippy::indexing_slicing,
    reason = "Tests mutate known JSON fixtures and assert collection sizes before indexing."
)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "Small fixture page and attempt numbers cannot overflow the timestamp calculations."
)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::{Future, ready};
use std::num::NonZero;
use std::slice;
use std::time::Duration;

use futures::executor::block_on;
use jiff::Timestamp;
use reqwest::header::HeaderMap;
use reqwest::{Method, Request, StatusCode};
use serde_json::{Value, json};

use crate::errors::InvalidResponseError;
use crate::github::GitHub;
use crate::github::http::{Http, HttpResponse, TransportError};
use crate::github::rest::{PaginationError, RestGitHub, SecretToken};
use crate::workflow::receipt::tests::receipt;
use crate::workflow::reconcile::{MismatchedReceipt, reconcile};

/// A finite exchange script asserts page progression before returning each response.
struct JobHttp {
    responses: RefCell<VecDeque<(String, StatusCode, Value)>>,
}

impl Http for JobHttp {
    fn send(&self, request: Request) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        let (url, status, body) = self.responses.borrow_mut().pop_front().unwrap();
        assert_eq!(request.method(), Method::GET);
        assert_eq!(request.url().as_str(), url);
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
                responses: RefCell::new(
                    responses
                        .into_iter()
                        .enumerate()
                        .map(|(page, (status, body))| {
                            (
                                format!(
                                    "https://api.github.com/repos/folo-rs/folo/actions/runs/42/jobs?filter=all&per_page=2&page={}",
                                    page + 1
                                ),
                                status,
                                body,
                            )
                        })
                        .collect(),
                ),
            },
            SecretToken::select(Some("job-test-credential".to_owned()), None).unwrap(),
            NonZero::new(2).unwrap(),
        )
}

fn job(id: u64, platform: &str, attempt: u64, conclusion: &str) -> Value {
    // Fixture execution windows are disjoint and follow their attempt's start.
    let start = i64::try_from(attempt).unwrap() * 100;
    json!({
        "id": id, "run_id": 42, "run_attempt": attempt,
        "name": format!("caller / cbh-collect:folo:{platform}"),
        "status": "completed", "conclusion": conclusion,
        "started_at": Timestamp::from_second(start + 1).unwrap().to_string(),
        "completed_at": Timestamp::from_second(start + 2).unwrap().to_string()
    })
}

fn add_attempt(github: &RestGitHub<JobHttp>, attempt: u64, status: StatusCode, body: Value) {
    github.http.responses.borrow_mut().push_back((
        format!("https://api.github.com/repos/folo-rs/folo/actions/runs/42/attempts/{attempt}"),
        status,
        body,
    ));
}

fn attempt(number: u64) -> Value {
    json!({
        "id": 42,
        "run_attempt": number,
        "run_started_at": Timestamp::from_second(i64::try_from(number).unwrap() * 100)
            .unwrap().to_string()
    })
}

fn copied(original: &Value, id: u64, attempt: u64) -> Value {
    let mut copied = original.clone();
    copied["id"] = json!(id);
    copied["run_attempt"] = json!(attempt);
    copied
}

#[test]
fn analysis_only_rerun_preserves_original_receipts_across_snapshot_pages() {
    let linux = job(1, "linux", 1, "success");
    let windows = job(2, "windows", 1, "success");
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 4, "jobs": [
                copied(&windows, 4, 2), copied(&linux, 3, 2)
            ]}),
        ),
        (
            StatusCode::OK,
            json!({"total_count": 4, "jobs": [linux, windows]}),
        ),
        (StatusCode::OK, json!({"total_count": 4, "jobs": []})),
    ]);
    add_attempt(&github, 2, StatusCode::OK, attempt(2));
    let receipts = [receipt("linux", 1), receipt("windows", 1)];
    let identity = &receipts[0];
    let jobs = block_on(github.workflow_jobs(&identity.repository, identity.run_id)).unwrap();
    assert_eq!(jobs.len(), 2);
    let selection = reconcile(
        &identity.repository,
        &identity.instance,
        identity.run_id,
        &identity.head,
        &["linux".to_owned(), "windows".to_owned()].into(),
        &jobs,
        &receipts,
    )
    .unwrap();
    assert_eq!(selection.receipt_indices, [0, 1]);
    assert!(selection.complete);
    assert!(github.http.responses.borrow().is_empty());
}

fn assert_retry_selection(conclusion: &str) {
    let linux = job(1, "linux", 1, "success");
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 4, "jobs": [
                copied(&linux, 3, 2), job(4, "windows", 2, conclusion)
            ]}),
        ),
        (
            StatusCode::OK,
            json!({"total_count": 4, "jobs": [
                linux, job(2, "windows", 1, "success")
            ]}),
        ),
        (StatusCode::OK, json!({"total_count": 4, "jobs": []})),
    ]);
    add_attempt(&github, 2, StatusCode::OK, attempt(2));
    let mut receipts = vec![receipt("linux", 1), receipt("windows", 1)];
    if conclusion == "success" {
        receipts.push(receipt("windows", 2));
    }
    let identity = &receipts[0];
    let jobs = block_on(github.workflow_jobs(&identity.repository, identity.run_id)).unwrap();
    let selection = reconcile(
        &identity.repository,
        &identity.instance,
        identity.run_id,
        &identity.head,
        &["linux".to_owned(), "windows".to_owned()].into(),
        &jobs,
        &receipts,
    )
    .unwrap();
    if conclusion == "success" {
        assert_eq!(selection.receipt_indices, [0, 2]);
        assert!(selection.complete);
    } else {
        assert_eq!(selection.receipt_indices, [0]);
        assert!(!selection.complete);
    }
}

#[test]
fn successful_retry_combines_with_a_copied_platform() {
    assert_retry_selection("success");
}

#[test]
fn failed_retry_invalidates_old_success_beside_a_copied_platform() {
    assert_retry_selection("failure");
}

#[test]
fn a_new_successful_execution_still_requires_its_own_receipt() {
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 2, "jobs": [
                job(1, "linux", 1, "success"), job(2, "linux", 2, "success")
            ]}),
        ),
        (StatusCode::OK, json!({"total_count": 2, "jobs": []})),
    ]);
    add_attempt(&github, 2, StatusCode::OK, attempt(2));
    let receipt = receipt("linux", 1);
    let jobs = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap();
    let error = reconcile(
        &receipt.repository,
        &receipt.instance,
        receipt.run_id,
        &receipt.head,
        &["linux".to_owned()].into(),
        &jobs,
        slice::from_ref(&receipt),
    )
    .unwrap_err();
    assert!(error.find_source::<MismatchedReceipt>().is_some());
}

#[test]
fn repeated_copies_keep_the_first_execution_not_the_previous_snapshot() {
    let original = job(1, "linux", 1, "success");
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 3, "jobs": [
                copied(&original, 3, 3), copied(&original, 2, 2)
            ]}),
        ),
        (
            StatusCode::OK,
            json!({"total_count": 3, "jobs": [original]}),
        ),
    ]);
    add_attempt(&github, 2, StatusCode::OK, attempt(2));
    add_attempt(&github, 3, StatusCode::OK, attempt(3));
    let receipt = receipt("linux", 1);
    let jobs = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id.get(), 1);
    assert_eq!(jobs[0].run_attempt.get(), 1);
}

#[test]
fn copied_success_without_original_execution_evidence_is_rejected() {
    let copied = copied(&job(1, "linux", 1, "success"), 2, 2);
    let github = github(vec![(
        StatusCode::OK,
        json!({"total_count": 1, "jobs": [copied]}),
    )]);
    add_attempt(&github, 2, StatusCode::OK, attempt(2));
    let receipt = receipt("linux", 1);
    let error = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
    assert!(error.find_source::<InvalidResponseError>().is_some());
}

fn assert_invalid_interval(field: &str, value: Value) {
    let mut success = job(1, "linux", 1, "success");
    success[field] = value;
    let github = github(vec![(
        StatusCode::OK,
        json!({"total_count": 1, "jobs": [success]}),
    )]);
    let receipt = receipt("linux", 1);
    let error = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
    assert!(error.find_source::<InvalidResponseError>().is_some());
}

#[test]
fn successful_job_requires_a_start_time() {
    assert_invalid_interval("started_at", Value::Null);
}

#[test]
fn successful_job_requires_a_completion_time() {
    assert_invalid_interval("completed_at", Value::Null);
}

#[test]
fn successful_job_rejects_an_invalid_start_time() {
    assert_invalid_interval("started_at", json!("invalid timestamp"));
}

#[test]
fn successful_job_rejects_an_invalid_completion_time() {
    assert_invalid_interval("completed_at", json!("invalid timestamp"));
}

#[test]
fn successful_job_rejects_a_reversed_interval() {
    assert_invalid_interval("started_at", json!("1970-01-01T00:02:00Z"));
}

#[test]
fn skipped_snapshot_timing_does_not_claim_a_successful_execution() {
    let mut skipped = job(2, "windows", 2, "skipped");
    skipped["completed_at"] = json!("1970-01-01T00:01:42Z");
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 2, "jobs": [job(1, "linux", 1, "success"), skipped]}),
        ),
        (StatusCode::OK, json!({"total_count": 2, "jobs": []})),
    ]);
    let receipt = receipt("linux", 1);
    let jobs = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[1].conclusion.as_deref(), Some("skipped"));
}

fn assert_attempt_failure(status: StatusCode, body: Value) {
    let original = job(1, "linux", 1, "success");
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 2, "jobs": [copied(&original, 2, 2), original]}),
        ),
        (StatusCode::OK, json!({"total_count": 2, "jobs": []})),
    ]);
    add_attempt(&github, 2, status, body);
    let receipt = receipt("linux", 1);
    block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
}

#[test]
fn attempt_metadata_http_failure_is_not_a_fallback() {
    assert_attempt_failure(StatusCode::FORBIDDEN, json!({"message": "denied"}));
}

#[test]
fn attempt_metadata_requires_the_requested_run() {
    let mut response = attempt(2);
    response["id"] = json!(43);
    assert_attempt_failure(StatusCode::OK, response);
}

#[test]
fn attempt_metadata_requires_the_requested_attempt() {
    let mut response = attempt(2);
    response["run_attempt"] = json!(3);
    assert_attempt_failure(StatusCode::OK, response);
}

#[test]
fn attempt_metadata_rejects_an_invalid_start_time() {
    let mut response = attempt(2);
    response["run_started_at"] = json!("invalid");
    assert_attempt_failure(StatusCode::OK, response);
}

#[test]
fn attempt_metadata_requires_a_start_time() {
    assert_attempt_failure(StatusCode::OK, json!({"id": 42, "run_attempt": 2}));
}

#[test]
fn success_spanning_the_attempt_boundary_is_ambiguous() {
    let mut success = job(1, "linux", 2, "success");
    success["started_at"] = json!("1970-01-01T00:01:41Z");
    let github = github(vec![(
        StatusCode::OK,
        json!({"total_count": 1, "jobs": [success]}),
    )]);
    add_attempt(&github, 2, StatusCode::OK, attempt(2));
    let receipt = receipt("linux", 1);
    let error = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
    assert!(error.find_source::<InvalidResponseError>().is_some());
}

#[test]
fn completion_on_the_attempt_boundary_does_not_prove_reuse() {
    let mut original = job(1, "linux", 1, "success");
    original["completed_at"] = attempt(2)["run_started_at"].clone();
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 2, "jobs": [copied(&original, 2, 2), original]}),
        ),
        (StatusCode::OK, json!({"total_count": 2, "jobs": []})),
    ]);
    add_attempt(&github, 2, StatusCode::OK, attempt(2));
    let receipt = receipt("linux", 1);
    let error = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
    assert!(error.find_source::<InvalidResponseError>().is_some());
}

#[test]
fn a_job_can_start_at_the_attempt_boundary_and_finish_in_the_same_timestamp() {
    let mut success = job(1, "linux", 2, "success");
    success["started_at"] = attempt(2)["run_started_at"].clone();
    success["completed_at"] = success["started_at"].clone();
    let github = github(vec![(
        StatusCode::OK,
        json!({"total_count": 1, "jobs": [success]}),
    )]);
    add_attempt(&github, 2, StatusCode::OK, attempt(2));
    let receipt = receipt("linux", 2);
    let jobs = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].run_attempt.get(), 2);
}

#[test]
fn a_failed_execution_cannot_prove_a_later_success_is_a_copy() {
    let original = job(1, "linux", 1, "failure");
    let mut success = copied(&original, 2, 2);
    success["conclusion"] = json!("success");
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 2, "jobs": [original, success]}),
        ),
        (StatusCode::OK, json!({"total_count": 2, "jobs": []})),
    ]);
    add_attempt(&github, 2, StatusCode::OK, attempt(2));
    let receipt = receipt("linux", 1);
    let error = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
    assert!(error.find_source::<InvalidResponseError>().is_some());
}

#[test]
fn an_unfinished_retry_cannot_disappear_despite_old_completion_fields() {
    let original = job(1, "linux", 1, "success");
    let mut unfinished = copied(&original, 2, 2);
    unfinished["status"] = json!("in_progress");
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 2, "jobs": [original, unfinished]}),
        ),
        (StatusCode::OK, json!({"total_count": 2, "jobs": []})),
    ]);
    let receipt = receipt("linux", 1);
    let jobs = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[1].status, "in_progress");
    reconcile(
        &receipt.repository,
        &receipt.instance,
        receipt.run_id,
        &receipt.head,
        &["linux".to_owned()].into(),
        &jobs,
        slice::from_ref(&receipt),
    )
    .unwrap_err();
}

#[test]
fn all_attempt_pages_drive_selection_without_using_merge_ref_sha() {
    let mut linux = job(1, "linux", 1, "success");
    linux["head_sha"] = json!("b".repeat(40));
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 3, "jobs": [
                job(3, "windows", 2, "failure"), linux
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
    let error = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
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

fn assert_invalid_page(second: Value) {
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
    let error = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
    assert!(error.find_source::<PaginationError>().is_some());
}

#[test]
fn missing_records_are_not_a_successful_partial_list() {
    assert_invalid_page(json!({"total_count": 3, "jobs": []}));
}

#[test]
fn repeated_job_ids_across_pages_are_rejected() {
    assert_invalid_page(json!({"total_count": 3, "jobs": [job(1, "linux", 1, "success")]}));
}

#[test]
fn an_incomplete_page_with_changed_total_is_rejected() {
    assert_invalid_page(json!({"total_count": 4, "jobs": [job(3, "linux", 2, "success")]}));
}

#[test]
fn an_oversized_page_is_rejected_even_when_the_final_total_would_match() {
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 3, "jobs": [
                job(1, "linux", 1, "success"), job(2, "windows", 1, "success"),
                job(3, "other", 1, "success")
            ]}),
        ),
        (StatusCode::OK, json!({"total_count": 3, "jobs": []})),
    ]);
    let receipt = receipt("linux", 1);
    let error = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
    assert!(error.find_source::<PaginationError>().is_some());
}

#[test]
fn changed_totals_are_rejected_even_when_later_pages_complete_the_new_total() {
    let github = github(vec![
        (
            StatusCode::OK,
            json!({"total_count": 3, "jobs": [
                job(1, "linux", 1, "success"), job(2, "windows", 1, "success")
            ]}),
        ),
        (
            StatusCode::OK,
            json!({"total_count": 4, "jobs": [
                job(3, "other", 1, "success"), job(4, "extra", 1, "success")
            ]}),
        ),
        (StatusCode::OK, json!({"total_count": 4, "jobs": []})),
    ]);
    let receipt = receipt("linux", 1);
    let error = block_on(github.workflow_jobs(&receipt.repository, receipt.run_id)).unwrap_err();
    assert!(error.find_source::<PaginationError>().is_some());
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
