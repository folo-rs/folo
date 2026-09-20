#![cfg_attr(coverage_nightly, coverage(off))]

use std::collections::BTreeMap;
use std::io::{Error as IoError, ErrorKind};
use std::num::NonZero;
use std::time::{Duration, SystemTime};

use futures::executor::block_on;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue, RETRY_AFTER, USER_AGENT,
};
use reqwest::{Method, Request, StatusCode};
use serde_json::{Value, json};
use tick::Clock;

use crate::cli::{PendingArgs, RunArgs};
use crate::errors::{InvalidResponseError, RequestFailedError, UnexpectedStatusError};
use crate::github::GitHub;
use crate::github::http::TransportError;
use crate::github::rest::testing::{github, repository, response};
use crate::github::rest::{InvalidArtifactIdError, PaginationError, RestGitHub, SecretToken};
use crate::identity::IssueIdentity;
use crate::lifecycle::{Report, alert, comment_preflight, find_issue, issue_report};
use crate::model::{CommitSha, IssueKind};
use crate::operations::Context;
use crate::result::tests::evidence;
use crate::result::{AnalysisMode, Outcome, PublicationState};
use crate::{marker, message};

fn issue(number: u64, title: &str, body: &str, state: &str) -> Value {
    json!({ "number": number, "title": title, "body": body, "state": state })
}

fn search(items: &[Value], total: usize) -> Value {
    json!({ "total_count": total, "incomplete_results": false, "items": items })
}

fn context() -> Context {
    Context {
        repository: repository(),
        instance: "project".parse().unwrap(),
        verbose: false,
    }
}

fn owner() -> PendingArgs {
    PendingArgs {
        run: RunArgs {
            run_id: NonZero::new(42).unwrap(),
            run_attempt: NonZero::new(2).unwrap(),
        },
        head: "a".repeat(40).parse().unwrap(),
    }
}

fn report() -> Report {
    Report {
        owner: owner(),
        evidence: evidence(AnalysisMode::History, Outcome::Findings, true),
        summary: "Rendered summary".to_owned(),
        artifact_url: None,
    }
}

fn assert_request(request: &Request, method: &Method, path: &str, body: Option<Value>) {
    assert_eq!(request.method(), method);
    assert_eq!(request.url().scheme(), "https");
    assert_eq!(request.url().host_str(), Some("api.github.com"));
    assert_eq!(request.url().path(), path);
    assert_eq!(request.headers()[ACCEPT], "application/vnd.github+json");
    assert_eq!(request.headers()[USER_AGENT], "cargo-bench-history-github");
    assert_eq!(request.headers()["X-GitHub-Api-Version"], "2022-11-28");
    assert_eq!(request.headers()[AUTHORIZATION], "Bearer test-credential");
    assert!(request.headers()[AUTHORIZATION].is_sensitive());
    assert!(!format!("{request:?}").contains("test-credential"));
    if let Some(body) = body {
        assert_eq!(request.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(
            serde_json::from_slice::<Value>(request.body().unwrap().as_bytes().unwrap()).unwrap(),
            body
        );
    } else {
        assert!(request.body().is_none());
    }
}

#[test]
fn title_search_encodes_literal_phrase_and_uses_complete_metadata_pagination() {
    let github = github([
        response(
            StatusCode::OK,
            search(
                &[
                    json!({"number": 1, "title": "first"}),
                    json!({"number": 2, "title": "second"}),
                ],
                3,
            ),
        ),
        response(
            StatusCode::OK,
            search(&[json!({"number": 3, "title": "third"})], 3),
        ),
    ]);
    let phrase = "Benchmark history findings for a\"\\b is:closed";
    let candidates = block_on(github.search_issues(&repository(), phrase, false)).unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.number)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(candidates.get(2).unwrap().title, "third");
    let requests = github.http.requests.borrow();
    assert_eq!(requests.len(), 2);
    for (request, page) in requests.iter().zip(["1", "2"]) {
        assert_request(request, &Method::GET, "/search/issues", None);
        let query: BTreeMap<_, _> = request.url().query_pairs().into_owned().collect();
        assert_eq!(
            query.get("q").unwrap(),
            "repo:folo-rs/folo is:issue is:open in:title \"Benchmark history findings for a\\\"\\\\b is:closed\""
        );
        assert_eq!(query.get("page").unwrap(), page);
        assert_eq!(query.get("per_page").unwrap(), "2");
    }
}

