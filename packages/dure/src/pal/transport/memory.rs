//! In-memory transport for unit tests.
//!
//! Accept remains possible while another connection's `recv` is blocked, which
//! is the steal-under-load contract without using the operating system.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::{ConnId, ListenerId};
use crate::pal::transport::Transport;
use crate::protocol::Message;

struct ConnState {
    incoming: VecDeque<Message>,
    peer: ConnId,
    closed: bool,
    /// Sends on this connection block, standing in for a peer that has stopped
    /// draining its pipe. Only `disconnect` releases them, as `CancelIoEx` does.
    stalled: bool,
}

struct ListenerState {
    pending: VecDeque<ConnId>,
}

struct Inner {
    next_id: AtomicU64,
    listeners: Mutex<HashMap<String, ListenerId>>,
    listener_state: Mutex<HashMap<ListenerId, ListenerState>>,
    conns: Mutex<HashMap<ConnId, ConnState>>,
    /// Successful startup commits, for client-side protocol assertions.
    startup_commits: AtomicUsize,
    /// Makes the next send fail so callers can exercise transport-failure handling.
    fail_next_send: AtomicBool,
    /// Guards the pending-connection predicate under `listener_state`. A
    /// `Condvar` may only ever be paired with one mutex, so the connection side
    /// has its own below.
    listener_cond: Condvar,
    /// Guards the queued-message and stalled predicates under `conns`.
    conn_cond: Condvar,
}

/// Thread-safe in-memory named-pipe stand-in.
#[derive(Clone)]
pub(crate) struct MemoryTransport {
    inner: Arc<Inner>,
}

impl fmt::Debug for MemoryTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryTransport").finish()
    }
}

impl MemoryTransport {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                next_id: AtomicU64::new(1),
                listeners: Mutex::new(HashMap::new()),
                listener_state: Mutex::new(HashMap::new()),
                conns: Mutex::new(HashMap::new()),
                startup_commits: AtomicUsize::new(0),
                fail_next_send: AtomicBool::new(false),
                listener_cond: Condvar::new(),
                conn_cond: Condvar::new(),
            }),
        }
    }

    fn alloc_id(&self) -> u64 {
        self.inner.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Make sends on `conn` block until it is disconnected or resumed.
    pub(crate) fn stall(&self, conn: ConnId) {
        let mut conns = self.inner.conns.lock().expect("conn map lock");
        if let Some(state) = conns.get_mut(&conn) {
            state.stalled = true;
        }
        self.inner.conn_cond.notify_all();
    }

    /// Let sends on `conn` proceed again, releasing whoever is blocked on it.
    pub(crate) fn resume(&self, conn: ConnId) {
        let mut conns = self.inner.conns.lock().expect("conn map lock");
        if let Some(state) = conns.get_mut(&conn) {
            state.stalled = false;
        }
        self.inner.conn_cond.notify_all();
    }

    pub(crate) fn startup_commit_count(&self) -> usize {
        self.inner.startup_commits.load(Ordering::SeqCst)
    }

    /// Make the next send fail without delivering its message.
    pub(crate) fn fail_next_send(&self) {
        self.inner.fail_next_send.store(true, Ordering::SeqCst);
    }

    fn accept_inner(
        &self,
        listener: ListenerId,
        timeout: Option<Duration>,
    ) -> Result<ConnId, PalError> {
        let started = Instant::now();
        let mut state = self
            .inner
            .listener_state
            .lock()
            .expect("listener state lock");
        loop {
            if let Some(listener_state) = state.get_mut(&listener)
                && let Some(conn) = listener_state.pending.pop_front()
            {
                return Ok(conn);
            }
            if !state.contains_key(&listener) {
                return Err(PalError::new(PalErrorKind::NotFound));
            }
            state = if let Some(timeout) = timeout {
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(PalError::new(PalErrorKind::Timeout));
                }
                self.inner
                    .listener_cond
                    .wait_timeout(state, remaining)
                    .expect("listener condvar")
                    .0
            } else {
                self.inner
                    .listener_cond
                    .wait(state)
                    .expect("listener condvar")
            };
        }
    }

    fn recv_inner(&self, conn: ConnId, timeout: Option<Duration>) -> Result<Message, PalError> {
        let started = Instant::now();
        let mut conns = self.inner.conns.lock().expect("conn map lock");
        loop {
            let Some(state) = conns.get_mut(&conn) else {
                return Err(PalError::new(PalErrorKind::NotFound));
            };
            if let Some(message) = state.incoming.pop_front() {
                return Ok(message);
            }
            if state.closed {
                return Err(PalError::new(PalErrorKind::Disconnected));
            }
            conns = if let Some(timeout) = timeout {
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(PalError::new(PalErrorKind::Timeout));
                }
                self.inner
                    .conn_cond
                    .wait_timeout(conns, remaining)
                    .expect("conn condvar")
                    .0
            } else {
                self.inner.conn_cond.wait(conns).expect("conn condvar")
            };
        }
    }
}

