use std::future::Future;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::time::Duration;

use ohno::AppError;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Client, Request, StatusCode, redirect, retry};

use crate::errors::RequestFailedError;

/// Executes complete HTTP exchanges and the adapter's requested retry delays.
///
/// Keeping both external effects here allows tests to drive the same REST logic without
/// network access, a runtime, or a real clock.
pub(crate) trait Http {
    fn send(&self, request: Request) -> impl Future<Output = Result<HttpResponse, TransportError>>;

    fn sleep(&self, delay: Duration) -> impl Future<Output = ()>;
}

/// The reqwest implementation of the companion's external HTTP boundary.
#[derive(Debug)]
pub(crate) struct ReqwestHttp {
    client: Client,
}

impl ReqwestHttp {
    pub(crate) fn new() -> Result<Self, AppError> {
        // Bound a stalled connection and the entire response body, not just its headers.
        // These budgets allow degraded service while still releasing the workflow runner.
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
        const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

        let client = Client::builder()
            // Redirects must not forward credentials or turn a create into another request.
            .redirect(redirect::Policy::none())
            // Only our semantic retry policy knows which operations can be repeated safely.
            .retry(retry::never())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| RequestFailedError::caused_by("building the HTTP client", error))?;
        Ok(Self { client })
    }
}

impl Http for ReqwestHttp {
    // The only network primitive; request policy and response interpretation run above it
    // and are exercised with the in-process HTTP fake.
    #[cfg_attr(test, mutants::skip)]
    async fn send(&self, request: Request) -> Result<HttpResponse, TransportError> {
        let transport_error = |error: reqwest::Error| {
            let retryable =
                error.is_timeout() || error.is_connect() || error.is_request() || error.is_body();
            TransportError::caused_by(retryable, error)
        };
        let response = self
            .client
            .execute(request)
            .await
            .map_err(transport_error)?;
        let status = response.status();
        let headers = response.headers().clone();
        // Reading is part of the exchange even for deletes and unsuccessful statuses.
        // A truncated body must never be converted to an apparent successful response.
        let body = response.bytes().await.map_err(transport_error)?.to_vec();
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    fn sleep(&self, delay: Duration) -> impl Future<Output = ()> {
        tokio::time::sleep(delay)
    }
}

/// A complete response, before GitHub status and JSON interpretation.
#[derive(Debug)]
pub(crate) struct HttpResponse {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
}

/// An unsuccessful exchange retaining the original transport or body-read error.
#[ohno::error]
#[display("HTTP exchange failed")]
pub(crate) struct TransportError {
    retryable: bool,
}

// No mutation is exposed through this immutable failure record.
impl UnwindSafe for TransportError {}
impl RefUnwindSafe for TransportError {}

impl TransportError {
    pub(crate) fn retryable(&self) -> bool {
        self.retryable
    }
}

// Short pauses absorb transient service failures; the final attempt has no following delay.
pub(crate) const RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(200), Duration::from_millis(800)];

// GitHub recommends a minute before retrying a secondary limit without Retry-After.
// Larger waits are left to workflow retry rather than tying up the runner.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

pub(crate) fn retry_delay(response: &HttpResponse, fallback: Duration) -> Option<Duration> {
    let rate_limited = response.status == StatusCode::TOO_MANY_REQUESTS
        || (response.status == StatusCode::FORBIDDEN
            && (response.headers.contains_key(RETRY_AFTER)
                || response
                    .headers
                    .get("x-ratelimit-remaining")
                    .is_some_and(|value| value == "0")));
    let transient = rate_limited
        || response.status == StatusCode::REQUEST_TIMEOUT
        || response.status.is_server_error();
    if !transient {
        return None;
    }
    if let Some(value) = response.headers.get(RETRY_AFTER) {
        // Accept bounded delta-seconds. Unsupported dates or excessive waits fail without
        // retrying early; interpreting a date would require a separate wall-clock policy.
        let seconds = value.to_str().ok()?.parse::<u64>().ok()?;
        let delay = Duration::from_secs(seconds);
        return (delay <= MAX_RETRY_AFTER).then_some(delay);
    }
    Some(if rate_limited {
        MAX_RETRY_AFTER
    } else {
        fallback
    })
}
