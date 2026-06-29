//! A self-refreshing GitHub Actions OIDC client-assertion credential for Entra ID.
//!
//! On a GitHub-hosted runner the workflow authenticates to Azure through GitHub's
//! OIDC workload identity federation. A benchmark collection run can take longer
//! than an hour — longer than a single Entra access token lives — so the run must be
//! able to acquire a *new* access token partway through. This credential makes that
//! work by minting a fresh GitHub OIDC assertion on demand, rather than reusing one
//! assertion captured at job start.
//!
//! GitHub exposes two per-job values for this: a token-request URL
//! (`ACTIONS_ID_TOKEN_REQUEST_URL`) and a bearer request token
//! (`ACTIONS_ID_TOKEN_REQUEST_TOKEN`). The request token stays valid for the whole
//! job, so a `GET` against the request URL returns a brand-new, still-valid OIDC JWT
//! at any point during the run. [`ClientAssertionCredential`] calls
//! [`GithubOidcAssertion::secret`] only when its cached access token has gone stale
//! (roughly hourly), so every token exchange presents an assertion minted moments
//! earlier rather than one that has already expired.
//!
//! The alternative — leaning on the Azure CLI session that `azure/login` leaves
//! behind — fails on long runs: that session caches a single OIDC assertion that
//! lives only a few minutes, so the first access-token refresh after it expires
//! re-submits a dead assertion and Entra rejects it (AADSTS700024). Self-minting
//! sidesteps that entirely, and needs neither an `azure/login` step nor a stored
//! secret.

use std::env;
use std::fmt;
use std::sync::Arc;

use azure_core::Error;
use azure_core::credentials::TokenCredential;
use azure_core::error::ErrorKind;
use azure_core::http::headers;
use azure_core::http::{ClientMethodOptions, HttpClient, Method, Request, Url};
use azure_identity::{ClientAssertion, ClientAssertionCredential};
use serde::Deserialize;

use super::StorageError;

/// The audience every GitHub OIDC token minted for Azure federation must carry. It
/// is the fixed value Entra assigns to its token-exchange endpoint and must match
/// the `audience` on the managed identity's federated credential.
const FEDERATION_AUDIENCE: &str = "api://AzureADTokenExchange";

/// The environment variable GitHub sets to the URL that issues an OIDC token for the
/// running job.
const ENV_REQUEST_URL: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";

/// The environment variable GitHub sets to the bearer token authorizing a request
/// against [`ENV_REQUEST_URL`]. It is valid for the whole job; treat it as a secret.
const ENV_REQUEST_TOKEN: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";

/// The environment variable the workflow sets to the client (application) ID of the
/// managed identity to federate into. Its presence is what opts a job into this
/// credential.
const ENV_CLIENT_ID: &str = "AZURE_CLIENT_ID";

/// The environment variable the workflow sets to the Entra tenant (directory) ID to
/// authenticate against.
const ENV_TENANT_ID: &str = "AZURE_TENANT_ID";

/// Builds a self-refreshing GitHub OIDC credential when the process is running in a
/// GitHub Actions job configured for Azure federation, or returns `None` otherwise
/// so the caller falls back to the Azure CLI / developer credential.
///
/// `http_client` is reused for the on-demand OIDC token `GET`, sharing the backend's
/// connection pool. A `Some(Err(..))` result means the job *is* in the federation
/// context but the credential could not be constructed (for example an invalid
/// tenant ID), which the caller surfaces rather than silently falling back.
pub(crate) fn from_env(
    http_client: &Arc<dyn HttpClient>,
) -> Option<Result<Arc<dyn TokenCredential>, StorageError>> {
    let params = params_from_env()?;
    Some(build_credential(params, Arc::clone(http_client)))
}

/// The four values that together opt a job into GitHub OIDC federation.
struct GithubOidcParams {
    request_url: String,
    request_token: String,
    client_id: String,
    tenant_id: String,
}

