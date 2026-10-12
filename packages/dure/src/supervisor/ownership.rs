//! Serialized client ownership transitions, independent of receive loops.
//!
//! Ref: docs/supervisor.md, "Accept and steal".

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::outbox::Outbox;
use crate::pal::ids::ConnId;
use crate::pal::pseudoconsole::{Pseudoconsole, WindowSize};
use crate::pal::transport::Transport;
use crate::protocol::Message;
use crate::supervisor::shared::{Client, Shared, preamble_messages};

// These transitions also release blocked live relays. Removing acknowledgements,
// displacement, disconnects or stopping strands the concurrent scenarios; a
// finite model cannot make those other tests terminate. Keep these coordination
// operations outside mutation discovery while exercising them through both
// generated sequences and controlled interleavings.
// Ref: docs/testing.md, "Mutation testing coverage and skipping mutations".

/// Closes attach admission and removes the final owner as one transaction.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn stop_clients<T: Transport, C>(shared: &Shared<T, C>) -> Option<Client<T>> {
    let _attach = shared.attach();
    shared.stopping.store(true, Ordering::SeqCst);
    shared.client().take()
}

/// Installs a client and applies its geometry as one attach transaction.
///
/// A panic aborts this connection, not the session's ownership locks. Once
/// installed, its slot is removed by the receive loop's common cleanup.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn attach_client<T, C>(
    shared: &Shared<T, C>,
    conn: ConnId,
    size: WindowSize,
) -> Option<u64>
where
    T: Transport + Clone,
    C: Pseudoconsole,
{
    let attach = shared.attach();
    let mut created = None;
    // The guard outlives this catch, so a PAL or transport-clone panic cannot
    // poison it. A completed transfer stays valid even if its resize panics;
    // caller cleanup then removes that owner rather than restoring a displaced one.
    let result = catch_unwind(AssertUnwindSafe(|| {
        if shared.stopping.load(Ordering::SeqCst) {
            return None;
        }
        let outbox = Outbox::start(shared.transport.clone(), conn);
        created = Some(Arc::clone(&outbox));
        let (previous, generation) = {
            let mut slot = shared.client();
            // Only queue bookkeeping runs under the slot. The opening hold is
            // capped at the outbox limit, so this cannot trigger the overflow
            // path's transport callback.
            outbox.send(Message::Attached {
                session_id: shared.session_id,
            });
            if let Some(held) = shared.take_preamble() {
                for message in preamble_messages(&held) {
                    outbox.send(message);
                }
            }
            let previous = slot.replace(Client {
                conn,
                outbox: Arc::clone(&outbox),
            });
            (previous, shared.next_attached_generation())
        };
        shared.note_claimed();
        if let Some(old) = previous {
            // Preserve the displacement notice even when its reader is slow.
            // Ref: docs/supervisor.md, "Displacement".
            old.outbox.send(Message::Displaced);
            old.outbox.finish();
        }
        // The redraw belongs to the newly installed client. Store publication
        // stays outside this transaction so durable I/O cannot hold up a steal.
        _ = shared.pty_host.resize(shared.pty, size);
        Some(generation)
    }));
    drop(attach);
    match result {
        Ok(Some(generation)) => Some(generation),
        Ok(None) => {
            shared.transport.disconnect(conn);
            None
        }
        Err(panic) => {
            if let Some(outbox) = created {
                outbox.abandon();
            } else {
                shared.transport.disconnect(conn);
            }
            resume_unwind(panic)
        }
    }
}

/// Applies live input only while the sender still owns the console.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn apply_client_message<T, C>(
    shared: &Shared<T, C>,
    conn: ConnId,
    message: Message,
) -> bool
where
    T: Transport,
    C: Pseudoconsole,
{
    let slot = shared.client();
    if slot.as_ref().map(|client| client.conn) != Some(conn) {
        return false;
    }
    // Input and resize cannot change the slot. Catching inside its guard keeps
    // displacement serialized without turning a PAL panic into poisoned state.
    let result = catch_unwind(AssertUnwindSafe(|| match message {
        Message::Input(data) => {
            _ = shared.pty_host.write_input(shared.pty, &data);
            true
        }
        Message::Resize { size } => {
            _ = shared.pty_host.resize(shared.pty, size);
            true
        }
        _ => false,
    }));
    drop(slot);
    match result {
        Ok(accepted) => accepted,
        Err(panic) => resume_unwind(panic),
    }
}

/// Removes a departing owner without disturbing a client that replaced it.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn detach_client<T, C>(shared: &Shared<T, C>, conn: ConnId) -> Option<u64>
where
    T: Transport + Clone,
{
    let (departing, generation) = {
        let mut slot = shared.client();
        if slot.as_ref().map(|client| client.conn) != Some(conn) {
            return None;
        }
        (slot.take(), shared.next_attached_generation())
    };
    if let Some(departing) = departing {
        departing.outbox.abandon();
    }
    Some(generation)
}
