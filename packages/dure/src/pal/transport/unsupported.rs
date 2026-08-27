//! Transport PAL used on non-Windows builds.

use std::time::Duration;

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::{ConnId, ListenerId};
use crate::pal::transport::Transport;
use crate::protocol::Message;

/// Stub transport. The platform gate refuses to run before this is used.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetTransport;

fn unsupported<T>() -> Result<T, PalError> {
    Err(PalError::new(PalErrorKind::Other))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl Transport for BuildTargetTransport {
    fn listen(&self, _name: &str) -> Result<ListenerId, PalError> {
        unsupported()
    }

    fn accept(&self, _listener: ListenerId) -> Result<ConnId, PalError> {
        unsupported()
    }

    fn connect(&self, _name: &str, _timeout: Duration) -> Result<ConnId, PalError> {
        unsupported()
    }

    fn send(&self, _conn: ConnId, _message: &Message) -> Result<(), PalError> {
        unsupported()
    }

    fn recv(&self, _conn: ConnId) -> Result<Message, PalError> {
        unsupported()
    }

    fn disconnect(&self, _conn: ConnId) {}

    fn close_listener(&self, _listener: ListenerId) {}

    fn pipe_name(&self, nonce: &str) -> String {
        format!(r"\\.\pipe\dure-{nonce}")
    }
}