/// Reads the GitHub OIDC parameters from the process environment.
fn params_from_env() -> Option<GithubOidcParams> {
    params_from(|key| env::var(key).ok())
}

/// Resolves the parameters from an arbitrary getter, so the detection rule is
/// testable without mutating the global process environment. An empty value counts
/// as absent: GitHub leaves an unset request variable as the empty string rather
/// than removing it.
fn params_from(get: impl Fn(&str) -> Option<String>) -> Option<GithubOidcParams> {
    let non_empty = |key| get(key).filter(|value| !value.is_empty());
    Some(GithubOidcParams {
        request_url: non_empty(ENV_REQUEST_URL)?,
        request_token: non_empty(ENV_REQUEST_TOKEN)?,
        client_id: non_empty(ENV_CLIENT_ID)?,
        tenant_id: non_empty(ENV_TENANT_ID)?,
    })
}

/// Constructs the [`ClientAssertionCredential`] wrapping a [`GithubOidcAssertion`].
fn build_credential(
    params: GithubOidcParams,
    http_client: Arc<dyn HttpClient>,
) -> Result<Arc<dyn TokenCredential>, StorageError> {
    let assertion = GithubOidcAssertion {
        request_url: params.request_url,
        request_token: params.request_token,
        http_client,
    };
    let credential: Arc<dyn TokenCredential> =
        ClientAssertionCredential::new(params.tenant_id, params.client_id, assertion, None)
            .map_err(|error| StorageError::Config {
                message: format!("could not initialize GitHub OIDC credential: {error}"),
            })?;
    Ok(credential)
}

/// A [`ClientAssertion`] that fetches a fresh GitHub Actions OIDC token on demand.
struct GithubOidcAssertion {
    /// The per-job GitHub token-request URL (`ACTIONS_ID_TOKEN_REQUEST_URL`).
    request_url: String,
    /// The per-job bearer token authorizing the request (a secret; redacted in
    /// `Debug`).
    request_token: String,
    /// The HTTP client used for the token `GET`, shared with the storage backend.
    http_client: Arc<dyn HttpClient>,
}

impl fmt::Debug for GithubOidcAssertion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Redact the request token: it authorizes minting OIDC tokens for this job
        // and must never reach logs.
        f.debug_struct("GithubOidcAssertion")
            .field("request_url", &self.request_url)
            .finish_non_exhaustive()
    }
}

/// The body GitHub returns from a successful OIDC token request: `{"value":"<jwt>"}`.
#[derive(Deserialize)]
struct OidcTokenResponse {
    value: String,
}