#[test]
fn alert_search_includes_closed_and_empty_results_are_complete() {
    let github = github([response(StatusCode::OK, search(&[], 0))]);
    let identity = IssueIdentity::Alert(context().instance, NonZero::new(42).unwrap());
    assert!(
        block_on(github.search_issues(&repository(), &identity.phrase(), true))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        github
            .http
            .requests
            .borrow()
            .first()
            .unwrap()
            .url()
            .query_pairs()
            .find(|(key, _)| key == "q")
            .unwrap()
            .1,
        "repo:folo-rs/folo is:issue in:title \"Benchmark history workflow failed for project (run 42)\""
    );
}

#[test]
fn capped_incomplete_and_malformed_searches_are_not_empty_success() {
    for body in [
        json!({"total_count": 0, "incomplete_results": true, "items": []}),
        json!({"total_count": 1000, "incomplete_results": false, "items": []}),
        json!({"total_count": 1001, "incomplete_results": false, "items": []}),
        json!({"total_count": 0, "items": []}),
        json!({"incomplete_results": false, "items": []}),
        json!({"total_count": -1, "incomplete_results": false, "items": []}),
        search(&[json!({"number": 0, "title": "invalid"})], 1),
        search(&[json!({"number": 1})], 1),
        search(
            &[json!({"number": 1, "title": "PR", "pull_request": {}})],
            1,
        ),
        json!([]),
    ] {
        let github = github([response(StatusCode::OK, body)]);
        block_on(github.search_issues(&repository(), "project", false)).unwrap_err();
        assert_eq!(github.http.requests.borrow().len(), 1);
    }
}

#[test]
fn inconsistent_search_pages_and_page_failures_remain_errors() {
    let first = search(
        &[
            json!({"number": 1, "title": "a"}),
            json!({"number": 2, "title": "b"}),
        ],
        3,
    );
    for later in [
        response(StatusCode::OK, search(&[], 3)),
        response(
            StatusCode::OK,
            search(&[json!({"number": 1, "title": "duplicate"})], 3),
        ),
        response(
            StatusCode::OK,
            search(&[json!({"number": 3, "title": "c"})], 4),
        ),
        response(StatusCode::FORBIDDEN, json!({})),
    ] {
        let github = github([response(StatusCode::OK, &first), later]);
        block_on(github.search_issues(&repository(), "project", false)).unwrap_err();
        assert_eq!(github.http.requests.borrow().len(), 2);
    }
}

