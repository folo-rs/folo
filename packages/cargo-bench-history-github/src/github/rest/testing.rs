use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::{Future, ready};
use std::num::NonZero;
use std::time::Duration;

use reqwest::header::HeaderMap;
use reqwest::{Request, StatusCode};
use serde::Serialize;

use crate::github::http::{Http, HttpResponse, TransportError};
use crate::github::rest::{RestGitHub, SecretToken};
use crate::model::Repository;

/// Finite response scripts catch unintended retries or pagination without any external I/O.
#[derive(Debug, Default)]
pub(crate) struct FakeHttp {
    pub(crate) requests: RefCell<Vec<Request>>,
    pub(crate) responses: RefCell<VecDeque<Result<HttpResponse, TransportError>>>,
    pub(crate) delays: RefCell<Vec<Duration>>,
}

impl Http for FakeHttp {
    fn send(&self, request: Request) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        self.requests.borrow_mut().push(request);
        ready(self.responses.borrow_mut().pop_front().unwrap())
    }

    fn sleep(&self, delay: Duration) -> impl Future<Output = ()> {
        self.delays.borrow_mut().push(delay);
        ready(())
    }
}

pub(crate) fn github(responses: impl IntoIterator<Item = HttpResponse>) -> RestGitHub<FakeHttp> {
    RestGitHub::new(
        FakeHttp {
            responses: RefCell::new(responses.into_iter().map(Ok).collect()),
            ..FakeHttp::default()
        },
        SecretToken::select(Some("test-credential".to_owned()), None).unwrap(),
        // Small pages exercise the real pagination boundary without large interpreter fixtures.
        NonZero::new(2).unwrap(),
    )
}

pub(crate) fn response(status: StatusCode, body: impl Serialize) -> HttpResponse {
    HttpResponse {
        status,
        headers: HeaderMap::new(),
        body: serde_json::to_vec(&body).unwrap(),
    }
}

pub(crate) fn repository() -> Repository {
    "folo-rs/folo".parse().unwrap()
}
