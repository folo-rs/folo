//! Carrying the console between the app and whichever client owns it.
//!
//! Ref: docs/supervisor.md, "Relay".

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use ohno::AppError;

use crate::constants::CONNECT_TIMEOUT;
use crate::pal::error::PalError;
use crate::pal::ids::{ConnId, JobId, ListenerId};
use crate::pal::processes::Processes;
use crate::pal::pseudoconsole::Pseudoconsole;
use crate::pal::session_store::SessionStore;
use crate::pal::transport::Transport;
use crate::protocol::Message;
use crate::supervisor::ownership::{
    apply_client_message, attach_client, detach_client, stop_clients,
};
use crate::supervisor::record_writer::RecordWriter;
use crate::supervisor::shared::{FirstAttach, Shared};
use crate::supervisor::startup::Initialized;
use crate::{PalFailedError, StoreError};

// Blocking serve loop. A mutation that returns before the accept and PTY threads
// are wired up leaves the test's client waiting forever, and watchdogs are
// disabled under cargo-mutants.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn serve<P, S, T, C>(
    processes: &P,
    store: &S,
    transport: &T,
    pty_host: &C,
    initialized: &Initialized,
    startup: ConnId,
) -> Result<i32, AppError>
where
    P: Processes,
    S: SessionStore + Clone,
    T: Transport + Clone + Send + Sync + 'static,
    C: Pseudoconsole + Clone + Send + Sync + 'static,
{
    let &Initialized {
        session_id,
        identity,
        listener,
        pty,
        job,
        app,
        ..
    } = initialized;

    let attached_generation = Arc::new(AtomicU64::default());
    let record_writer = RecordWriter::start(store, session_id, Arc::clone(&attached_generation));
    let shared = Arc::new(Shared {
        transport: transport.clone(),
        pty_host: pty_host.clone(),
        pty,
        session_id,
        client: Mutex::new(None),
        attach: Mutex::new(()),
        attached_generation: Arc::clone(&attached_generation),
        preamble: Mutex::new(Some(Vec::new())),
        first_attach: Mutex::new(FirstAttach::default()),
        first_attach_changed: Condvar::new(),
        stopping: AtomicBool::new(false),
    });

    // The startup channel doubles as the initiator's liveness signal: it stays
    // open for as long as `dure run` intends to attach.
    thread::spawn({
        let shared = Arc::clone(&shared);
        let transport = transport.clone();
        move || {
            _ = transport.recv(startup);
            transport.disconnect(startup);
            shared.note_initiator_gone();
        }
    });

    let store_flag = record_writer.set_attached();
    thread::spawn({
        let shared = Arc::clone(&shared);
        let transport = transport.clone();
        move || accept_loop(&shared, &transport, listener, store_flag)
    });

    let output_failed = AtomicBool::new(false);
    let job_closed = AtomicBool::new(false);

    thread::scope(|scope| {
        let pty_pump = scope.spawn({
            let shared = Arc::clone(&shared);
            let output_failed = &output_failed;
            let job_closed = &job_closed;
            move || {
                let result = pty_output_loop(&shared);
                if result.is_err() {
                    // A stopped pump can no longer drain an app that keeps
                    // writing. End the job so wait_app cannot block behind a
                    // full pseudoconsole pipe.
                    output_failed.store(true, Ordering::SeqCst);
                    close_job_once(processes, job, job_closed);
                }
                result
            }
        });

        // Everything below this point is teardown the session owes the host
        // whether or not the wait succeeded: the listener, the job holding the
        // app and its descendants, the pseudoconsole, and the published record
        // all outlive this function otherwise. Failures are reported only once
        // that is done.
        let waited = processes.wait_app(app);

        // An app can outlive neither its output nor its exit status: both are
        // only deliverable while the session is still up, so a session nobody
        // has attached to yet stays up until its initiator arrives or gives up.
        // A failed wait or pump has no complete outcome to deliver.
        if waited.is_ok() && !output_failed.load(Ordering::SeqCst) {
            shared.await_first_attach();
        }

        transport.close_listener(listener);
        // End descendants before finishing their console; the output pump
        // remains active while the console flushes its final output.
        close_job_once(processes, job, &job_closed);
        // The app has exited, so ending the pseudoconsole flushes what it still
        // holds and lets the output loop finish those bytes before clean EOF.
        // Joining the pump before announcing the exit orders the app's final
        // output ahead of `AppExited` instead of racing it.
        pty_host.finish(pty);
        let pumped = pty_pump
            .join()
            .expect("the output pump contains no panic-capable callbacks");
        pty_host.close(pty);

        // Both under the attach lock, which `client_loop` also takes for the
        // whole attach transaction. An attach therefore either completes
        // before the slot is claimed here and receives the outcome, or observes
        // the stop and is refused.
        let client = stop_clients(&shared);
        if let Some(client) = &client {
            if pumped.is_ok()
                && let Ok(status) = &waited
            {
                // Attach treats a disconnect without `AppExited` as a relay
                // failure, so a complete status is queued behind all output.
                client.outbox.send(Message::AppExited { status: *status });
            }
            client.outbox.finish();
        }

        // Nothing can publish the record after this, so the delete below is
        // final — including over a session id that is later reused.
        record_writer.finish();
        // Ids are reused, so an unconditional delete could reap whichever
        // session claimed this id after this supervisor published.
        let deleted = store.delete_owned_by(session_id, &identity);

        if let Some(client) = client {
            // Delivery finishes before cleanup errors are reported. Record
            // deletion is independent of the outcome already owed to a client.
            client.outbox.wait_for_writer();
        }

        // A wait that failed is the cause and a record that outlives it is only
        // a consequence, so the wait failure is the one worth reporting.
        let status = waited.map_err(PalFailedError::caused_by)?;
        pumped.map_err(PalFailedError::caused_by)?;
        deleted.map_err(StoreError::caused_by)?;
        Ok(status)
    })
}