#[test]
fn full_search_page_requires_final_empty_page_and_extra_items_are_rejected() {
    let items = [
        json!({"number": 1, "title": "a"}),
        json!({"number": 2, "title": "b"}),
    ];
    let github = github([
        response(StatusCode::OK, search(&items, 2)),
        response(StatusCode::OK, search(&[], 2)),
    ]);
    assert_eq!(
        block_on(github.search_issues(&repository(), "project", false))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(github.http.requests.borrow().len(), 2);
    for (items, total) in [
        (items.to_vec(), 1),
        (vec![json!({"number":1,"title":"a"})], 0),
        (
            vec![
                json!({"number":1,"title":"a"}),
                json!({"number":2,"title":"b"}),
                json!({"number":3,"title":"c"}),
            ],
            3,
        ),
    ] {
        let github = self::github([response(StatusCode::OK, search(&items, total))]);
        block_on(github.search_issues(&repository(), "project", false)).unwrap_err();
    }
}

#[test]
fn selected_issue_uses_fresh_direct_body_and_state_not_indexed_values() {
    let context = context();
    let identity = IssueIdentity::Alert(context.instance.clone(), owner().run.run_id);
    let title = identity.phrase();
    let body = message::failure_issue(
        &context.instance,
        42,
        "https://github.com/folo-rs/folo/actions/runs/42",
    );
    let github = github([
        response(
            StatusCode::OK,
            search(&[issue(9, &title, "stale indexed body", "open")], 1),
        ),
        response(StatusCode::OK, issue(9, &title, &body, "closed")),
    ]);
    let selected = block_on(find_issue(&github, &context, &identity))
        .unwrap()
        .unwrap();
    assert!(!selected.open);
    assert_eq!(selected.body, body);
    let requests = github.http.requests.borrow();
    assert_request(
        requests.get(1).unwrap(),
        &Method::GET,
        "/repos/folo-rs/folo/issues/9",
        None,
    );
}

#[test]
fn stale_search_identity_and_duplicate_exact_candidates_fail_without_writes() {
    let context = context();
    let identity = IssueIdentity::Alert(context.instance.clone(), owner().run.run_id);
    let title = identity.phrase();
    let body = message::failure_issue(
        &context.instance,
        42,
        "https://github.com/folo-rs/folo/actions/runs/42",
    );
    let github = github([
        response(StatusCode::OK, search(&[issue(9, &title, "", "open")], 1)),
        response(StatusCode::OK, issue(9, "Changed title", &body, "closed")),
    ]);
    block_on(find_issue(&github, &context, &identity)).unwrap_err();
    let github = self::github([
        response(
            StatusCode::OK,
            search(
                &[issue(9, &title, "", "open"), issue(10, &title, "", "open")],
                2,
            ),
        ),
        response(StatusCode::OK, search(&[], 2)),
    ]);
    block_on(find_issue(&github, &context, &identity)).unwrap_err();
    assert_eq!(github.http.requests.borrow().len(), 2);
}

#[test]
fn issue_read_validates_number_state_and_kind() {
    for value in [
        issue(10, "title", "", "open"),
        issue(9, "title", "", "unknown"),
        json!({"number":9,"title":"title","body":"","state":"open","pull_request":{}}),
        json!({"number":9,"title":"title","body":""}),
    ] {
        let github = github([response(StatusCode::OK, value)]);
        let error = block_on(github.read_issue(&repository(), 9)).unwrap_err();
        assert!(error.find_source::<InvalidResponseError>().is_some());
    }
}

#[test]
fn issue_writes_update_title_and_body_together_without_close_fields() {
    let body = "Summary Δ with \"JSON\" escapes.";
    let github = github([
        response(StatusCode::CREATED, issue(9, "Title", body, "open")),
        response(StatusCode::OK, json!({})),
    ]);
    let issue = block_on(github.create_issue(&repository(), "Title", body)).unwrap();
    assert!(issue.open);
    block_on(github.update_issue(&repository(), 9, "Updated title", "updated")).unwrap();
    let requests = github.http.requests.borrow();
    assert_request(
        requests.first().unwrap(),
        &Method::POST,
        "/repos/folo-rs/folo/issues",
        Some(json!({"title":"Title","body":body})),
    );
    assert_request(
        requests.get(1).unwrap(),
        &Method::PATCH,
        "/repos/folo-rs/folo/issues/9",
        Some(json!({"title":"Updated title","body":"updated"})),
    );
}

#[test]
fn comment_reads_and_writes_stay_within_the_selected_pr() {
    let github = github([
        response(
            StatusCode::OK,
            json!([{"id":1,"body":"first"},{"id":2,"body":null}]),
        ),
        response(StatusCode::OK, json!([{"id":3,"body":""}])),
        response(StatusCode::CREATED, json!({"id":4,"body":"created"})),
        response(StatusCode::OK, json!({})),
    ]);
    let comments = block_on(github.comments(&repository(), 7)).unwrap();
    assert_eq!(comments.len(), 3);
    assert_eq!(comments.first().unwrap().body, "first");
    assert!(comments.get(1).unwrap().body.is_empty());
    block_on(github.create_comment(&repository(), 7, "created")).unwrap();
    block_on(github.update_comment(&repository(), 4, "updated")).unwrap();
    let requests = github.http.requests.borrow();
    for (request, page) in requests.get(..2).unwrap().iter().zip(["1", "2"]) {
        assert_request(
            request,
            &Method::GET,
            "/repos/folo-rs/folo/issues/7/comments",
            None,
        );
        assert_eq!(
            request
                .url()
                .query_pairs()
                .find(|(key, _)| key == "page")
                .unwrap()
                .1,
            page
        );
    }
    assert_request(
        requests.get(2).unwrap(),
        &Method::POST,
        "/repos/folo-rs/folo/issues/7/comments",
        Some(json!({"body":"created"})),
    );
    assert_request(
        requests.get(3).unwrap(),
        &Method::PATCH,
        "/repos/folo-rs/folo/issues/comments/4",
        Some(json!({"body":"updated"})),
    );
}

#[test]
fn repeated_comment_pages_and_invalid_ids_are_errors() {
    let page = json!([{"id":1,"body":""},{"id":2,"body":""}]);
    let github = github([
        response(StatusCode::OK, &page),
        response(StatusCode::OK, &page),
    ]);
    assert!(
        block_on(github.comments(&repository(), 7))
            .unwrap_err()
            .find_source::<PaginationError>()
            .is_some()
    );
    let github = self::github([]);
    let errors = block_on(async {
        [
            github.read_issue(&repository(), 0).await.unwrap_err(),
            github
                .update_issue(&repository(), 0, "", "")
                .await
                .unwrap_err(),
            github.comments(&repository(), 0).await.unwrap_err(),
            github
                .create_comment(&repository(), 0, "")
                .await
                .unwrap_err(),
            github
                .update_comment(&repository(), 0, "")
                .await
                .unwrap_err(),
            github
                .pull_request_head(&repository(), 0)
                .await
                .unwrap_err(),
        ]
    });
    assert!(
        errors
            .iter()
            .all(|error| error.find_source::<InvalidArtifactIdError>().is_some())
    );
    assert!(github.http.requests.borrow().is_empty());
}

#[test]
fn search_rate_limits_retry_but_permissions_and_query_errors_do_not() {
    for (status, header, value, delay) in [
        (StatusCode::TOO_MANY_REQUESTS, RETRY_AFTER, "7", 7),
        (StatusCode::FORBIDDEN, RETRY_AFTER, "0", 0),
    ] {
        let mut limited = response(status, json!({}));
        limited
            .headers
            .insert(header, HeaderValue::from_str(value).unwrap());
        let github = github([limited, response(StatusCode::OK, search(&[], 0))]);
        block_on(github.search_issues(&repository(), "project", false)).unwrap();
        assert_eq!(*github.http.delays.borrow(), [Duration::from_secs(delay)]);
        let requests = github.http.requests.borrow();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests.first().unwrap().url(),
            requests.get(1).unwrap().url()
        );
    }
    for status in [
        StatusCode::FORBIDDEN,
        StatusCode::UNAUTHORIZED,
        StatusCode::UNPROCESSABLE_ENTITY,
    ] {
        let github = github([response(status, json!({}))]);
        block_on(github.search_issues(&repository(), "project", false)).unwrap_err();
        assert_eq!(github.http.requests.borrow().len(), 1);
        assert!(github.http.delays.borrow().is_empty());
    }
}

