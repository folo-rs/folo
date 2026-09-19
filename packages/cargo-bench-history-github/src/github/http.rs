use std::future::Future;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::time::Duration;

use ohno::AppError;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Client, Request, StatusCode, redirect, retry};
use serde_json::Value;

use crate::errors::RequestFailedError;

/// Executes complete HTTP exchanges and the adapter's requested retry delays.
///
/// Keeping both external effects here allows tests to drive the same REST logic without
/// network access, a runtime, or a real clock.
pub(crate) trait Http {
    /// Executes one exchange, including the response body, without adding semantic retries.
    fn send(&self, request: Request) -> impl Future<Output = Result<HttpResponse, TransportError>>;

    /// Performs the delay selected by REST policy; fakes record it without using real time.
    fn sleep(&self, delay: Duration) -> impl Future<Output = ()>;
}

/// The reqwest implementation of the companion's external HTTP boundary.
#[derive(Debug)]
pub(crate) struct ReqwestHttp {
    client: Client,
}

impl ReqwestHttp {
    /// Builds the bounded transport while leaving redirects and retry authority to REST policy.
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
    /// Executes the transport exchange selected by REST policy and reads its complete body.
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
        // Reading is part of the exchange even for unsuccessful statuses.
        // A truncated body must never be converted to an apparent successful response.
        let body = response.bytes().await.map_err(transport_error)?.to_vec();
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    /// Implements policy-selected waits; in-process HTTP fakes record them instead.
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
    /// Exposes whether REST policy may retry this transport failure for an idempotent operation.
    pub(crate) fn retryable(&self) -> bool {
        self.retryable
    }
}

// Short pauses absorb transient service failures; the final attempt has no following delay.
pub(crate) const RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(200), Duration::from_millis(800)];

// Use GitHub's minimum secondary-limit wait when no Retry-After is supplied. It is also
// the runner's maximum retry wait; longer or increasing waits beyond it fail explicitly.
// Ref: https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#exceeding-the-rate-limit
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Selects a bounded retry wait from GitHub's status, quota hints and prior throttling delay.
///
/// The REST sender calls this only for operations it can safely repeat. Absence of a supported
/// wait preserves the response failure instead of retrying early or inventing a reset time.
pub(crate) fn retry_delay(
    response: &HttpResponse,
    fallback: Duration,
    previous_delay: Option<Duration>,
) -> Option<Duration> {
    // GitHub requires waiting until the absolute primary-quota reset. Without a clock-backed
    // reset calculation, preserve the response error instead of guessing an earlier retry.
    if response
        .headers
        .get("x-ratelimit-remaining")
        .is_some_and(|value| value == "0")
    {
        return None;
    }
    let rate_limited = response.status == StatusCode::TOO_MANY_REQUESTS
        || (response.status == StatusCode::FORBIDDEN
            && (response.headers.contains_key(RETRY_AFTER)
                || secondary_rate_limit(&response.body)));
    let transient = rate_limited
        || response.status == StatusCode::REQUEST_TIMEOUT
        || response.status.is_server_error();
    if !transient {
        return None;
    }
    let delay = if let Some(value) = response.headers.get(RETRY_AFTER) {
        // Accept bounded delta-seconds. Unsupported dates or excessive waits fail without
        // retrying early; interpreting a date would require a separate wall-clock policy.
        let seconds = value.to_str().ok()?.parse::<u64>().ok()?;
        Duration::from_secs(seconds)
    } else if rate_limited {
        MAX_RETRY_AFTER
    } else {
        fallback
    };
    let delay = if let Some(previous) = previous_delay.filter(|_| rate_limited) {
        // Continued secondary throttling requires exponential backoff, not repeated fixed
        // waits. A required increase beyond our wait budget terminates retries.
        const BACKOFF_MULTIPLIER: u32 = 2;
        delay.max(previous.checked_mul(BACKOFF_MULTIPLIER)?)
    } else {
        delay
    };
    (delay <= MAX_RETRY_AFTER).then_some(delay)
}

/// Recognizes GitHub's secondary-throttling response without treating every forbidden response alike.
fn secondary_rate_limit(body: &[u8]) -> bool {
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    body.get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| message.contains("secondary rate limit"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use reqwest::header::{HeaderName, HeaderValue};

    use super::*;

    fn response(status: StatusCode) -> HttpResponse {
        HttpResponse {
            status,
            headers: HeaderMap::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn secondary_limit_classification_requires_the_error_message() {
        assert!(secondary_rate_limit(
            br#"{"message":"You have exceeded a secondary rate limit."}"#
        ));
        for body in [
            b"not JSON".as_slice(),
            b"{}",
            br#"{"message":null}"#,
            br#"{"message":"Resource not accessible by integration"}"#,
            br#"{"other":"secondary rate limit"}"#,
        ] {
            assert!(!secondary_rate_limit(body));
        }
    }

    #[test]
    fn exhausted_primary_quota_cannot_use_a_relative_retry_after() {
        let mut response = response(StatusCode::TOO_MANY_REQUESTS);
        response.headers.insert(
            HeaderName::from_static("x-ratelimit-remaining"),
            HeaderValue::from_static("0"),
        );
        response
            .headers
            .insert(RETRY_AFTER, HeaderValue::from_static("1"));
        assert_eq!(
            retry_delay(&response, Duration::from_millis(200), None),
            None
        );
    }

    #[test]
    fn rate_limit_backoff_honors_both_server_wait_and_previous_delay() {
        let mut response = response(StatusCode::TOO_MANY_REQUESTS);
        response
            .headers
            .insert(RETRY_AFTER, HeaderValue::from_static("5"));
        assert_eq!(
            retry_delay(
                &response,
                Duration::from_millis(200),
                Some(Duration::from_secs(7))
            ),
            Some(Duration::from_secs(14))
        );
        response
            .headers
            .insert(RETRY_AFTER, HeaderValue::from_static("30"));
        assert_eq!(
            retry_delay(
                &response,
                Duration::from_millis(200),
                Some(Duration::from_secs(7))
            ),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            retry_delay(
                &response,
                Duration::from_millis(200),
                Some(Duration::from_secs(31))
            ),
            None
        );
    }

    #[test]
    fn maximum_retry_after_is_allowed_but_larger_values_are_not() {
        let mut response = response(StatusCode::SERVICE_UNAVAILABLE);
        response
            .headers
            .insert(RETRY_AFTER, HeaderValue::from_static("60"));
        assert_eq!(
            retry_delay(&response, Duration::from_millis(200), None),
            Some(Duration::from_secs(60))
        );
        response
            .headers
            .insert(RETRY_AFTER, HeaderValue::from_static("61"));
        assert_eq!(
            retry_delay(&response, Duration::from_millis(200), None),
            None
        );
    }

    #[test]
    fn ordinary_transient_responses_keep_their_non_rate_limit_backoff() {
        let response = response(StatusCode::BAD_GATEWAY);
        let fallback = Duration::from_millis(800);
        assert_eq!(
            retry_delay(&response, fallback, Some(Duration::from_secs(30))),
            Some(fallback)
        );
    }
}