/// Closes the app-lifetime job once across normal teardown and pump failure.
// Replacing this with a no-op strands wait_app after an output failure. Proving
// non-termination would require the real-time timeout mutation tests forbid.
#[cfg_attr(test, mutants::skip)]
fn close_job_once<P: Processes>(processes: &P, job: JobId, closed: &AtomicBool) {
    if closed
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        processes.close_job(job);
    }
}

// Blocking accept. A mutation that drops the stop check or the accept error
// path hangs unit tests because watchdogs are disabled under cargo-mutants.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn accept_loop<T, C>(
    shared: &Arc<Shared<T, C>>,
    transport: &T,
    listener: ListenerId,
    set_attached: impl Fn(u64, bool) + Clone + Send + 'static,
) where
    T: Transport + Clone,
    C: Pseudoconsole,
{
    while !shared.stopping.load(Ordering::SeqCst) {
        let Ok(conn) = transport.accept(listener) else {
            break;
        };
        // Steal happens after a valid Attach in `client_loop`. Installing the
        // slot on accept would let a stalled connection displace a live client
        // and inject Output before Attached.
        thread::spawn({
            let shared = Arc::clone(shared);
            let set_attached = set_attached.clone();
            move || client_loop(&shared, conn, &set_attached)
        });
    }
}

// Blocking recv. A mutation that drops the disconnect path hangs unit tests
// because watchdogs are disabled under cargo-mutants.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn client_loop<T, C>(
    shared: &Shared<T, C>,
    conn: ConnId,
    set_attached: &impl Fn(u64, bool),
) where
    T: Transport + Clone,
    C: Pseudoconsole,
{
    // A panicking PAL ends this relay but still releases any ownership it
    // acquired. The ownership helpers release their guards before propagating.
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Ok(Message::Attach { size }) = shared.transport.recv_timeout(conn, CONNECT_TIMEOUT)
        else {
            shared.transport.disconnect(conn);
            return;
        };
        let Some(generation) = attach_client(shared, conn, size) else {
            return;
        };
        set_attached(generation, true);
        while let Ok(message) = shared.transport.recv(conn) {
            if !apply_client_message(shared, conn, message) {
                break;
            }
        }
    }));
    if let Some(generation) = detach_client(shared, conn) {
        set_attached(generation, false);
    }
    if let Err(panic) = result {
        resume_unwind(panic);
    }
}

// Blocking read of pty output. A mutation that drops the stop check hangs
// unit tests because watchdogs are disabled under cargo-mutants.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn pty_output_loop<T, C>(shared: &Shared<T, C>) -> Result<(), PalError>
where
    T: Transport + Clone,
    C: Pseudoconsole,
{
    while !shared.stopping.load(Ordering::SeqCst) {
        let Some(bytes) = shared.pty_host.read_output(shared.pty)? else {
            return Ok(());
        };
        route_output(shared, bytes);
    }
    Ok(())
}

/// Routes one output burst without blocking on its eventual pipe write.
// Removing delivery strands the concurrent relay tests on their expected output.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn route_output<T: Transport + Clone, C>(shared: &Shared<T, C>, bytes: Vec<u8>) {
    let client = {
        let slot = shared.client();
        match slot.as_ref() {
            Some(client) => Some(client.clone()),
            None => {
                shared.hold_for_first_client(&bytes);
                None
            }
        }
    };
    if let Some(client) = client {
        client.outbox.send(Message::Output(bytes));
    }
}