#[test]
fn secondary_limit_messages_retry_without_retry_after_or_exhausted_primary_quota() {
    let mut limited = response(
        StatusCode::FORBIDDEN,
        json!({"message":"You have exceeded a secondary rate limit."}),
    );
    limited.headers.insert(
        HeaderName::from_static("x-ratelimit-remaining"),
        HeaderValue::from_static("1"),
    );
    let github = github([limited, response(StatusCode::OK, search(&[], 0))]);
    block_on(github.search_issues(&repository(), "project", false)).unwrap();
    assert_eq!(*github.http.delays.borrow(), [Duration::from_secs(60)]);
    assert_eq!(github.http.requests.borrow().len(), 2);
}

#[test]
fn continued_secondary_limits_exceed_the_bounded_wait_budget() {
    let github = github([(); 2].map(|()| {
        response(
            StatusCode::FORBIDDEN,
            json!({"message":"You have exceeded a secondary rate limit."}),
        )
    }));
    block_on(github.search_issues(&repository(), "project", false)).unwrap_err();
    assert_eq!(*github.http.delays.borrow(), [Duration::from_secs(60)]);
    assert_eq!(github.http.requests.borrow().len(), 2);
}

#[test]
fn exhausted_primary_quota_does_not_retry_before_its_absolute_reset() {
    let mut limited = response(
        StatusCode::FORBIDDEN,
        json!({"message":"API rate limit exceeded"}),
    );
    limited.headers.insert(
        HeaderName::from_static("x-ratelimit-remaining"),
        HeaderValue::from_static("0"),
    );
    // An absolute reset requires a clock-backed calculation; this adapter does not guess.
    limited.headers.insert(
        HeaderName::from_static("x-ratelimit-reset"),
        HeaderValue::from_static("2000000000"),
    );
    let github = github([limited]);
    block_on(github.search_issues(&repository(), "project", false)).unwrap_err();
    assert!(github.http.delays.borrow().is_empty());
    assert_eq!(github.http.requests.borrow().len(), 1);
}

