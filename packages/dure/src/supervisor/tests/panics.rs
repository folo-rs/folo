//! Panicking PAL callbacks must not poison session ownership or strand its owner.

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use testing::with_watchdog;

use super::*;
use crate::pal::ids::PtyId;
use crate::supervisor::ownership::{attach_client, detach_client};

/// Injects panics before calling the in-memory host, outside its own locks.
#[derive(Clone, Debug, Default)]
struct PanicPty {
    inner: MemoryPseudoconsole,
    fail_input: Arc<AtomicBool>,
    fail_resize: Arc<AtomicBool>,
}

impl Pseudoconsole for PanicPty {
    fn create(&self, size: WindowSize) -> Result<PtyId, PalError> {
        self.inner.create(size)
    }
    fn resize(&self, pty: PtyId, size: WindowSize) -> Result<(), PalError> {
        assert!(
            !self.fail_resize.swap(false, Ordering::SeqCst),
            "resize panic"
        );
        self.inner.resize(pty, size)
    }
    fn write_input(&self, pty: PtyId, data: &[u8]) -> Result<(), PalError> {
        assert!(
            !self.fail_input.swap(false, Ordering::SeqCst),
            "input panic"
        );
        self.inner.write_input(pty, data)
    }
    fn read_output(&self, pty: PtyId) -> Result<Option<Vec<u8>>, PalError> {
        self.inner.read_output(pty)
    }
    fn finish(&self, pty: PtyId) {
        self.inner.finish(pty);
    }
    fn close(&self, pty: PtyId) {
        self.inner.close(pty);
    }
}

#[test]
fn panicking_client_operations_release_the_owner_and_leave_locks_usable() {
    for initial_resize in [false, true] {
        for live_resize in [false, true] {
            let transport = MemoryTransport::new();
            let pty = PanicPty::default();
            let shared = shared_session(&transport, &pty);
            let (server, peer) = connected_pair(&transport, "panic");
            transport.send(peer, &ORDINARY_ATTACH).unwrap();
            let message = if live_resize {
                Message::Resize {
                    size: DEFAULT_PTY_SIZE,
                }
            } else {
                Message::Input(b"x".to_vec())
            };
            transport.send(peer, &message).unwrap();
            // A bounded script still ends if a mutation removes the callback.
            transport.send(peer, &Message::Displaced).unwrap();
            if initial_resize {
                pty.fail_resize.store(true, Ordering::SeqCst);
            }
            let set_attached = |_generation, attached| {
                if attached && !initial_resize {
                    if live_resize {
                        pty.fail_resize.store(true, Ordering::SeqCst);
                    } else {
                        pty.fail_input.store(true, Ordering::SeqCst);
                    }
                }
            };
            testing::assert_panics(AssertUnwindSafe(|| {
                client_loop(&shared, server, &set_attached);
            }));
            assert!(client_conn(&shared).is_none());
            drop(shared.attach());
            let (next, _) = connected_pair(&transport, "next");
            assert!(attach_client(&shared, next, DEFAULT_PTY_SIZE).is_some());
            assert!(detach_client(&shared, next).is_some());
        }
    }
}

/// Injects clone and initial-receive panics without changing the underlying pipes.
#[derive(Debug, Default)]
struct PanicTransport {
    inner: MemoryTransport,
    fail_clone: Arc<AtomicBool>,
    fail_attach_receive: Arc<AtomicBool>,
}

impl Clone for PanicTransport {
    fn clone(&self) -> Self {
        assert!(
            !self.fail_clone.swap(false, Ordering::SeqCst),
            "transport clone panic"
        );
        Self {
            inner: self.inner.clone(),
            fail_clone: Arc::clone(&self.fail_clone),
            fail_attach_receive: Arc::clone(&self.fail_attach_receive),
        }
    }
}

impl Transport for PanicTransport {
    fn listen(&self, name: &str) -> Result<ListenerId, PalError> {
        self.inner.listen(name)
    }
    fn accept(&self, listener: ListenerId) -> Result<ConnId, PalError> {
        self.inner.accept(listener)
    }
    fn accept_timeout(&self, listener: ListenerId, timeout: Duration) -> Result<ConnId, PalError> {
        self.inner.accept_timeout(listener, timeout)
    }
    fn connect(&self, name: &str, timeout: Duration) -> Result<ConnId, PalError> {
        self.inner.connect(name, timeout)
    }
    fn send(&self, conn: ConnId, message: &Message) -> Result<(), PalError> {
        self.inner.send(conn, message)
    }
    fn recv(&self, conn: ConnId) -> Result<Message, PalError> {
        self.inner.recv(conn)
    }
    fn recv_timeout(&self, conn: ConnId, timeout: Duration) -> Result<Message, PalError> {
        assert!(
            !self.fail_attach_receive.swap(false, Ordering::SeqCst),
            "initial receive panic"
        );
        self.inner.recv_timeout(conn, timeout)
    }
    fn disconnect(&self, conn: ConnId) {
        self.inner.disconnect(conn);
    }
    fn close_listener(&self, listener: ListenerId) {
        self.inner.close_listener(listener);
    }
    fn pipe_name(&self, nonce: &str) -> String {
        self.inner.pipe_name(nonce)
    }
}

#[test]
fn a_transport_clone_panic_releases_the_attach_lock_and_connection() {
    let transport = PanicTransport::default();
    let pty = MemoryPseudoconsole::new();
    let shared = shared_session(&transport, &pty);
    let (server, peer) = connected_pair(&transport.inner, "clone-panic");
    transport.fail_clone.store(true, Ordering::SeqCst);
    testing::assert_panics(AssertUnwindSafe(|| {
        attach_client(&shared, server, DEFAULT_PTY_SIZE)
    }));
    drop(shared.attach());
    assert!(client_conn(&shared).is_none());
    assert_eq!(
        transport
            .recv_timeout(peer, Duration::ZERO)
            .unwrap_err()
            .kind(),
        PalErrorKind::Disconnected
    );
    let (next, _) = connected_pair(&transport.inner, "next");
    assert!(attach_client(&shared, next, DEFAULT_PTY_SIZE).is_some());
    assert!(detach_client(&shared, next).is_some());
}

#[test]
fn a_panic_receiving_attach_disconnects_only_the_unattached_connection() {
    with_watchdog(|| {
        let transport = PanicTransport::default();
        let pty = MemoryPseudoconsole::new();
        let shared = shared_session(&transport, &pty);
        let (owner, owner_peer) = connected_pair(&transport.inner, "owner");
        assert!(attach_client(&shared, owner, DEFAULT_PTY_SIZE).is_some());
        let outbox = Arc::clone(&shared.client().as_ref().unwrap().outbox);
        let (server, peer) = connected_pair(&transport.inner, "receive-panic");

        transport.fail_attach_receive.store(true, Ordering::SeqCst);
        testing::assert_panics(AssertUnwindSafe(|| {
            client_loop(&shared, server, &|_, _| {
                panic!("an unattached client cannot change the attached flag");
            });
        }));

        // Sending observes whether the peer is closed without waiting for a reply.
        let peer_send = transport.send(peer, &Message::Input(b"x".to_vec()));
        let retained_owner = client_conn(&shared);
        let owner_connection = transport.send(owner_peer, &Message::Input(b"x".to_vec()));
        let detached_owner = detach_client(&shared, owner);
        outbox.wait_for_writer();

        assert_eq!(peer_send.unwrap_err().kind(), PalErrorKind::NotFound);
        assert_eq!(retained_owner, Some(owner));
        owner_connection.unwrap();
        assert!(detached_owner.is_some());
        drop(shared.attach());
    });
}
