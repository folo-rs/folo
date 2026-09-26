//! Loopback HTTP service lifetime shared by native publication-boundary tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use tiny_http::{Request, Server};

/// Owns a request loop that can be stopped without polling or timer-based coordination.
pub(crate) struct HttpService {
    url: String,
    server: Arc<Server>,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HttpService {
    pub(crate) fn new(mut respond: impl FnMut(&str, Request) + Send + 'static) -> Self {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let url = format!("http://{}", server.server_addr());
        let stopped = Arc::new(AtomicBool::new(false));
        let thread = thread::spawn({
            let server = Arc::clone(&server);
            let stopped = Arc::clone(&stopped);
            let url = url.clone();
            move || {
                while !stopped.load(Ordering::Acquire) {
                    let Ok(request) = server.recv() else { break };
                    respond(&url, request);
                }
            }
        });
        Self {
            url,
            server,
            stopped,
            thread: Some(thread),
        }
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for HttpService {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}