#[test]
fn retry_after_budget_and_transient_attempts_are_bounded() {
    for value in ["61", "-1", "date", "Wed, 16 Sep 2026 12:00:00 GMT"] {
        let mut limited = response(StatusCode::TOO_MANY_REQUESTS, json!({}));
        limited
            .headers
            .insert(RETRY_AFTER, HeaderValue::from_str(value).unwrap());
        let github = github([limited]);
        block_on(github.search_issues(&repository(), "project", false)).unwrap_err();
        assert!(github.http.delays.borrow().is_empty());
    }
    for status in [
        StatusCode::REQUEST_TIMEOUT,
        StatusCode::BAD_GATEWAY,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let github = github([status; 3].map(|status| response(status, json!({}))));
        block_on(github.search_issues(&repository(), "project", false)).unwrap_err();
        assert_eq!(
            *github.http.delays.borrow(),
            [Duration::from_millis(200), Duration::from_millis(800)]
        );
        assert_eq!(github.http.requests.borrow().len(), 3);
    }
}

#[test]
fn update_retries_reuse_captured_date_and_body() {
    let github = github([
        response(StatusCode::SERVICE_UNAVAILABLE, json!({})),
        response(StatusCode::OK, json!({})),
    ]);
    let title = "Benchmark history findings for project (updated 2026-09-17)";
    block_on(github.update_issue(&repository(), 9, title, "report")).unwrap();
    for request in github.http.requests.borrow().iter() {
        assert_request(
            request,
            &Method::PATCH,
            "/repos/folo-rs/folo/issues/9",
            Some(json!({"title":title, "body":"report"})),
        );
    }
    assert_eq!(*github.http.delays.borrow(), [Duration::from_millis(200)]);
}

#[test]
fn creates_do_not_retry_transport_status_or_redirect_failures() {
    for status in [
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::BAD_GATEWAY,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::UNAUTHORIZED,
    ] {
        let github = github([response(status, json!({})), response(status, json!({}))]);
        block_on(github.create_issue(&repository(), "title", "body")).unwrap_err();
        block_on(github.create_comment(&repository(), 7, "body")).unwrap_err();
        assert_eq!(github.http.requests.borrow().len(), 2);
        assert!(github.http.delays.borrow().is_empty());
    }
    let github = github([]);
    github
        .http
        .responses
        .borrow_mut()
        .push_back(Err(TransportError::caused_by(
            true,
            IoError::from(ErrorKind::ConnectionReset),
        )));
    let error = block_on(github.create_issue(&repository(), "title", "body")).unwrap_err();
    assert_eq!(
        error.find_source::<IoError>().unwrap().kind(),
        ErrorKind::ConnectionReset
    );
    assert!(github.http.delays.borrow().is_empty());
}

#[test]
fn secondary_limit_messages_never_authorize_another_comment_post() {
    let github = github([response(
        StatusCode::FORBIDDEN,
        json!({"message":"You have exceeded a secondary rate limit."}),
    )]);
    block_on(github.create_comment(&repository(), 7, "body")).unwrap_err();
    assert!(github.http.delays.borrow().is_empty());
    assert_eq!(github.http.requests.borrow().len(), 1);
}

