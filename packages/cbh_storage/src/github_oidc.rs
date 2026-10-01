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

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

use azure_core::Error;
use azure_core::credentials::TokenCredential;
use azure_core::error::ErrorKind;
use azure_core::http::{
    ClientMethodOptions, HttpClient, Method, Request, StatusCode, Url, headers,
};
use azure_core::sleep::sleep;
use azure_core::time::Duration;
use azure_identity::{ClientAssertion, ClientAssertionCredential};
use serde::Deserialize;

use crate::{StorageConfigurationError, StorageError};

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

/// Short exponential waits absorb brief issuer disruptions without prolonged authentication stalls.
/// Ref: ../docs/implementation.md, "GitHub OIDC acquisition".
const RETRY_DELAYS: [Duration; 3] = [
    Duration::seconds(1),
    Duration::seconds(2),
    Duration::seconds(4),
];

/// Retry the Azure SDK's established transient status set, not arbitrary server errors.
const RETRY_STATUSES: &[StatusCode] = &[
    StatusCode::RequestTimeout,
    StatusCode::TooManyRequests,
    StatusCode::InternalServerError,
    StatusCode::BadGateway,
    StatusCode::ServiceUnavailable,
    StatusCode::GatewayTimeout,
];

/// The values that together opt a job into GitHub OIDC federation.
struct GithubOidcParams {
    request_url: String,
    request_token: String,
    client_id: String,
    tenant_id: String,
}