#[async_trait::async_trait]
impl ClientAssertion for GithubOidcAssertion {
    async fn secret(
        &self,
        _options: Option<ClientMethodOptions<'_>>,
    ) -> azure_core::Result<String> {
        let mut url = Url::parse(&self.request_url).map_err(|error| {
            Error::with_message(
                ErrorKind::Credential,
                format!(
                    "invalid GitHub OIDC request URL {:?}: {error}",
                    self.request_url
                ),
            )
        })?;
        // Append rather than replace: the request URL already carries an
        // `api-version` query that must be preserved.
        url.query_pairs_mut()
            .append_pair("audience", FEDERATION_AUDIENCE);

        let mut request = Request::new(url, Method::Get);
        request.insert_header(
            headers::AUTHORIZATION,
            format!("Bearer {}", self.request_token),
        );
        request.insert_header(headers::ACCEPT, "application/json");

        let response = self.http_client.execute_request(&request).await?;
        let status = response.status();
        let body = response.into_body().collect().await?;
        if !status.is_success() {
            return Err(Error::with_message(
                ErrorKind::Credential,
                format!("GitHub OIDC token request failed with HTTP status {status}"),
            ));
        }

        let parsed: OidcTokenResponse = serde_json::from_slice(&body).map_err(|error| {
            Error::with_message(
                ErrorKind::Credential,
                format!("could not parse GitHub OIDC token response: {error}"),
            )
        })?;
        if parsed.value.is_empty() {
            return Err(Error::with_message(
                ErrorKind::Credential,
                "GitHub OIDC token response carried an empty token value",
            ));
        }
        Ok(parsed.value)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use azure_core::http::headers::Headers;
    use azure_core::http::{AsyncRawResponse, StatusCode};
    use futures::executor::block_on;

    /// Records the request a [`StubHttpClient`] saw, so a test can assert on the URL
    /// and bearer header the assertion built.
    #[derive(Clone, Debug, Default)]
    struct SeenRequest {
        url: String,
        authorization: Option<String>,
    }

    /// A minimal [`HttpClient`] that records the request and replies with a fixed
    /// status and body, exercising the assertion's `GET`/parse logic with no real
    /// network (so the tests stay Miri-safe).
    #[derive(Debug)]
    struct StubHttpClient {
        status: StatusCode,
        body: Vec<u8>,
        seen: Mutex<Option<SeenRequest>>,
    }

    impl StubHttpClient {
        fn new(status: StatusCode, body: impl Into<Vec<u8>>) -> Self {
            Self {
                status,
                body: body.into(),
                seen: Mutex::new(None),
            }
        }

        fn seen(&self) -> SeenRequest {
            self.seen
                .lock()
                .unwrap()
                .clone()
                .expect("a request should have been sent")
        }
    }

    #[async_trait::async_trait]
    impl HttpClient for StubHttpClient {
        async fn execute_request(&self, request: &Request) -> azure_core::Result<AsyncRawResponse> {
            let authorization = request
                .headers()
                .get_optional_str(&headers::AUTHORIZATION)
                .map(ToOwned::to_owned);
            *self.seen.lock().unwrap() = Some(SeenRequest {
                url: request.url().to_string(),
                authorization,
            });
            Ok(AsyncRawResponse::from_bytes(
                self.status,
                Headers::default(),
                self.body.clone(),
            ))
        }
    }

    /// Builds an assertion over `client` with a request URL that already carries a
    /// query, so the audience-append behaviour is observable.
    fn assertion(client: Arc<dyn HttpClient>) -> GithubOidcAssertion {
        GithubOidcAssertion {
            request_url: "https://example.test/token?api-version=2.0".to_owned(),
            request_token: "request-secret".to_owned(),
            http_client: client,
        }
    }

    #[test]
    fn secret_returns_token_value_and_sends_audience_and_bearer() {
        let client = Arc::new(StubHttpClient::new(
            StatusCode::Ok,
            br#"{"value":"the-jwt"}"#.to_vec(),
        ));
        let client_for_assertion = Arc::clone(&client);
        let assertion = assertion(client_for_assertion);

        let token = block_on(assertion.secret(None)).expect("token");
        assert_eq!(token, "the-jwt");

        let seen = client.seen();
        // The pre-existing query is preserved and the fixed audience is appended.
        assert!(seen.url.contains("api-version=2.0"), "{}", seen.url);
        assert!(
            seen.url
                .contains("audience=api%3A%2F%2FAzureADTokenExchange"),
            "{}",
            seen.url
        );
        assert_eq!(seen.authorization.as_deref(), Some("Bearer request-secret"));
    }

    #[test]
    fn secret_maps_http_error_status_to_credential_error() {
        let client = Arc::new(StubHttpClient::new(StatusCode::Forbidden, b"nope".to_vec()));
        let error = block_on(assertion(client).secret(None)).expect_err("error");
        assert!(matches!(error.kind(), ErrorKind::Credential), "{error:?}");
    }

    #[test]
    fn secret_maps_malformed_json_to_credential_error() {
        let client = Arc::new(StubHttpClient::new(StatusCode::Ok, b"not json".to_vec()));
        let error = block_on(assertion(client).secret(None)).expect_err("error");
        assert!(matches!(error.kind(), ErrorKind::Credential), "{error:?}");
    }

    #[test]
    fn secret_rejects_an_empty_token_value() {
        let client = Arc::new(StubHttpClient::new(
            StatusCode::Ok,
            br#"{"value":""}"#.to_vec(),
        ));
        let error = block_on(assertion(client).secret(None)).expect_err("error");
        assert!(matches!(error.kind(), ErrorKind::Credential), "{error:?}");
    }

    #[test]
    fn secret_rejects_an_invalid_request_url() {
        let client = Arc::new(StubHttpClient::new(
            StatusCode::Ok,
            br#"{"value":"x"}"#.to_vec(),
        ));
        let mut assertion = assertion(client);
        assertion.request_url = "not a url".to_owned();
        let error = block_on(assertion.secret(None)).expect_err("error");
        assert!(matches!(error.kind(), ErrorKind::Credential), "{error:?}");
    }

    #[test]
    fn debug_redacts_the_request_token() {
        let client = Arc::new(StubHttpClient::new(StatusCode::Ok, b"{}".to_vec()));
        let rendered = format!("{:?}", assertion(client));
        assert!(!rendered.contains("request-secret"), "{rendered}");
        assert!(rendered.contains("example.test"), "{rendered}");
    }

    /// The full set of GitHub OIDC variables, all present and non-empty.
    fn full_env() -> Vec<(&'static str, &'static str)> {
        vec![
            (ENV_REQUEST_URL, "https://example.test/token"),
            (ENV_REQUEST_TOKEN, "secret"),
            (ENV_CLIENT_ID, "client"),
            (ENV_TENANT_ID, "tenant"),
        ]
    }

    /// A getter over an explicit set of variables, so detection is tested without
    /// touching the global process environment.
    fn getter(pairs: Vec<(&'static str, &'static str)>) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn params_present_when_all_four_set() {
        let params = params_from(getter(full_env())).expect("params");
        assert_eq!(params.request_url, "https://example.test/token");
        assert_eq!(params.request_token, "secret");
        assert_eq!(params.client_id, "client");
        assert_eq!(params.tenant_id, "tenant");
    }

    #[test]
    fn params_absent_when_any_var_missing() {
        for missing in [
            ENV_REQUEST_URL,
            ENV_REQUEST_TOKEN,
            ENV_CLIENT_ID,
            ENV_TENANT_ID,
        ] {
            let pairs: Vec<_> = full_env()
                .into_iter()
                .filter(|(name, _)| *name != missing)
                .collect();
            assert!(params_from(getter(pairs)).is_none(), "missing {missing}");
        }
    }

    #[test]
    fn params_absent_when_a_var_is_empty() {
        let pairs: Vec<_> = full_env()
            .into_iter()
            .map(|(name, value)| {
                if name == ENV_CLIENT_ID {
                    (name, "")
                } else {
                    (name, value)
                }
            })
            .collect();
        assert!(params_from(getter(pairs)).is_none());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP pipeline (reqwest) that Miri cannot run"
    )]
    fn build_credential_succeeds_with_valid_params() {
        let http_client: Arc<dyn HttpClient> =
            Arc::new(StubHttpClient::new(StatusCode::Ok, b"{}".to_vec()));
        let params = GithubOidcParams {
            request_url: "https://example.test/token".to_owned(),
            request_token: "secret".to_owned(),
            client_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            tenant_id: "22222222-2222-2222-2222-222222222222".to_owned(),
        };
        let credential = build_credential(params, http_client);
        assert!(credential.is_ok(), "{credential:?}");
    }

    #[test]
    fn build_credential_rejects_an_invalid_tenant_id() {
        let http_client: Arc<dyn HttpClient> =
            Arc::new(StubHttpClient::new(StatusCode::Ok, b"{}".to_vec()));
        let params = GithubOidcParams {
            request_url: "https://example.test/token".to_owned(),
            request_token: "secret".to_owned(),
            client_id: "client".to_owned(),
            // A space is not a legal tenant-ID character; this is rejected before any
            // network pipeline is built.
            tenant_id: "not a valid tenant".to_owned(),
        };
        let error = build_credential(params, http_client).expect_err("error");
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }
}