#[test]
fn ambiguous_create_reconciles_lagging_search_then_direct_content() {
    let context = context();
    let report = report();
    let clock = Clock::new_frozen_at(SystemTime::UNIX_EPOCH);
    let identity = IssueIdentity::Rolling(context.instance.clone());
    let title = identity.title(&clock).unwrap();
    let body = message::regression_issue(
        &context.instance,
        &report.owner,
        &report.evidence,
        report.evidence.publication_state(),
        &report.summary,
        None,
    );
    let github = github([
        response(StatusCode::OK, search(&[], 0)),
        response(StatusCode::BAD_GATEWAY, json!({})),
        response(StatusCode::OK, search(&[], 0)),
        response(
            StatusCode::OK,
            search(&[issue(9, &title, "indexed", "open")], 1),
        ),
        response(StatusCode::OK, issue(9, &title, &body, "open")),
    ]);
    block_on(issue_report(
        &github,
        &context,
        &clock,
        &report,
        PublicationState::Findings,
    ))
    .unwrap();
    let requests = github.http.requests.borrow();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method() == Method::POST)
            .count(),
        1
    );
    assert!(github.http.delays.borrow().is_empty());
}

#[test]
fn ambiguous_create_needs_intended_content_and_bounded_absence_remains_failure() {
    let context = context();
    let title = IssueIdentity::Alert(context.instance.clone(), owner().run.run_id).phrase();
    let body = message::failure_issue(&context.instance, 42, "different content");
    let github = github([
        response(StatusCode::OK, search(&[], 0)),
        response(StatusCode::BAD_GATEWAY, json!({})),
        response(StatusCode::OK, search(&[issue(9, &title, "", "open")], 1)),
        response(StatusCode::OK, issue(9, &title, &body, "open")),
    ]);
    let error = block_on(alert(
        &github,
        &context,
        owner().run.run_id,
        "https://github.com/folo-rs/folo/actions/runs/42",
    ))
    .unwrap_err();
    assert!(error.find_source::<UnexpectedStatusError>().is_some());
    assert_eq!(
        github
            .http
            .requests
            .borrow()
            .iter()
            .filter(|request| request.method() == Method::POST)
            .count(),
        1
    );
    let github = self::github([
        response(StatusCode::OK, search(&[], 0)),
        response(StatusCode::BAD_GATEWAY, json!({})),
        response(StatusCode::OK, search(&[], 0)),
        response(StatusCode::OK, search(&[], 0)),
        response(StatusCode::OK, search(&[], 0)),
    ]);
    block_on(alert(
        &github,
        &context,
        owner().run.run_id,
        "https://github.com/folo-rs/folo/actions/runs/42",
    ))
    .unwrap_err();
    assert_eq!(github.http.requests.borrow().len(), 5);
}

#[test]
fn transport_retries_and_credential_redaction_preserve_error_sources() {
    let github = github([]);
    github.http.responses.borrow_mut().extend([
        Err(TransportError::caused_by(
            true,
            IoError::from(ErrorKind::UnexpectedEof),
        )),
        Ok(response(StatusCode::OK, search(&[], 0))),
    ]);
    block_on(github.search_issues(&repository(), "project", false)).unwrap();
    assert_eq!(*github.http.delays.borrow(), [Duration::from_millis(200)]);
    let github = self::github([response(
        StatusCode::UNAUTHORIZED,
        json!({"message":"test-credential"}),
    )]);
    let error = block_on(github.search_issues(&repository(), "project", false)).unwrap_err();
    assert!(!format!("{error:?}").contains("test-credential"));
    assert!(!format!("{github:?}").contains("test-credential"));
    let github = RestGitHub::new(
        self::github([]).http,
        SecretToken::select(Some("bad\nheader".to_owned()), None).unwrap(),
        NonZero::new(2).unwrap(),
    );
    let error = block_on(github.comments(&repository(), 7)).unwrap_err();
    assert!(error.find_source::<RequestFailedError>().is_some());
    assert!(github.http.requests.borrow().is_empty());
}