/// Builds a self-refreshing GitHub OIDC credential when the process is running in a
/// GitHub Actions job configured for Azure federation, or returns `None` otherwise so
/// the caller falls back to the Azure CLI / developer credential.
///
/// The federation parameters are read through `get`; the caller passes a real
/// process-environment lookup, while tests pass a getter over an explicit set, so the
/// wiring from detected parameters to a built credential is exercised without mutating
/// the global process environment. A `Some(Err(..))` result means the job *is* in the
/// federation context but the credential could not be constructed (for example an
/// invalid tenant ID), which the caller surfaces rather than silently falling back.
/// `http_client` is reused for the on-demand OIDC token `GET`, sharing the backend's
/// connection pool.
pub(crate) fn credential_from(
    get: impl Fn(&str) -> Option<String>,
    http_client: &Arc<dyn HttpClient>,
) -> Option<Result<Arc<dyn TokenCredential>, StorageError>> {
    let params = params_from(get)?;
    Some(build_credential(params, Arc::clone(http_client)))
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
            .map_err(|error| {
                StorageConfigurationError::caused_by(
                    "could not initialize GitHub OIDC credential".to_owned(),
                    error,
                )
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
        // Both the bearer token and the per-job URL can carry sensitive request data.
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
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
        self.secret_with_sleep(sleep).await
    }
}

impl GithubOidcAssertion {
    /// Acquires an assertion with the same retry policy under real and simulated delays.
    async fn secret_with_sleep<F: Future<Output = ()> + Send>(
        &self,
        sleep: impl Fn(Duration) -> F + Send,
    ) -> azure_core::Result<String> {
        let mut url = Url::parse(&self.request_url).map_err(|_error| {
            Error::with_message(ErrorKind::Credential, "invalid GitHub OIDC request URL")
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

        let mut delays = RETRY_DELAYS.into_iter();
        loop {
            let error = match self.request_token(&request).await {
                Ok(token) => return Ok(token),
                Err(error) => error,
            };
            let retryable = match error.kind() {
                ErrorKind::Io | ErrorKind::Connection => true,
                ErrorKind::HttpResponse { status, .. } => RETRY_STATUSES.contains(status),
                _ => false,
            };
            if !retryable {
                return Err(Error::new(ErrorKind::Credential, error));
            }
            let Some(delay) = delays.next() else {
                // The issuer owns its retry budget; do not invite an outer transport retry.
                return Err(Error::with_error(
                    ErrorKind::Credential,
                    error,
                    "GitHub OIDC token request exhausted its retries",
                ));
            };
            sleep(delay).await;
        }
    }

    async fn request_token(&self, request: &Request) -> azure_core::Result<String> {
        let response = self
            .http_client
            .execute_request(request)
            .await
            .map_err(|error| redact_transport_error(&error))?;
        let status = response.status();
        if !status.is_success() {
            // An error body is unnecessary for classification and may contain token data.
            return Err(Error::with_message(
                ErrorKind::HttpResponse {
                    status,
                    error_code: None,
                    raw_response: None,
                },
                format!("GitHub OIDC token request failed with HTTP status {status}"),
            ));
        }

        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|error| redact_transport_error(&error))?;
        let parsed: OidcTokenResponse = serde_json::from_slice(&body).map_err(|_error| {
            // Deserializer errors can quote values from the token-bearing response.
            Error::with_message(
                ErrorKind::Credential,
                "could not parse GitHub OIDC token response",
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

/// Keeps transport classification without retaining an error that may quote credentials.
fn redact_transport_error(error: &Error) -> Error {
    let kind = match error.kind() {
        ErrorKind::Io => ErrorKind::Io,
        ErrorKind::Connection => ErrorKind::Connection,
        _ => ErrorKind::Credential,
    };
    let message = format!("GitHub OIDC token request transport failed ({kind:?})");
    Error::with_message(kind, message)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::VecDeque;
    use std::iter;
    use std::sync::Mutex;

    use azure_core::http::AsyncRawResponse;
    use azure_core::http::headers::Headers;
    use futures::executor::block_on;
    use futures::{future, stream};
    use ohno::ErrorExt as _;

    use super::*;
    use crate::StorageConfigurationError;

    /// Records the request a [`StubHttpClient`] saw, so a test can assert on the URL
    /// and bearer header the assertion built.
    #[derive(Clone, Debug, PartialEq)]
    struct SeenRequest {
        url: String,
        method: Method,
        authorization: Option<String>,
        accept: Option<String>,
    }

    /// Drives complete request sequences in process, rejecting any unexpected extra attempt.
    #[derive(Debug)]
    struct StubHttpClient {
        responses: Mutex<VecDeque<azure_core::Result<AsyncRawResponse>>>,
        seen: Mutex<Vec<SeenRequest>>,
    }

    impl StubHttpClient {
        fn new(status: StatusCode, body: impl Into<Vec<u8>>) -> Self {
            Self::scripted([Ok(AsyncRawResponse::from_bytes(
                status,
                Headers::default(),
                body.into(),
            ))])
        }

        fn scripted(
            responses: impl IntoIterator<Item = azure_core::Result<AsyncRawResponse>>,
        ) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                seen: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> SeenRequest {
            self.seen.lock().unwrap().last().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl HttpClient for StubHttpClient {
        async fn execute_request(&self, request: &Request) -> azure_core::Result<AsyncRawResponse> {
            let authorization = request
                .headers()
                .get_optional_str(&headers::AUTHORIZATION)
                .map(ToOwned::to_owned);
            self.seen.lock().unwrap().push(SeenRequest {
                url: request.url().to_string(),
                method: request.method(),
                authorization,
                accept: request
                    .headers()
                    .get_optional_str(&headers::ACCEPT)
                    .map(ToOwned::to_owned),
            });
            let response = self.responses.lock().unwrap().pop_front();
            response.unwrap()
        }
    }

    /// Builds an assertion over `client` with a request URL that already carries a
    /// query, so the audience-append behaviour is observable.
    fn assertion(client: Arc<impl HttpClient + 'static>) -> GithubOidcAssertion {
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
    fn secret_retries_transient_statuses_then_returns_a_fresh_token() {
        for status in [
            StatusCode::RequestTimeout,
            StatusCode::TooManyRequests,
            StatusCode::InternalServerError,
            StatusCode::BadGateway,
            StatusCode::ServiceUnavailable,
            StatusCode::GatewayTimeout,
        ] {
            let client = Arc::new(StubHttpClient::scripted([
                Ok(AsyncRawResponse::from_bytes(
                    status,
                    Headers::default(),
                    "not a token response",
                )),
                Ok(AsyncRawResponse::from_bytes(
                    StatusCode::Ok,
                    Headers::default(),
                    br#"{"value":"fresh-token"}"#.to_vec(),
                )),
            ]));
            let assertion = assertion(Arc::clone(&client));
            let token = block_on(assertion.secret_with_sleep(|delay| {
                assert_eq!(client.seen.lock().unwrap().len(), 1);
                assert_eq!(delay, Duration::seconds(1));
                future::ready(())
            }))
            .unwrap();

            assert_eq!(token, "fresh-token");
            let seen = client.seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            let first = seen.first().unwrap();
            assert_eq!(first, seen.get(1).unwrap());
            assert_eq!(first.method, Method::Get);
            assert_eq!(first.accept.as_deref(), Some("application/json"));
        }
    }

    #[test]
    fn secret_bounds_mixed_transient_failures_and_redacts_exhaustion() {
        let client = Arc::new(StubHttpClient::scripted([
            Ok(AsyncRawResponse::from_bytes(
                StatusCode::ServiceUnavailable,
                Headers::default(),
                "response-secret",
            )),
            Err(Error::with_message(ErrorKind::Connection, "request-secret")),
            Ok(AsyncRawResponse::new(
                StatusCode::Ok,
                Headers::default(),
                Box::pin(stream::iter([Err(Error::with_message(
                    ErrorKind::Io,
                    "body-secret",
                ))])),
            )),
            Ok(AsyncRawResponse::from_bytes(
                StatusCode::BadGateway,
                Headers::default(),
                "final-response-secret",
            )),
        ]));
        let assertion = assertion(Arc::clone(&client));
        let waits = Mutex::new(Vec::new());
        let error = block_on(assertion.secret_with_sleep(|delay| {
            let mut waits = waits.lock().unwrap();
            assert_eq!(client.seen.lock().unwrap().len(), waits.len() + 1);
            waits.push(delay);
            future::ready(())
        }))
        .unwrap_err();

        assert!(matches!(error.kind(), ErrorKind::Credential));
        assert_eq!(
            error.downcast_ref::<Error>().unwrap().http_status(),
            Some(StatusCode::BadGateway)
        );
        assert_eq!(
            *waits.lock().unwrap(),
            [
                Duration::seconds(1),
                Duration::seconds(2),
                Duration::seconds(4)
            ]
        );
        assert_eq!(client.seen.lock().unwrap().len(), 4);
        let rendered = format!("{error:?} {error}");
        for canary in ["request-secret", "response-secret", "body-secret"] {
            assert!(!rendered.contains(canary));
        }
    }

    #[test]
    fn secret_retries_transport_and_body_failures() {
        for kind in [ErrorKind::Connection, ErrorKind::Io] {
            for body_failure in [false, true] {
                let error = Error::with_message(kind.clone(), "request-secret");
                let failure = if body_failure {
                    Ok(AsyncRawResponse::new(
                        StatusCode::Ok,
                        Headers::default(),
                        Box::pin(stream::iter([Err(error)])),
                    ))
                } else {
                    Err(error)
                };
                let client = Arc::new(StubHttpClient::scripted([
                    failure,
                    Ok(AsyncRawResponse::from_bytes(
                        StatusCode::Ok,
                        Headers::default(),
                        br#"{"value":"fresh-token"}"#.to_vec(),
                    )),
                ]));
                let assertion = assertion(Arc::clone(&client));
                let token = block_on(assertion.secret_with_sleep(|delay| {
                    assert_eq!(client.seen.lock().unwrap().len(), 1);
                    assert_eq!(delay, Duration::seconds(1));
                    future::ready(())
                }))
                .unwrap();
                assert_eq!(token, "fresh-token");
                assert_eq!(client.seen.lock().unwrap().len(), 2);
            }
        }
    }

    #[test]
    fn secret_does_not_retry_non_transient_statuses_or_read_their_bodies() {
        for status in [
            StatusCode::BadRequest,
            StatusCode::Unauthorized,
            StatusCode::Forbidden,
            StatusCode::NotFound,
            StatusCode::NotImplemented,
        ] {
            let client = Arc::new(StubHttpClient::scripted([Ok(AsyncRawResponse::new(
                status,
                Headers::default(),
                Box::pin(stream::poll_fn(|_| panic!("error body must not be read"))),
            ))]));
            let error = block_on(assertion(client).secret(None)).unwrap_err();
            assert!(matches!(error.kind(), ErrorKind::Credential));
            assert_eq!(
                error.downcast_ref::<Error>().unwrap().http_status(),
                Some(status)
            );
        }
    }

    #[test]
    fn secret_redacts_non_transient_transport_and_parser_errors() {
        let clients = [
            StubHttpClient::scripted([Err(Error::with_message(
                ErrorKind::Credential,
                "request-secret",
            ))]),
            StubHttpClient::new(StatusCode::Ok, br#""response-secret""#.to_vec()),
        ];
        for client in clients {
            let error = block_on(assertion(Arc::new(client)).secret(None)).unwrap_err();
            assert!(matches!(error.kind(), ErrorKind::Credential));
            let rendered = format!("{error:?} {error}");
            for canary in ["request-secret", "response-secret"] {
                assert!(!rendered.contains(canary));
            }
        }
    }

    #[test]
    fn secret_stops_retrying_after_non_transient_rejection() {
        let client = Arc::new(StubHttpClient::scripted([
            Ok(AsyncRawResponse::from_bytes(
                StatusCode::ServiceUnavailable,
                Headers::default(),
                "",
            )),
            Ok(AsyncRawResponse::from_bytes(
                StatusCode::Forbidden,
                Headers::default(),
                "",
            )),
        ]));
        let assertion = assertion(Arc::clone(&client));
        let error = block_on(assertion.secret_with_sleep(|delay| {
            assert_eq!(delay, Duration::seconds(1));
            assert_eq!(client.seen.lock().unwrap().len(), 1);
            future::ready(())
        }))
        .unwrap_err();
        assert!(matches!(error.kind(), ErrorKind::Credential));
        assert_eq!(
            error.downcast_ref::<Error>().unwrap().http_status(),
            Some(StatusCode::Forbidden)
        );
        assert_eq!(client.seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn secret_succeeds_on_the_final_attempt() {
        let client = Arc::new(StubHttpClient::scripted(
            RETRY_DELAYS
                .iter()
                .map(|_| Err(Error::with_message(ErrorKind::Io, "request-secret")))
                .chain([Ok(AsyncRawResponse::from_bytes(
                    StatusCode::Ok,
                    Headers::default(),
                    br#"{"value":"fresh-token"}"#.to_vec(),
                ))]),
        ));
        let token =
            block_on(assertion(Arc::clone(&client)).secret_with_sleep(|_| future::ready(())))
                .unwrap();
        assert_eq!(token, "fresh-token");
        assert_eq!(client.seen.lock().unwrap().len(), 4);
    }

    #[test]
    fn secret_redacts_exhausted_transport_errors() {
        let client = Arc::new(StubHttpClient::scripted(
            iter::repeat_with(|| Err(Error::with_message(ErrorKind::Io, "request-secret")))
                .take(RETRY_DELAYS.len() + 1),
        ));
        let error =
            block_on(assertion(Arc::clone(&client)).secret_with_sleep(|_| future::ready(())))
                .unwrap_err();
        assert!(matches!(error.kind(), ErrorKind::Credential));
        assert_eq!(client.seen.lock().unwrap().len(), 4);
        assert!(!format!("{error:?} {error}").contains("request-secret"));
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
        assertion.request_url = "not a url: request-secret".to_owned();
        let error = block_on(assertion.secret(None)).expect_err("error");
        assert!(matches!(error.kind(), ErrorKind::Credential), "{error:?}");
        assert!(!format!("{error:?} {error}").contains("request-secret"));
    }

    #[test]
    fn debug_redacts_the_request_credentials() {
        let client = Arc::new(StubHttpClient::new(StatusCode::Ok, b"{}".to_vec()));
        let rendered = format!("{:?}", assertion(client));
        assert!(!rendered.contains("request-secret"), "{rendered}");
        assert!(!rendered.contains("example.test"), "{rendered}");
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
        assert!(error.find_source::<StorageConfigurationError>().is_some());
        assert!(error.find_source::<Error>().is_some());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP pipeline (reqwest) that Miri cannot run"
    )]
    fn credential_from_builds_a_credential_when_all_params_present() {
        let http_client: Arc<dyn HttpClient> =
            Arc::new(StubHttpClient::new(StatusCode::Ok, b"{}".to_vec()));
        // The tenant and client IDs must be legal GUIDs so the assertion credential
        // actually constructs (the env getter's strings flow straight through).
        let present = vec![
            (ENV_REQUEST_URL, "https://example.test/token"),
            (ENV_REQUEST_TOKEN, "secret"),
            (ENV_CLIENT_ID, "11111111-1111-1111-1111-111111111111"),
            (ENV_TENANT_ID, "22222222-2222-2222-2222-222222222222"),
        ];
        let result =
            credential_from(getter(present), &http_client).expect("params present yields Some");
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn credential_from_absent_when_a_var_is_missing() {
        let http_client: Arc<dyn HttpClient> =
            Arc::new(StubHttpClient::new(StatusCode::Ok, b"{}".to_vec()));
        assert!(credential_from(getter(Vec::new()), &http_client).is_none());
    }
}
