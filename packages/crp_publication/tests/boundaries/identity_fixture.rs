//! Shared OIDC exchange/revocation service for publication boundary scenarios.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tiny_http::{Method, Request, Response, StatusCode};

use crate::http_fixture::HttpService;

/// Records distinct exchanges and exact revocations for one multi-upload session.
#[derive(Default)]
struct LeaseState {
    assertions: usize,
    tokens: Vec<String>,
    revoked: Vec<String>,
}

/// Owns an exact-token lease fixture without sharing production credential values.
pub(crate) struct LeaseService {
    http: HttpService,
    state: Arc<Mutex<LeaseState>>,
}

impl LeaseService {
    pub(crate) fn new(reject_first: bool) -> Self {
        let state = Arc::new(Mutex::new(LeaseState::default()));
        let http = HttpService::new({
            let state = Arc::clone(&state);
            move |_, mut request| {
                let (status, response) = match request.method() {
                    Method::Get => {
                        let ordinal = {
                            let mut state = state.lock().unwrap();
                            state.assertions = state.assertions.checked_add(1).unwrap();
                            state.assertions
                        };
                        (
                            200,
                            serde_json::json!({"value":format!("assertion-{ordinal}")}),
                        )
                    }
                    Method::Post => {
                        let mut body = String::new();
                        request.as_reader().read_to_string(&mut body).unwrap();
                        let body: Value = serde_json::from_str(&body).unwrap();
                        let (ordinal, token) = {
                            let mut state = state.lock().unwrap();
                            let ordinal = state.tokens.len().checked_add(1).unwrap();
                            let token = format!("lease-{ordinal}");
                            state.tokens.push(token.clone());
                            (ordinal, token)
                        };
                        assert_eq!(body.get("jwt").unwrap(), &format!("assertion-{ordinal}"));
                        (200, serde_json::json!({"token":token}))
                    }
                    Method::Delete => {
                        let token = request
                            .headers()
                            .iter()
                            .find(|header| header.field.equiv("Authorization"))
                            .unwrap()
                            .value
                            .as_str()
                            .strip_prefix("Bearer ")
                            .unwrap()
                            .to_owned();
                        let (known, first) = {
                            let mut state = state.lock().unwrap();
                            let known = state.tokens.contains(&token);
                            let first = state.revoked.is_empty();
                            state.revoked.push(token);
                            (known, first)
                        };
                        assert!(known);
                        (
                            if reject_first && first { 403 } else { 204 },
                            serde_json::json!({}),
                        )
                    }
                    _ => panic!("unexpected lease operation"),
                };
                request
                    .respond(
                        Response::from_string(response.to_string())
                            .with_status_code(StatusCode(status)),
                    )
                    .unwrap();
            }
        });
        Self { http, state }
    }

    pub(crate) fn url(&self) -> &str {
        self.http.url()
    }

    pub(crate) fn observations(&self) -> (usize, Vec<String>, Vec<String>) {
        let state = self.state.lock().unwrap();
        (
            state.assertions,
            state.tokens.clone(),
            state.revoked.clone(),
        )
    }
}

/// A disposable identity endpoint; stopping it wakes a blocked receiver without a timer.
pub(crate) struct IdentityService {
    http: HttpService,
    operations: Arc<Mutex<Vec<&'static str>>>,
}

impl IdentityService {
    pub(crate) fn new(reject_exchange: bool) -> Self {
        Self::with_failures(reject_exchange, false)
    }

    pub(crate) fn with_failures(reject_exchange: bool, reject_revocation: bool) -> Self {
        let operations = Arc::new(Mutex::new(Vec::new()));
        let http = HttpService::new({
            let operations = Arc::clone(&operations);
            move |_, request| respond(request, &operations, reject_exchange, reject_revocation)
        });
        Self { http, operations }
    }

    pub(crate) fn url(&self) -> &str {
        self.http.url()
    }

    pub(crate) fn operations(&self) -> Vec<&'static str> {
        self.operations.lock().unwrap().clone()
    }
}

fn respond(
    mut request: Request,
    operations: &Mutex<Vec<&'static str>>,
    reject_exchange: bool,
    reject_revocation: bool,
) {
    let authorization = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Authorization"))
        .map(|header| header.value.as_str().to_owned());
    if request.method() == &Method::Get {
        assert!(request.url().contains("audience=crates.io"));
        assert_eq!(
            authorization.as_deref(),
            Some("Bearer identity-credential-canary")
        );
        operations.lock().unwrap().push("identity");
        request
            .respond(Response::from_string(
                r#"{"value":"jwt-credential-canary"}"#,
            ))
            .unwrap();
    } else if request.method() == &Method::Post {
        // Exchange authenticates through the JSON assertion, not a registry bearer token.
        assert!(authorization.is_none());
        let mut body = String::new();
        request.as_reader().read_to_string(&mut body).unwrap();
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body.get("jwt").unwrap(), "jwt-credential-canary");
        operations.lock().unwrap().push("exchange");
        let (status, body) = if reject_exchange {
            (400, r#"{"error":"rejected jwt-credential-canary"}"#)
        } else {
            (200, r#"{"token":"registry-credential-canary"}"#)
        };
        request
            .respond(Response::from_string(body).with_status_code(StatusCode(status)))
            .unwrap();
    } else {
        assert_eq!(request.method(), &Method::Delete);
        assert_eq!(
            authorization.as_deref(),
            Some("Bearer registry-credential-canary")
        );
        operations.lock().unwrap().push("revoke");
        request
            .respond(Response::empty(StatusCode(if reject_revocation {
                403
            } else {
                204
            })))
            .unwrap();
    }
}