#[test]
fn token_selection_prefers_primary_and_rejects_missing_credentials() {
    for (primary, fallback, expected) in [
        (Some("primary"), Some("fallback"), "primary"),
        (None, Some("fallback"), "fallback"),
        (Some(""), Some("fallback"), "fallback"),
        (Some(" \t"), Some("fallback"), "fallback"),
    ] {
        let token =
            SecretToken::select(primary.map(str::to_owned), fallback.map(str::to_owned)).unwrap();
        assert!(!format!("{token:?}").contains(expected));
        let github = RestGitHub::new(github([]).http, token, NonZero::new(2).unwrap());
        let request = github
            .request(Method::GET, &repository(), "issues/9")
            .unwrap();
        assert_eq!(
            request.headers()[AUTHORIZATION],
            format!("Bearer {expected}")
        );
    }
    for (primary, fallback) in [(None, None), (Some(""), None), (None, Some(" "))] {
        SecretToken::select(primary.map(str::to_owned), fallback.map(str::to_owned)).unwrap_err();
    }
}

#[test]
fn exhausted_and_unretryable_transport_errors_remain_failures() {
    for retryable in [true, false] {
        let github = github([]);
        let attempts = if retryable { 3 } else { 1 };
        for _ in 0..attempts {
            github
                .http
                .responses
                .borrow_mut()
                .push_back(Err(TransportError::caused_by(
                    retryable,
                    IoError::from(ErrorKind::UnexpectedEof),
                )));
        }
        let error = block_on(github.update_comment(&repository(), 7, "body")).unwrap_err();
        assert_eq!(
            error.find_source::<IoError>().unwrap().kind(),
            ErrorKind::UnexpectedEof
        );
        assert_eq!(github.http.requests.borrow().len(), attempts);
        assert_eq!(github.http.delays.borrow().len(), attempts - 1);
    }
}

#[test]
fn inconsistent_comparison_relationships_are_not_valid_distances() {
    let base = "a".repeat(40).parse().unwrap();
    let head = "b".repeat(40).parse().unwrap();
    for value in [
        json!({"ahead_by":0}),
        json!({"status":"unknown","ahead_by":0,"behind_by":0}),
        json!({"status":"ahead","ahead_by":0,"behind_by":0}),
        json!({"status":"ahead","ahead_by":1,"behind_by":1}),
        json!({"status":"identical","ahead_by":1,"behind_by":0}),
        json!({"status":"identical","ahead_by":0,"behind_by":1}),
        json!({"status":"behind","ahead_by":0,"behind_by":0}),
        json!({"status":"behind","ahead_by":1,"behind_by":1}),
        json!({"status":"diverged","ahead_by":0,"behind_by":1}),
        json!({"status":"diverged","ahead_by":1,"behind_by":0}),
    ] {
        let github = github([response(StatusCode::OK, value)]);
        block_on(github.compare(&repository(), &base, &head)).unwrap_err();
    }
}

#[test]
fn commit_comparison_and_pull_head_decode_and_reject_invalid_relationships() {
    let base: CommitSha = "a".repeat(40).parse().unwrap();
    let head: CommitSha = "b".repeat(40).parse().unwrap();
    for (status, ahead, behind, expected) in [
        ("ahead", 2, 0, Some(2)),
        ("identical", 0, 0, Some(0)),
        ("behind", 0, 2, None),
        ("diverged", 2, 3, None),
    ] {
        let github = github([response(
            StatusCode::OK,
            json!({"status":status,"ahead_by":ahead,"behind_by":behind}),
        )]);
        assert_eq!(
            block_on(github.compare(&repository(), &base, &head))
                .unwrap()
                .ahead_by,
            expected
        );
    }
    let github = github([
        response(StatusCode::NOT_FOUND, json!({})),
        response(
            StatusCode::OK,
            json!({"status":"ahead","ahead_by":0,"behind_by":0}),
        ),
        response(StatusCode::OK, json!({"head":{"sha":"B".repeat(40)}})),
        response(StatusCode::OK, json!({"head":{"sha":"invalid"}})),
    ]);
    assert_eq!(
        block_on(github.compare(&repository(), &base, &head))
            .unwrap()
            .ahead_by,
        None
    );
    block_on(github.compare(&repository(), &base, &head)).unwrap_err();
    assert_eq!(
        block_on(github.pull_request_head(&repository(), 7)).unwrap(),
        head
    );
    block_on(github.pull_request_head(&repository(), 7)).unwrap_err();
    assert_request(
        github.http.requests.borrow().get(2).unwrap(),
        &Method::GET,
        "/repos/folo-rs/folo/pulls/7",
        None,
    );
}