impl Default for MemoryTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for MemoryTransport {
    fn listen(&self, name: &str) -> Result<ListenerId, PalError> {
        let id = ListenerId(self.alloc_id());
        let mut listeners = self.inner.listeners.lock().expect("listener map lock");
        if listeners.contains_key(name) {
            return Err(PalError::new(PalErrorKind::Other));
        }
        listeners.insert(name.to_string(), id);
        self.inner
            .listener_state
            .lock()
            .expect("listener state lock")
            .insert(
                id,
                ListenerState {
                    pending: VecDeque::new(),
                },
            );
        Ok(id)
    }

    fn accept(&self, listener: ListenerId) -> Result<ConnId, PalError> {
        self.accept_inner(listener, None)
    }

    fn accept_timeout(&self, listener: ListenerId, timeout: Duration) -> Result<ConnId, PalError> {
        self.accept_inner(listener, Some(timeout))
    }

    fn connect(&self, name: &str, _timeout: Duration) -> Result<ConnId, PalError> {
        let listeners = self.inner.listeners.lock().expect("listener map lock");
        let Some(&listener) = listeners.get(name) else {
            return Err(PalError::new(PalErrorKind::Timeout));
        };
        drop(listeners);

        let server = ConnId(self.alloc_id());
        let client = ConnId(self.alloc_id());
        {
            let mut conns = self.inner.conns.lock().expect("conn map lock");
            conns.insert(
                server,
                ConnState {
                    incoming: VecDeque::new(),
                    peer: client,
                    closed: false,
                    stalled: false,
                },
            );
            conns.insert(
                client,
                ConnState {
                    incoming: VecDeque::new(),
                    peer: server,
                    closed: false,
                    stalled: false,
                },
            );
        }

        let mut state = self
            .inner
            .listener_state
            .lock()
            .expect("listener state lock");
        let Some(listener_state) = state.get_mut(&listener) else {
            drop(state);
            self.disconnect(server);
            return Err(PalError::new(PalErrorKind::NotFound));
        };
        listener_state.pending.push_back(server);
        self.inner.listener_cond.notify_all();
        Ok(client)
    }

    fn send(&self, conn: ConnId, message: &Message) -> Result<(), PalError> {
        if self.inner.fail_next_send.swap(false, Ordering::SeqCst) {
            return Err(PalError::new(PalErrorKind::Other));
        }
        let mut conns = self.inner.conns.lock().expect("conn map lock");
        let peer = loop {
            let Some(state) = conns.get(&conn) else {
                return Err(PalError::new(PalErrorKind::NotFound));
            };
            if state.closed {
                return Err(PalError::new(PalErrorKind::NotFound));
            }
            if !state.stalled {
                break state.peer;
            }
            conns = self.inner.conn_cond.wait(conns).expect("conn condvar");
        };
        let Some(state) = conns.get_mut(&peer) else {
            return Err(PalError::new(PalErrorKind::NotFound));
        };
        if state.closed {
            return Err(PalError::new(PalErrorKind::NotFound));
        }
        state.incoming.push_back(message.clone());
        if matches!(message, Message::StartupCommit) {
            self.inner.startup_commits.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.conn_cond.notify_all();
        Ok(())
    }

    fn recv(&self, conn: ConnId) -> Result<Message, PalError> {
        self.recv_inner(conn, None)
    }

    fn recv_timeout(&self, conn: ConnId, timeout: Duration) -> Result<Message, PalError> {
        self.recv_inner(conn, Some(timeout))
    }

    fn disconnect(&self, conn: ConnId) {
        let mut conns = self.inner.conns.lock().expect("conn map lock");
        let peer = conns.get(&conn).map(|state| state.peer);
        if let Some(state) = conns.get_mut(&conn) {
            state.closed = true;
        }
        if let Some(peer) = peer
            && let Some(state) = conns.get_mut(&peer)
        {
            state.closed = true;
        }
        self.inner.conn_cond.notify_all();
    }

    fn close_listener(&self, listener: ListenerId) {
        self.inner
            .listeners
            .lock()
            .expect("listener map lock")
            .retain(|_, id| *id != listener);
        let pending = self
            .inner
            .listener_state
            .lock()
            .expect("listener state lock")
            .remove(&listener)
            .map_or_else(VecDeque::new, |state| state.pending);
        self.inner.listener_cond.notify_all();
        for conn in pending {
            self.disconnect(conn);
        }
    }

    fn pipe_name(&self, nonce: &str) -> String {
        format!("memory:{nonce}")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn accept_timeout_returns_timeout_when_no_connection_is_pending() {
        let transport = MemoryTransport::new();
        let listener = transport.listen("session").unwrap();

        let error = transport
            .accept_timeout(listener, Duration::ZERO)
            .unwrap_err();

        assert_eq!(error.kind(), PalErrorKind::Timeout);
    }

    #[test]
    fn recv_timeout_returns_timeout_when_no_message_is_pending() {
        let transport = MemoryTransport::new();
        let listener = transport.listen("session").unwrap();
        let client = transport.connect("session", Duration::ZERO).unwrap();
        _ = transport.accept(listener).unwrap();

        let error = transport.recv_timeout(client, Duration::ZERO).unwrap_err();

        assert_eq!(error.kind(), PalErrorKind::Timeout);
    }
}
