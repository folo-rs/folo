//! In-memory transport for unit tests.
//!
//! Accept remains possible while another connection's `recv` is blocked, which
//! is the steal-under-load contract without using the operating system.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

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
    cond: Condvar,
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
                cond: Condvar::new(),
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
        self.inner.cond.notify_all();
    }

    /// Let sends on `conn` proceed again, releasing whoever is blocked on it.
    pub(crate) fn resume(&self, conn: ConnId) {
        let mut conns = self.inner.conns.lock().expect("conn map lock");
        if let Some(state) = conns.get_mut(&conn) {
            state.stalled = false;
        }
        self.inner.cond.notify_all();
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
            state = self.inner.cond.wait(state).expect("listener condvar");
        }
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
            return Err(PalError::new(PalErrorKind::Timeout));
        };
        listener_state.pending.push_back(server);
        self.inner.cond.notify_all();
        Ok(client)
    }

    fn send(&self, conn: ConnId, message: &Message) -> Result<(), PalError> {
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
            conns = self.inner.cond.wait(conns).expect("conn condvar");
        };
        let Some(state) = conns.get_mut(&peer) else {
            return Err(PalError::new(PalErrorKind::NotFound));
        };
        if state.closed {
            return Err(PalError::new(PalErrorKind::NotFound));
        }
        state.incoming.push_back(message.clone());
        self.inner.cond.notify_all();
        Ok(())
    }

    fn recv(&self, conn: ConnId) -> Result<Message, PalError> {
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
            conns = self.inner.cond.wait(conns).expect("conn condvar");
        }
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
        self.inner.cond.notify_all();
    }

    fn close_listener(&self, listener: ListenerId) {
        self.inner
            .listeners
            .lock()
            .expect("listener map lock")
            .retain(|_, id| *id != listener);
        self.inner
            .listener_state
            .lock()
            .expect("listener state lock")
            .remove(&listener);
        self.inner.cond.notify_all();
    }

    fn pipe_name(&self, nonce: &str) -> String {
        format!("memory:{nonce}")
    }
}