#[test]
fn owner_markers_reject_old_run_only_placeholders() {
    let context = context();
    let old = format!(
        "<!-- cargo-bench-history:project:run:42:{} -->",
        owner().head.as_str()
    );
    assert!(marker::find_owner(&old, &context.instance).is_none());
    let valid = marker::run_owner(&context.instance, &owner());
    assert_eq!(marker::find_owner(&valid, &context.instance), Some(owner()));
    assert!(marker::find_owner(&format!("{valid}\n{valid}"), &context.instance).is_none());
}

#[test]
fn alert_body_requires_both_project_identity_and_run_identity() {
    let context = context();
    let identity = IssueIdentity::Alert(context.instance.clone(), owner().run.run_id);
    let title = identity.phrase();
    for body in [
        marker::issue(&context.instance, IssueKind::FailureAlert),
        marker::alert_run(&context.instance, 42),
    ] {
        let github = github([
            response(StatusCode::OK, search(&[issue(9, &title, "", "open")], 1)),
            response(StatusCode::OK, issue(9, &title, &body, "open")),
        ]);
        block_on(find_issue(&github, &context, &identity)).unwrap_err();
    }
}

#[test]
fn a_closed_rolling_issue_from_a_stale_search_is_not_an_open_candidate() {
    let context = context();
    let report = report();
    let clock = Clock::new_frozen_at(SystemTime::UNIX_EPOCH);
    let identity = IssueIdentity::Rolling(context.instance.clone());
    let title = identity.title(&clock).unwrap();
    let body = message::regression_issue(
        &context.instance,
        &report.owner,
        &report.evidence,
        report.evidence.publication_state(),
        &report.summary,
        None,
    );
    let github = github([
        response(StatusCode::OK, search(&[issue(9, &title, "", "open")], 1)),
        response(StatusCode::OK, issue(9, &title, &body, "closed")),
    ]);
    block_on(find_issue(&github, &context, &identity)).unwrap_err();
}

#[test]
fn successful_create_responses_must_confirm_both_intended_title_and_content() {
    let context = context();
    let title = IssueIdentity::Alert(context.instance.clone(), owner().run.run_id).phrase();
    let run_url = "https://github.com/folo-rs/folo/actions/runs/42";
    let body = message::failure_issue(&context.instance, 42, run_url);
    for (title, body) in [
        ("Wrong title", body.as_str()),
        (title.as_str(), "Wrong body"),
    ] {
        let github = github([
            response(StatusCode::OK, search(&[], 0)),
            response(StatusCode::CREATED, issue(9, title, body, "open")),
        ]);
        block_on(alert(&github, &context, owner().run.run_id, run_url)).unwrap_err();
        assert_eq!(github.http.requests.borrow().len(), 2);
    }
}

#[test]
fn successful_comment_create_must_confirm_the_requested_body_without_another_post() {
    let context = context();
    let github = github([
        response(StatusCode::OK, json!([])),
        response(
            StatusCode::OK,
            json!({"head":{"sha":owner().head.as_str()}}),
        ),
        response(
            StatusCode::CREATED,
            json!({"id":9,"body":"Different content"}),
        ),
    ]);
    block_on(comment_preflight(&github, &context, 7, "foo", &owner())).unwrap_err();
    let requests = github.http.requests.borrow();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method() == Method::POST)
            .count(),
        1
    );
}

#[test]
fn ambiguous_comment_create_preserves_different_matching_content_and_original_failure() {
    let context = context();
    let other = message::pr_in_progress(&context.instance, "other-package", &owner());
    let github = github([
        response(StatusCode::OK, json!([])),
        response(
            StatusCode::OK,
            json!({"head":{"sha":owner().head.as_str()}}),
        ),
        response(StatusCode::BAD_GATEWAY, json!({})),
        response(StatusCode::OK, json!([{"id":9,"body":other}])),
    ]);
    let error = block_on(comment_preflight(&github, &context, 7, "foo", &owner())).unwrap_err();
    assert!(error.find_source::<UnexpectedStatusError>().is_some());
    assert_eq!(github.http.requests.borrow().len(), 4);
}
