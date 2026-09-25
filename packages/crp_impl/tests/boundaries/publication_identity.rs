//! OIDC exchange/revocation transport against a local service with fixture credentials.

use std::sync::{Arc, Mutex};

use crp_impl::publication::identity::{ActionsIdentity, TrustedPublisher};
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

/// A disposable identity endpoint; stopping it wakes a blocked receiver without a timer.
pub(crate) struct IdentityService {
    http: HttpService,
    operations: Arc<Mutex<Vec<&'static str>>>,
}

impl IdentityService {
    pub(crate) fn new(reject_exchange: bool) -> Self {
        let operations = Arc::new(Mutex::new(Vec::new()));
        let http = HttpService::new({
            let operations = Arc::clone(&operations);
            move |_, request| respond(request, &operations, reject_exchange)
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

fn respond(mut request: Request, operations: &Mutex<Vec<&'static str>>, reject_exchange: bool) {
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
        request.respond(Response::empty(StatusCode(204))).unwrap();
    }
}
