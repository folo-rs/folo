//! Exchanges GitHub OIDC assertions for per-upload crates.io credentials and revokes them.

use std::any::type_name;
use std::env;
use std::fmt::{self, Debug, Formatter};
use std::time::Duration;

use ohno::AppError;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};

use crate::PublicationOutput;

/// Verifies caller identity setup without constructing or uploading a package.
// Ambient identity and production HTTP selection are exercised by executable failure tests;
// verify_identity covers exchange/revocation sequencing without acquiring live credentials.
#[cfg_attr(test, mutants::skip)]
pub fn check_publishing_identity(output: &PublicationOutput) -> Result<String, AppError> {
    output.notes().note(|| {
        "Checking the caller workflow's GitHub OIDC identity against crates.io; \
        this obtains and immediately revokes a short-lived crates.io credential without publishing."
            .to_owned()
    });
    let identity = ActionsIdentity::from_environment()?;
    let publisher = TrustedPublisher::new(output.clone())?;
    verify_identity(
        || publisher.exchange(&identity),
        |token| publisher.revoke(token),
    )
}

fn verify_identity(
    exchange: impl FnOnce() -> Result<String, AppError>,
    revoke: impl FnOnce(&str) -> Result<(), AppError>,
) -> Result<String, AppError> {
    let token = exchange()?;
    revoke(&token)?;
    Ok("GitHub OIDC exchange for a crates.io credential and revocation of that credential succeeded. Package-specific publication grants are checked when uploading.".to_owned())
}

/// Ambient GitHub identity, retained only in private invocation-owned credential state.
#[derive(Deserialize, Serialize)]
pub struct ActionsIdentity {
    request_url: String,
    request_token: String,
}

impl ActionsIdentity {
    pub fn from_environment() -> Result<Self, AppError> {
        Ok(Self {
            request_url: required_environment(
                "ACTIONS_ID_TOKEN_REQUEST_URL",
                env::var("ACTIONS_ID_TOKEN_REQUEST_URL"),
            )?,
            request_token: required_environment(
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN"),
            )?,
        })
    }

    /// Acquires a fresh JWT for each exchange; crates.io rejects reusing a JWT identity.
    // The HTTP exchange and credential-body decoding are covered by publication_identity.
    #[cfg_attr(test, mutants::skip)]
    fn token(&self, client: &Client) -> Result<String, AppError> {
        let url = audience_url(&self.request_url)?;
        let response = client
            .get(url)
            .bearer_auth(&self.request_token)
            .send()
            .map_err(|error| {
                IdentityTransport::caused_by("GitHub OIDC request", error.without_url())
            })?;
        successful(response.status(), "GitHub OIDC request")?;
        // Deserializer diagnostics can echo malformed values from a credential-bearing body.
        let token: OidcResponse = response
            .json()
            .map_err(|_sensitive_body| MalformedIdentityResponse::new("GitHub OIDC response"))?;
        require_credential(token.value, "GitHub OIDC response")
    }
}

impl Debug for ActionsIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

/// Narrow synchronous client for the registry's temporary credential lifecycle.
#[derive(Debug)]
pub struct TrustedPublisher {
    client: Client,
    endpoint: String,
    pub(crate) output: PublicationOutput,
}

impl TrustedPublisher {
    // This field forwarder is observed through the persisted session's loopback exchanges.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn new(output: PublicationOutput) -> Result<Self, AppError> {
        Self::with_endpoint(TOKEN_ENDPOINT, output)
    }

    // Boundary tests supply a local service; production selection remains fixed to crates.io.
    pub fn with_endpoint(endpoint: &str, output: PublicationOutput) -> Result<Self, AppError> {
        let client = Client::builder()
            .timeout(IDENTITY_REQUEST_TIMEOUT)
            .redirect(Policy::none())
            .user_agent(output.user_agent())
            .build()
            .map_err(|error| IdentityTransport::caused_by("credential client setup", error))?;
        Ok(Self {
            client,
            endpoint: endpoint.to_owned(),
            output,
        })
    }

    // Real HTTP and response decoding belong to publication_identity's local-service tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn exchange(&self, identity: &ActionsIdentity) -> Result<String, AppError> {
        let jwt = identity.token(&self.client)?;
        let response = self
            .client
            .post(&self.endpoint)
            .json(&ExchangeRequest { jwt: &jwt })
            .send()
            .map_err(|error| {
                IdentityTransport::caused_by("Trusted Publishing exchange", error.without_url())
            })?;
        successful(response.status(), "Trusted Publishing exchange")?;
        let token: ExchangeResponse = response.json().map_err(|_sensitive_body| {
            MalformedIdentityResponse::new("Trusted Publishing response")
        })?;
        require_credential(token.token, "Trusted Publishing response")
    }

    // publication_identity and publication_credentials observe DELETE and failed revocations.
    #[cfg_attr(test, mutants::skip)]
    pub fn revoke(&self, token: &str) -> Result<(), AppError> {
        let response = self
            .client
            .delete(&self.endpoint)
            .bearer_auth(token)
            .send()
            .map_err(|error| {
                IdentityTransport::caused_by("Trusted Publishing revocation", error.without_url())
            })?;
        successful(response.status(), "Trusted Publishing revocation")?;
        Ok(())
    }
}

/// A JWT returned by the GitHub Actions identity endpoint.
#[derive(Deserialize)]
struct OidcResponse {
    value: String,
}

/// The registry accepts the freshly minted GitHub assertion in this exchange body.
#[derive(Serialize)]
struct ExchangeRequest<'a> {
    jwt: &'a str,
}

/// Registry credentials deliberately do not implement Debug.
#[derive(Deserialize)]
struct ExchangeResponse {
    token: String,
}

