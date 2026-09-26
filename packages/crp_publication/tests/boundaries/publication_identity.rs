//! OIDC exchange/revocation transport against a local service with fixture credentials.

use std::sync::Arc;

use crp_publication::PublicationOutput;
use crp_publication::publication::identity::{ActionsIdentity, TrustedPublisher};
use serde_json::json;
use tiny_http::{Method, Response};

use crate::http_fixture::HttpService;
use crate::identity_fixture::IdentityService;

#[test]
#[cfg_attr(miri, ignore = "Uses a loopback HTTP service")]
fn exchanges_fresh_oidc_identity_and_revokes_without_exposing_credentials() {
    let service = IdentityService::new(false);
    let identity: ActionsIdentity = serde_json::from_value(json!({
        "request_url": format!("{}/identity?request=fixture", service.url()),
        "request_token": "identity-credential-canary"
    }))
    .unwrap();
    let publisher = publisher(&format!("{}/tokens", service.url()));
    let token = publisher.exchange(&identity).unwrap();
    assert_eq!(token, "registry-credential-canary");
    publisher.revoke(&token).unwrap();
    assert_eq!(service.operations(), ["identity", "exchange", "revoke"]);
    assert!(!format!("{identity:?}").contains("credential-canary"));
}

#[test]
#[cfg_attr(miri, ignore = "Uses a loopback HTTP service")]
fn rejected_exchange_does_not_echo_identity_response_bodies() {
    let service = IdentityService::new(true);
    let identity: ActionsIdentity = serde_json::from_value(json!({
        "request_url": format!("{}/identity", service.url()),
        "request_token": "identity-credential-canary"
    }))
    .unwrap();
    let error = publisher(&format!("{}/tokens", service.url()))
        .exchange(&identity)
        .unwrap_err();
    assert!(!error.to_string().contains("credential-canary"));
    assert_eq!(service.operations(), ["identity", "exchange"]);
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
        let error = publisher(&format!("{}/tokens", service.url()))
            .exchange(&identity)
            .unwrap_err();
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
        publisher(&format!("{}/tokens", service.url()))
            .exchange(&identity)
            .unwrap_err();
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
        let publisher = publisher("invalid URL containing credential-canary");
        let error = publisher.exchange(&identity).unwrap_err();
        assert!(!error.to_string().contains("credential-canary"));
        let error = publisher.revoke("registry-credential-canary").unwrap_err();
        assert!(!error.to_string().contains("credential-canary"));
    }
}

fn publisher(endpoint: &str) -> TrustedPublisher {
    TrustedPublisher::with_endpoint(
        endpoint,
        PublicationOutput::new("1.2.3", false, Arc::new(crp_diag::Discard)),
    )
    .unwrap()
}
