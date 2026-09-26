//! GitHub OIDC exchange and revocation for per-upload Cargo credentials.

use std::any::type_name;
use std::env;
use std::fmt::{self, Debug, Formatter};
use std::time::Duration;

use ohno::AppError;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};

use crate::verbose::Verbose;

/// Verifies caller identity setup without constructing or uploading a package.
pub(crate) fn check_publishing_identity(verbose: Verbose) -> Result<String, AppError> {
    verbose.note(|| {
        "Checking the caller workflow's GitHub OIDC identity against crates.io; \
        this exchanges and immediately revokes a temporary credential without publishing."
            .to_owned()
    });
    let identity = ActionsIdentity::from_environment()?;
    let publisher = TrustedPublisher::new()?;
    let token = publisher.exchange(&identity)?;
    publisher.revoke(&token)?;
    Ok("GitHub OIDC exchange and revocation succeeded. Package-specific publication grants are checked when uploading.".to_owned())
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
            request_url: required_environment("ACTIONS_ID_TOKEN_REQUEST_URL")?,
            request_token: required_environment("ACTIONS_ID_TOKEN_REQUEST_TOKEN")?,
        })
    }

    /// Acquires a fresh JWT for each exchange; crates.io rejects reusing a JWT identity.
    fn token(&self, client: &Client) -> Result<String, AppError> {
        let url = audience_url(&self.request_url)?;
        let response = client
            .get(url)
            .bearer_auth(&self.request_token)
            .send()
            .map_err(|error| {
                IdentityTransport::caused_by("GitHub OIDC request", error.without_url())
            })?;
        let response = successful(response, "GitHub OIDC request")?;
        // Deserializer diagnostics can echo malformed values from a credential-bearing body.
        let token: OidcResponse = response
            .json()
            .map_err(|_sensitive_body| MalformedIdentityResponse::new("GitHub OIDC response"))?;
        if token.value.is_empty() {
            return Err(EmptyCredential::new("GitHub OIDC response").into());
        }
        Ok(token.value)
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
}

impl TrustedPublisher {
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn new() -> Result<Self, AppError> {
        Self::with_endpoint(TOKEN_ENDPOINT)
    }

    // Boundary tests supply a local service; production selection remains fixed to crates.io.
    pub fn with_endpoint(endpoint: &str) -> Result<Self, AppError> {
        let client = Client::builder()
            .timeout(IDENTITY_REQUEST_TIMEOUT)
            .redirect(Policy::none())
            .user_agent(concat!("cargo-release-plan/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| IdentityTransport::caused_by("credential client setup", error))?;
        Ok(Self {
            client,
            endpoint: endpoint.to_owned(),
        })
    }

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
        let response = successful(response, "Trusted Publishing exchange")?;
        let token: ExchangeResponse = response.json().map_err(|_sensitive_body| {
            MalformedIdentityResponse::new("Trusted Publishing response")
        })?;
        if token.token.is_empty() {
            return Err(EmptyCredential::new("Trusted Publishing response").into());
        }
        Ok(token.token)
    }

    pub fn revoke(&self, token: &str) -> Result<(), AppError> {
        let response = self
            .client
            .delete(&self.endpoint)
            .bearer_auth(token)
            .send()
            .map_err(|error| {
                IdentityTransport::caused_by("Trusted Publishing revocation", error.without_url())
            })?;
        successful(response, "Trusted Publishing revocation")?;
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
// Authentication requests are short control operations, not package uploads or compile jobs.
const IDENTITY_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

fn required_environment(name: &'static str) -> Result<String, AppError> {
    let value = env::var(name).map_err(|error| MissingIdentity::caused_by(name, error))?;
    if value.is_empty() {
        return Err(MissingIdentity::new(name).into());
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

fn successful(
    response: reqwest::blocking::Response,
    operation: &'static str,
) -> Result<reqwest::blocking::Response, AppError> {
    if !response.status().is_success() {
        // An identity endpoint can echo request material in its body. Only status and
        // operation enter diagnostics; bearer credentials and JWT bodies never do.
        return Err(IdentityRejected::new(operation, response.status()).into());
    }
    Ok(response)
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
    use super::*;

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