// The crates.io Trusted Publishing API uses POST/DELETE on the same resource.
const TOKEN_ENDPOINT: &str = "https://crates.io/api/v1/trusted_publishing/tokens";
// A deliberately short operational allowance for assertion/token exchange and revocation.
// These control operations carry no package upload; failure should leave room for cleanup.
const IDENTITY_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

fn required_environment(
    name: &'static str,
    value: Result<String, env::VarError>,
) -> Result<String, AppError> {
    let value = value.map_err(|error| MissingIdentity::caused_by(name, error))?;
    if value.is_empty() {
        return Err(MissingIdentity::new(name).into());
    }
    Ok(value)
}

fn require_credential(value: String, operation: &'static str) -> Result<String, AppError> {
    if value.trim().is_empty() {
        return Err(EmptyCredential::new(operation).into());
    }
    Ok(value)
}

fn audience_url(url: &str) -> Result<Url, AppError> {
    let mut url = Url::parse(url)
        .map_err(|error| IdentityTransport::caused_by("GitHub OIDC endpoint", error))?;
    let query: Vec<_> = url
        .query_pairs()
        .filter(|(name, _)| name != "audience")
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(query)
        .append_pair("audience", "crates.io");
    Ok(url)
}

fn successful(status: StatusCode, operation: &'static str) -> Result<(), AppError> {
    if !status.is_success() {
        // An identity endpoint can echo request material in its body. Only status and
        // operation enter diagnostics; bearer credentials and JWT bodies never do.
        return Err(IdentityRejected::new(operation, status).into());
    }
    Ok(())
}

#[ohno::error]
#[display("GitHub Actions identity requires {name}")]
struct MissingIdentity {
    name: &'static str,
}

#[ohno::error]
#[display("{operation} could not complete")]
struct IdentityTransport {
    operation: &'static str,
}

#[ohno::error]
#[display(
    "{operation} failed with HTTP {status}; verify the caller workflow and Trusted Publisher registration"
)]
struct IdentityRejected {
    operation: &'static str,
    status: StatusCode,
}

#[ohno::error]
#[display("{operation} returned an empty credential")]
struct EmptyCredential {
    operation: &'static str,
}

#[ohno::error]
#[display("{operation} returned an invalid credential response")]
struct MalformedIdentityResponse {
    operation: &'static str,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn identity_check_revokes_the_exact_issued_credential_before_reporting_success() {
        let exchanged = Cell::new(false);
        let revoked = Cell::new(false);
        let result = verify_identity(
            || {
                exchanged.set(true);
                Ok("lease-canary".to_owned())
            },
            |token| {
                assert!(exchanged.get());
                assert_eq!(token, "lease-canary");
                revoked.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(revoked.get());
        assert!(result.contains("revocation"));
        assert!(!result.contains("lease-canary"));
        let error = verify_identity(
            || Err(EmptyCredential::new("exchange").into()),
            |_| panic!("no credential was issued"),
        )
        .unwrap_err();
        assert!(error.find_source::<EmptyCredential>().is_some());
        let error = verify_identity(
            || Ok("lease-canary".to_owned()),
            |_| Err(IdentityRejected::new("revocation", StatusCode::FORBIDDEN).into()),
        )
        .unwrap_err();
        assert!(error.find_source::<IdentityRejected>().is_some());
    }

    #[test]
    fn required_identity_values_distinguish_absence_empty_and_invalid_unicode() {
        assert_eq!(
            required_environment("identity", Ok("credential-canary".to_owned())).unwrap(),
            "credential-canary"
        );
        for value in [
            Ok(String::new()),
            Err(env::VarError::NotPresent),
            Err(env::VarError::NotUnicode(OsString::from(
                "sensitive-canary",
            ))),
        ] {
            let error = required_environment("identity", value).unwrap_err();
            assert_eq!(
                error.find_source::<MissingIdentity>().unwrap().name,
                "identity"
            );
        }
    }

    #[test]
    fn identity_status_accepts_only_success_without_consuming_a_response_body() {
        for status in [StatusCode::OK, StatusCode::CREATED, StatusCode::NO_CONTENT] {
            successful(status, "identity").unwrap();
        }
        for status in [
            StatusCode::CONTINUE,
            StatusCode::FOUND,
            StatusCode::UNAUTHORIZED,
            StatusCode::NOT_FOUND,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let error = successful(status, "identity").unwrap_err();
            let rejected = error.find_source::<IdentityRejected>().unwrap();
            assert_eq!(rejected.status, status);
            assert_eq!(rejected.operation, "identity");
        }
    }

    #[test]
    fn empty_protocol_credentials_are_rejected_without_echoing_their_value() {
        for value in ["", " \t"] {
            let error = require_credential(value.to_owned(), "identity response").unwrap_err();
            assert!(error.find_source::<EmptyCredential>().is_some());
        }
        assert_eq!(
            require_credential("credential-canary".into(), "identity response").unwrap(),
            "credential-canary"
        );
    }

    #[test]
    fn selects_registry_audience_without_dropping_other_oidc_parameters() {
        for input in [
            "https://example.invalid/token?request=fixture",
            "https://example.invalid/token?audience=other&request=fixture",
        ] {
            let url = audience_url(input).unwrap();
            let pairs: Vec<_> = url.query_pairs().collect();
            assert!(
                pairs
                    .iter()
                    .any(|(name, value)| name == "request" && value == "fixture")
            );
            assert_eq!(
                pairs.iter().filter(|(name, _)| name == "audience").count(),
                1
            );
            assert!(
                pairs
                    .iter()
                    .any(|(name, value)| name == "audience" && value == "crates.io")
            );
        }
    }
}
