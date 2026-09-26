//! OIDC exchange/revocation transport against a local service with fixture credentials.

use std::sync::{Arc, Mutex};

use crp_publication::publication::identity::{ActionsIdentity, TrustedPublisher};
use serde_json::{Value, json};
use tiny_http::{Method, Request, Response, StatusCode};

use crate::http_fixture::HttpService;

#[test]
#[cfg_attr(miri, ignore = "Uses a loopback HTTP service")]
fn exchanges_fresh_oidc_identity_and_revokes_without_exposing_credentials() {
    let service = IdentityService::new(false);
    let identity: ActionsIdentity = serde_json::from_value(json!({
        "request_url": format!("{}/identity?request=fixture", service.http.url()),
        "request_token": "identity-credential-canary"
    }))
    .unwrap();
    let publisher =
        TrustedPublisher::with_endpoint(&format!("{}/tokens", service.http.url())).unwrap();
    let token = publisher.exchange(&identity).unwrap();
    assert_eq!(token, "registry-credential-canary");
    publisher.revoke(&token).unwrap();
    assert_eq!(
        *service.operations.lock().unwrap(),
        ["identity", "exchange", "revoke"]
    );
    assert!(!format!("{identity:?}").contains("credential-canary"));
}

#[test]
#[cfg_attr(miri, ignore = "Uses a loopback HTTP service")]
fn rejected_exchange_does_not_echo_identity_response_bodies() {
    let service = IdentityService::new(true);
    let identity: ActionsIdentity = serde_json::from_value(json!({
        "request_url": format!("{}/identity", service.http.url()),
        "request_token": "identity-credential-canary"
    }))
    .unwrap();
    let publisher =
        TrustedPublisher::with_endpoint(&format!("{}/tokens", service.http.url())).unwrap();
    let error = publisher.exchange(&identity).unwrap_err();
    assert!(!error.to_string().contains("credential-canary"));
    assert_eq!(
        *service.operations.lock().unwrap(),
        ["identity", "exchange"]
    );
}

#[test]
#[cfg_attr(miri, ignore = "Uses a loopback HTTP service")]
fn malformed_success_responses_cannot_echo_credential_values() {
    for malformed_identity in [false, true] {
        let service = HttpService::new(move |_, request| {
            let body = if request.method() == &Method::Get && !malformed_identity {
                r#"{"value":"jwt-credential-canary"}"#
            } else {
                r#""credential-canary""#
            };
            request.respond(Response::from_string(body)).unwrap();
        });
        let identity: ActionsIdentity = serde_json::from_value(json!({
            "request_url":format!("{}/identity",service.url()),
            "request_token":"identity-credential-canary"
        }))
        .unwrap();
        let publisher =
            TrustedPublisher::with_endpoint(&format!("{}/tokens", service.url())).unwrap();
        let error = publisher.exchange(&identity).unwrap_err();
        assert!(!error.to_string().contains("credential-canary"));
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses loopback identity responses and the real HTTP client"
)]
fn empty_credentials_and_invalid_transport_inputs_fail_without_exposing_them() {
    for empty_identity in [false, true] {
        let service = HttpService::new(move |_, request| {
            let body = match request.method() {
                Method::Get if empty_identity => json!({"value":""}),
                Method::Get => json!({"value":"jwt-credential-canary"}),
                Method::Post => json!({"token":""}),
                method => panic!("Unexpected identity operation: {method}"),
            };
            request
                .respond(Response::from_string(body.to_string()))
                .unwrap();
        });
        let identity: ActionsIdentity = serde_json::from_value(json!({
            "request_url":format!("{}/identity",service.url()),
            "request_token":"identity-credential-canary"
        }))
        .unwrap();
        let publisher =
            TrustedPublisher::with_endpoint(&format!("{}/tokens", service.url())).unwrap();
        publisher.exchange(&identity).unwrap_err();
    }

    let service = HttpService::new(|_, request| {
        assert_eq!(request.method(), &Method::Get);
        request
            .respond(Response::from_string(
                r#"{"value":"jwt-credential-canary"}"#,
            ))
            .unwrap();
    });
    for invalid_identity in [false, true] {
        let identity: ActionsIdentity = serde_json::from_value(json!({
            "request_url":if invalid_identity {
                "invalid URL containing credential-canary".to_owned()
            } else {
                format!("{}/identity",service.url())
            },
            "request_token":"identity-credential-canary"
        }))
        .unwrap();
        let publisher =
            TrustedPublisher::with_endpoint("invalid URL containing credential-canary").unwrap();
        let error = publisher.exchange(&identity).unwrap_err();
        assert!(!error.to_string().contains("credential-canary"));
        let error = publisher.revoke("registry-credential-canary").unwrap_err();
        assert!(!error.to_string().contains("credential-canary"));
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
