//! Exact-version registry observations against local response fixtures.

use crp_impl::publication::registry::RegistryClient;
use tiny_http::{Response, StatusCode};

use crate::http_fixture::HttpService;

#[test]
#[cfg_attr(miri, ignore = "Uses a loopback HTTP service")]
fn absence_is_distinct_from_query_failure_or_mismatched_identity() {
    let service = HttpService::new(|_, request| {
        let (status, body) = match request.url() {
            "/index/pr/es/present" => (200, r#"{"name":"present","vers":"1.0.0","yanked":true}"#),
            "/index/mi/ss/missing" => (404, "{}"),
            "/index/fa/il/failed" => (503, "{}"),
            "/index/wr/on/wrong" => (200, r#"{"name":"another","vers":"1.0.0"}"#),
            "/index/br/ok/broken" => (200, "{}"),
            path => panic!("Unexpected fixture request: {path}"),
        };
        request
            .respond(Response::from_string(body).with_status_code(StatusCode(status)))
            .unwrap();
    });
    let client = RegistryClient::with_endpoint(&format!("{}/index", service.url())).unwrap();
    assert!(client.contains("present", "1.0.0").unwrap());
    assert!(!client.contains("present", "1.0.1").unwrap());
    assert!(!client.contains("missing", "1.0.0").unwrap());
    for name in ["failed", "wrong", "broken"] {
        client.contains(name, "1.0.0").unwrap_err();
    }
}
