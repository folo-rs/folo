//! Supervisor role: own the app, accept clients, last-connect-wins steal.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use ohno::AppError;

use crate::constants::{CONNECT_TIMEOUT, DEFAULT_PTY_COLS, DEFAULT_PTY_ROWS};
use crate::outbox::Outbox;
use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::{AppId, ConnId, JobId, ListenerId, PtyId};
use crate::pal::processes::{AppSpawn, Processes};
use crate::pal::pseudoconsole::{Pseudoconsole, WindowSize};
use crate::pal::session_store::SessionStore;
use crate::pal::transport::Transport;
use crate::protocol::Message;
use crate::session_id::SessionId;
use crate::session_record::{ProcessIdentity, SessionRecord};
use crate::{BreakawayDeniedError, PalFailedError, StartupFailedError, StoreError};

/// Size used until the first client attaches.
///
/// VGA text-mode geometry (`DEFAULT_PTY_COLS` by `DEFAULT_PTY_ROWS`). The first
/// attach always resizes to the client's real size (design.md, "Attach, detach,
/// steal").
const DEFAULT_PTY_SIZE: WindowSize = WindowSize {
    cols: DEFAULT_PTY_COLS,
    rows: DEFAULT_PTY_ROWS,
};

/// Resources that must be torn down if initialization fails.
struct InitGuard<'a, P: Processes, S: SessionStore, C: Pseudoconsole> {
    processes: &'a P,
    store: &'a S,
    pty_host: &'a C,
    job: Option<JobId>,
    pty: Option<PtyId>,
    session: Option<(SessionId, ProcessIdentity)>,
    committed: bool,
}

impl<P: Processes, S: SessionStore, C: Pseudoconsole> Drop for InitGuard<'_, P, S, C> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some((id, owner)) = self.session {
            _ = self.store.delete_owned_by(id, &owner);
        }
        // Descendants of the app stay attached to the pseudoconsole until the
        // job that owns their lifetime is closed, and closing a pseudoconsole
        // waits for its attached clients.
        if let Some(job) = self.job {
            self.processes.close_job(job);
        }
        if let Some(pty) = self.pty {
            self.pty_host.close(pty);
        }
    }
}

/// Initialize the session, publish the record, then relay until the app exits.
// Blocking supervisor entry point. A mutation that returns before serving leaves
// the test's client waiting on a session that never appears, and watchdogs are
// disabled under cargo-mutants.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn run_supervisor<P, S, T, C>(
    processes: &P,
    store: &S,
    transport: &T,
    pty_host: &C,
    startup_pipe: &str,
    launch_directory: PathBuf,
    command: Vec<String>,
) -> Result<i32, AppError>
where
    P: Processes,
    S: SessionStore + Clone,
    T: Transport + Clone + Send + Sync + 'static,
    C: Pseudoconsole + Clone + Send + Sync + 'static,
{
    let startup = transport
        .connect(startup_pipe, CONNECT_TIMEOUT)
        .map_err(|_error| StartupFailedError::new())?;

    let mut guard = InitGuard {
        processes,
        store,
        pty_host,
        job: None,
        pty: None,
        session: None,
        committed: false,
    };

    let result = initialize(
        &mut guard,
        processes,
        store,
        transport,
        pty_host,
        launch_directory,
        command,
    );

    let initialized = match result {
        Ok(initialized) => initialized,
        Err(error) => {
            _ = transport.send(startup, &Message::StartupErr);
            transport.disconnect(startup);
            return Err(error);
        }
    };

    // The client may already have dropped the startup pipe. The session is
    // live either way; resume attaches independently of this acknowledgement.
    _ = transport.send(
        startup,
        &Message::StartupOk {
            session_id: initialized.session_id,
            // Only this process can see the job it landed in, and the client is
            // the one with a console to report it on.
            // Ref: docs/implementation.md, "Job breakaway".
            durability: processes.durability(),
        },
    );
    guard.committed = true;

    // The startup connection stays open past the acknowledgement: `serve` reads
    // it as the initiator's liveness signal and disconnects it.
    let status = serve(processes, store, transport, pty_host, &initialized, startup)?;
    Ok(status)
}

#[derive(Clone, Copy)]
struct Initialized {
    session_id: SessionId,
    identity: ProcessIdentity,
    listener: ListenerId,
    pty: PtyId,
    job: JobId,
    app: AppId,
}

fn initialize<P, S, T, C>(
    guard: &mut InitGuard<'_, P, S, C>,
    processes: &P,
    store: &S,
    transport: &T,
    pty_host: &C,
    launch_directory: PathBuf,
    command: Vec<String>,
) -> Result<Initialized, AppError>
where
    P: Processes,
    S: SessionStore,
    T: Transport,
    C: Pseudoconsole,
{
    let job = processes
        .create_lifetime_job()
        .map_err(|error| map_startup(&error))?;
    guard.job = Some(job);

    let pty = pty_host
        .create(DEFAULT_PTY_SIZE)
        .map_err(|error| map_startup(&error))?;
    guard.pty = Some(pty);

    let app = processes
        .spawn_app(&AppSpawn {
            command: command.clone(),
            launch_directory: launch_directory.clone(),
            pty,
            job,
        })
        .map_err(|error| map_startup(&error))?;

    let nonce = processes.random_nonce();
    let pipe_name = transport.pipe_name(&nonce);
    let listener = transport
        .listen(&pipe_name)
        .map_err(|error| map_startup(&error))?;

    let identity = processes
        .current_identity()
        .map_err(|error| map_startup(&error))?;
    let session_id = store
        .allocate_id(&identity)
        .map_err(|_error| StoreError::new())?;
    guard.session = Some((session_id, identity));

    let started_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        });
    let record = SessionRecord {
        id: session_id.get(),
        supervisor_pid: identity.pid,
        supervisor_creation_time: identity.creation_time,
        pipe_name,
        launch_directory,
        command,
        started_at_unix_ms,
        attached: false,
    };
    store.publish(&record).map_err(|_error| StoreError::new())?;

    Ok(Initialized {
        session_id,
        identity,
        listener,
        pty,
        job,
        app,
    })
}

fn map_startup(error: &PalError) -> AppError {
    match error.kind() {
        PalErrorKind::BreakawayDenied => BreakawayDeniedError::new().into(),
        _ => StartupFailedError::new().into(),
    }
}

struct Shared<T: Transport, C> {
    transport: T,
    pty_host: C,
    pty: PtyId,
    session_id: SessionId,
    /// Holding this across a pseudoconsole write keeps a displaced client from
    /// reaching the app between its ownership check and the write itself.
    /// Writes here land in the console host's input buffer, which the host
    /// drains independently of the app, so the hold is bounded. Attach also
    /// acknowledges under it, which orders `Attached` ahead of any `Output` on
    /// the same connection and closes the window in which output would be
    /// discarded for want of an installed client.
    client: Mutex<Option<Client<T>>>,
    /// Serializes an entire attach: acknowledgement, ownership transfer, and
    /// displacement of the previous client. Without it two attaches can
    /// acknowledge in one order and install in another, letting an older
    /// attach displace a newer one.
    attach: Mutex<()>,
    /// Gate that holds the session open until somebody has come for it.
    first_attach: Mutex<FirstAttach>,
    first_attach_changed: Condvar,
    stopping: AtomicBool,
}

/// The connection that currently owns the console, and its write side.
struct Client<T: Transport> {
    conn: ConnId,
    outbox: Arc<Outbox<T>>,
}

impl<T: Transport> Clone for Client<T> {
    fn clone(&self) -> Self {
        Self {
            conn: self.conn,
            outbox: Arc::clone(&self.outbox),
        }
    }
}

/// Whether the session has been claimed, and whether anyone is still coming.
///
/// An app that exits immediately would otherwise be torn down before `dure run`
/// finishes attaching, losing both its output and its exit status. The
/// supervisor therefore holds the session open after the app exits until either
/// the first client attaches or the process that started the session goes away.
/// Ref: docs/design.md, "Commands"; docs/implementation.md, "Process split".
#[derive(Debug, Default)]
struct FirstAttach {
    attached: bool,
    initiator_gone: bool,
}

impl<T: Transport, C> Shared<T, C> {
    fn first_attach(&self) -> MutexGuard<'_, FirstAttach> {
        self.first_attach
            .lock()
            .expect("first-attach flags are only set, never held across a panic")
    }

    /// Records that a client took the session.
    // A mutation that drops either flag or the wait leaves the gate closed, and
    // watchdogs are disabled under cargo-mutants, so the test hangs instead of
    // failing.
    #[cfg_attr(test, mutants::skip)]
    fn note_attached(&self) {
        self.first_attach().attached = true;
        self.first_attach_changed.notify_all();
    }

    /// Records that the process that started the session dropped its channel.
    #[cfg_attr(test, mutants::skip)]
    fn note_initiator_gone(&self) {
        self.first_attach().initiator_gone = true;
        self.first_attach_changed.notify_all();
    }

    /// Blocks until the session has been claimed or nobody is coming for it.
    #[cfg_attr(test, mutants::skip)]
    fn await_first_attach(&self) {
        let mut state = self.first_attach();
        while !state.attached && !state.initiator_gone {
            state = self
                .first_attach_changed
                .wait(state)
                .expect("first-attach flags are only set, never held across a panic");
        }
    }

    fn client(&self) -> MutexGuard<'_, Option<Client<T>>> {
        self.client
            .lock()
            .expect("client slot is only copied or replaced, never held across a panic")
    }
}

// Blocking serve loop. A mutation that returns before the accept and PTY threads
// are wired up leaves the test's client waiting forever, and watchdogs are
// disabled under cargo-mutants.
#[cfg_attr(test, mutants::skip)]
fn serve<P, S, T, C>(
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
    let Initialized {
        session_id,
        identity,
        listener,
        pty,
        job,
        app,
    } = *initialized;

    let record_live = Arc::new(Mutex::new(true));
    let shared = Arc::new(Shared {
        transport: transport.clone(),
        pty_host: pty_host.clone(),
        pty,
        session_id,
        client: Mutex::new(None),
        attach: Mutex::new(()),
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

    let store_flag = store_attached_flag(store, session_id, Arc::clone(&record_live));
    thread::spawn({
        let shared = Arc::clone(&shared);
        let transport = transport.clone();
        move || accept_loop(&shared, &transport, listener, store_flag)
    });

    let pty_pump = thread::spawn({
        let shared = Arc::clone(&shared);
        move || pty_output_loop(&shared)
    });

    let status = processes
        .wait_app(app)
        .map_err(|_error| PalFailedError::new())?;

    // An app can outlive neither its output nor its exit status: both are only
    // deliverable while the session is still up, so a session nobody has
    // attached to yet stays up until its initiator arrives or gives up.
    shared.await_first_attach();

    transport.close_listener(listener);
    // Descendants of the app stay attached to the pseudoconsole until this job
    // ends them, and closing a pseudoconsole waits for its attached clients.
    processes.close_job(job);
    // The app has exited, so closing the pseudoconsole flushes what it still
    // holds and ends the output loop with a read failure. Joining the pump
    // before announcing the exit is what orders the app's final output ahead
    // of `AppExited` instead of racing it.
    pty_host.close(pty);
    _ = pty_pump.join();

    // Both under the attach lock, which `client_loop` also takes for the whole
    // attach transaction. An attach therefore either completes before the slot
    // is claimed here and receives the exit status, or observes the stop and is
    // refused. Reading the slot outside the lock would let a client install
    // itself between the two and never learn that the app exited.
    let client = {
        let _attach = shared
            .attach
            .lock()
            .expect("the attach lock guards no data, so it is never poisoned by its guard");
        shared.stopping.store(true, Ordering::SeqCst);
        shared.client().take()
    };
    if let Some(client) = &client {
        // Attach treats a disconnect without `AppExited` as a relay failure
        // when the input thread has already stopped, so the status must be
        // queued behind the output rather than racing it.
        client.outbox.send(Message::AppExited { status });
        client.outbox.finish();
    }

    {
        // Client threads take this lock before publishing an attached-flag
        // update. Clearing it first prevents a late publish from recreating
        // the record after delete, including over a reused session id.
        let mut live = record_live
            .lock()
            .expect("record_live is only set false here, never held across a panic");
        *live = false;
        // Ids are reused, so an unconditional delete could reap whichever
        // session claimed this id after this supervisor published.
        store
            .delete_owned_by(session_id, &identity)
            .map_err(|_error| StoreError::new())?;
    }
    if let Some(client) = client {
        // The session already owns nothing, so waiting here for the exit status
        // to land costs a client that is still reading nothing and a client
        // that has stopped reading only this process outliving it.
        client.outbox.flush();
    }
    Ok(status)
}

fn store_attached_flag<S: SessionStore + Clone>(
    store: &S,
    id: SessionId,
    record_live: Arc<Mutex<bool>>,
) -> impl Fn(bool) + Clone + Send + 'static {
    let store = store.clone();
    move |attached: bool| {
        let live = record_live
            .lock()
            .expect("record_live is only set false at delete, never held across a panic");
        if !*live {
            return;
        }
        if let Ok(Some(mut record)) = store.read(id) {
            record.attached = attached;
            // A failed write leaves a stale attached flag. The flag is
            // advisory; liveness is the supervisor process.
            _ = store.publish(&record);
        }
    }
}

// Blocking accept. A mutation that drops the stop check or the accept error
// path hangs unit tests because watchdogs are disabled under cargo-mutants.
#[cfg_attr(test, mutants::skip)]
fn accept_loop<T, C>(
    shared: &Arc<Shared<T, C>>,
    transport: &T,
    listener: ListenerId,
    set_attached: impl Fn(bool) + Clone + Send + 'static,
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
fn client_loop<T, C>(shared: &Shared<T, C>, conn: ConnId, set_attached: &impl Fn(bool))
where
    T: Transport + Clone,
    C: Pseudoconsole,
{
    match shared.transport.recv(conn) {
        Ok(Message::Attach { cols, rows }) => {
            // One serialized attach transaction: acknowledge, take ownership,
            // and displace the previous client without another attach
            // interleaving between the acknowledgement and the transfer.
            let _attach = shared
                .attach
                .lock()
                .expect("the attach lock guards no data, so it is never poisoned by its guard");
            // The exit status is routed to whoever owns the slot at the moment
            // teardown claims it, under this same lock. An attach that installed
            // itself afterwards would own a session that has already given up
            // its record, job, and pseudoconsole, and would lose the supervisor
            // without ever being told the app exited. Refusing before
            // acknowledging is what makes `resume` report a session that is gone
            // instead of a relay that broke.
            if shared.stopping.load(Ordering::SeqCst) {
                shared.transport.disconnect(conn);
                return;
            }
            let outbox = Outbox::start(shared.transport.clone(), conn);
            let previous = {
                let mut slot = shared.client();
                // Acknowledging under the client slot keeps `Attached` ahead of
                // any `Output` on this connection, and installing in the same
                // critical section means output the app produces right after the
                // acknowledgement is not discarded for want of an installed
                // client. This one write is direct rather than queued because
                // the peer is blocked waiting for exactly this frame, and a
                // failure here is how a client that is already gone is detected.
                if shared
                    .transport
                    .send(
                        conn,
                        &Message::Attached {
                            session_id: shared.session_id,
                        },
                    )
                    .is_err()
                {
                    drop(slot);
                    outbox.abandon();
                    return;
                }
                slot.replace(Client {
                    conn,
                    outbox: Arc::clone(&outbox),
                })
            };
            if let Some(old) = previous {
                // Queued rather than written here, so a client that stopped
                // reading cannot hold up the steal that is replacing it. The
                // displaced client may already have disconnected; steal still
                // proceeds, because last-connect-wins does not depend on this
                // notice.
                old.outbox.send(Message::Displaced);
                old.outbox.finish();
            }
            // Applied only once this connection owns the client slot: the app
            // redraws in response to a size change, and that redraw belongs to
            // the client that asked for the size. Resize failure means the pty
            // is already gone; wait_app and read_output observe that and stop
            // the relay.
            _ = shared
                .pty_host
                .resize(shared.pty, WindowSize { cols, rows });
            set_attached(true);
            shared.note_attached();
        }
        _ => {
            shared.transport.disconnect(conn);
            return;
        }
    }

    while let Ok(message) = shared.transport.recv(conn) {
        // Ownership is checked and the message applied under one lock: a client
        // displaced while its receive was in flight must not reach the app
        // after the new client became the live console.
        let slot = shared.client();
        if slot.as_ref().map(|client| client.conn) != Some(conn) {
            break;
        }
        match message {
            Message::Input(data) => {
                // Input after the app has exited is dropped. wait_app publishes
                // AppExited to the live client.
                _ = shared.pty_host.write_input(shared.pty, &data);
            }
            Message::Resize { cols, rows } => {
                _ = shared
                    .pty_host
                    .resize(shared.pty, WindowSize { cols, rows });
            }
            _ => break,
        }
        drop(slot);
    }
    let departing = {
        let mut slot = shared.client();
        if slot.as_ref().map(|client| client.conn) == Some(conn) {
            let departing = slot.take();
            // Holding the slot so a replacement cannot publish attached=true
            // before this disconnect publishes attached=false.
            set_attached(false);
            departing
        } else {
            None
        }
    };
    if let Some(departing) = departing {
        // The peer is gone or misbehaving, so nothing still queued for it is
        // worth waiting on.
        departing.outbox.abandon();
    }
}

// Blocking read of pty output. A mutation that drops the stop check hangs
// unit tests because watchdogs are disabled under cargo-mutants.
#[cfg_attr(test, mutants::skip)]
fn pty_output_loop<T, C>(shared: &Shared<T, C>)
where
    T: Transport + Clone,
    C: Pseudoconsole,
{
    while !shared.stopping.load(Ordering::SeqCst) {
        let Ok(bytes) = shared.pty_host.read_output(shared.pty) else {
            break;
        };
        // Queued, never written here: a client that stopped reading must not be
        // able to hold the pump, which the exit teardown joins. A write failure
        // is handled by the outbox dropping the connection, which ends the
        // client's own loop and clears the slot.
        let client = shared.client().clone();
        if let Some(client) = client {
            client.outbox.send(Message::Output(bytes));
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;

    use testing::with_watchdog;

    use super::*;
    use crate::durability::Durability;
    use crate::pal::ids::{AppId, ConnId, JobId};
    use crate::pal::processes::MockProcesses;
    use crate::pal::pseudoconsole::MemoryPseudoconsole;
    use crate::pal::session_store::FsSessionStore;
    use crate::pal::transport::MemoryTransport;
    use crate::protocol::Message;
    use crate::session_record::ProcessIdentity;

    fn mock_processes(exit: Arc<(Mutex<bool>, Condvar)>) -> MockProcesses {
        mock_processes_with(exit, Durability::Durable)
    }

    fn mock_processes_with(
        exit: Arc<(Mutex<bool>, Condvar)>,
        durability: Durability,
    ) -> MockProcesses {
        let mut processes = MockProcesses::new();
        processes.expect_durability().returning(move || durability);
        processes
            .expect_create_lifetime_job()
            .returning(|| Ok(JobId(1)));
        processes.expect_spawn_app().returning(|_| Ok(AppId(1)));
        processes.expect_current_identity().returning(|| {
            Ok(ProcessIdentity {
                pid: 10,
                creation_time: 100,
            })
        });
        processes
            .expect_random_nonce()
            .returning(|| "nonce".to_string());
        processes.expect_close_job().returning(|_| ());
        processes.expect_wait_app().returning(move |_| {
            let (lock, cvar) = &*exit;
            let mut done = lock.lock().expect("exit lock");
            while !*done {
                done = cvar.wait(done).expect("exit wait");
            }
            Ok(7)
        });
        processes
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn steal_displaces_first_client() {
        with_watchdog(|| {
            let transport = MemoryTransport::new();
            let pty = MemoryPseudoconsole::new();
            let dir = tempfile::TempDir::new().unwrap();
            let store = FsSessionStore::new(dir.path().to_path_buf());
            let exit = Arc::new((Mutex::new(false), Condvar::new()));
            let processes = mock_processes(Arc::clone(&exit));

            let startup = transport.listen("startup").unwrap();
            let supervisor = thread::spawn({
                let transport = transport.clone();
                move || {
                    run_supervisor(
                        &processes,
                        &store,
                        &transport,
                        &pty,
                        "startup",
                        PathBuf::from("/work"),
                        vec!["app.exe".to_string()],
                    )
                }
            });

            let startup_conn = transport.accept(startup).unwrap();
            let Message::StartupOk { session_id, .. } = transport.recv(startup_conn).unwrap()
            else {
                panic!("expected startup ok");
            };
            transport.disconnect(startup_conn);

            let pipe = transport.pipe_name("nonce");
            let first = transport.connect(&pipe, CONNECT_TIMEOUT).unwrap();
            transport
                .send(first, &Message::Attach { cols: 80, rows: 24 })
                .unwrap();
            assert!(matches!(
                transport.recv(first).unwrap(),
                Message::Attached { session_id: id } if id == session_id
            ));

            let second = transport.connect(&pipe, CONNECT_TIMEOUT).unwrap();
            transport
                .send(
                    second,
                    &Message::Attach {
                        cols: 100,
                        rows: 30,
                    },
                )
                .unwrap();
            assert!(matches!(
                transport.recv(second).unwrap(),
                Message::Attached { .. }
            ));
            assert!(matches!(transport.recv(first).unwrap(), Message::Displaced));

            {
                let (lock, cvar) = &*exit;
                *lock.lock().expect("exit lock") = true;
                cvar.notify_all();
            }
            assert_eq!(supervisor.join().unwrap().unwrap(), 7);
            let leftover = FsSessionStore::new(dir.path().to_path_buf());
            assert!(leftover.list().unwrap().is_empty());
        });
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn final_output_arrives_before_the_exit_status() {
        with_watchdog(|| {
            let transport = MemoryTransport::new();
            let pty = MemoryPseudoconsole::new();
            let dir = tempfile::TempDir::new().unwrap();
            let store = FsSessionStore::new(dir.path().to_path_buf());
            let exit = Arc::new((Mutex::new(false), Condvar::new()));
            let processes = mock_processes(Arc::clone(&exit));

            let startup = transport.listen("startup").unwrap();
            let supervisor = thread::spawn({
                let transport = transport.clone();
                let pty = pty.clone();
                move || {
                    run_supervisor(
                        &processes,
                        &store,
                        &transport,
                        &pty,
                        "startup",
                        PathBuf::from("/work"),
                        vec!["app.exe".to_string()],
                    )
                }
            });

            let startup_conn = transport.accept(startup).unwrap();
            assert!(matches!(
                transport.recv(startup_conn).unwrap(),
                Message::StartupOk { .. }
            ));
            transport.disconnect(startup_conn);

            let client = transport
                .connect(&transport.pipe_name("nonce"), CONNECT_TIMEOUT)
                .unwrap();
            transport
                .send(client, &Message::Attach { cols: 80, rows: 24 })
                .unwrap();
            assert!(matches!(
                transport.recv(client).unwrap(),
                Message::Attached { .. }
            ));

            // The supervisor creates exactly one pty on this host, so it holds
            // the first allocated id.
            pty.push_output(PtyId(1), b"bye");
            {
                let (lock, cvar) = &*exit;
                *lock.lock().expect("exit lock") = true;
                cvar.notify_all();
            }

            assert_eq!(
                transport.recv(client).unwrap(),
                Message::Output(b"bye".to_vec())
            );
            assert_eq!(
                transport.recv(client).unwrap(),
                Message::AppExited { status: 7 }
            );
            assert_eq!(supervisor.join().unwrap().unwrap(), 7);
        });
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn an_app_that_exits_before_anyone_attaches_still_reports_its_status() {
        with_watchdog(|| {
            let transport = MemoryTransport::new();
            let pty = MemoryPseudoconsole::new();
            let dir = tempfile::TempDir::new().unwrap();
            let store = FsSessionStore::new(dir.path().to_path_buf());
            // The app is already gone by the time the supervisor waits on it.
            let exit = Arc::new((Mutex::new(true), Condvar::new()));
            let processes = mock_processes(Arc::clone(&exit));

            let startup = transport.listen("startup").unwrap();
            let supervisor = thread::spawn({
                let transport = transport.clone();
                move || {
                    run_supervisor(
                        &processes,
                        &store,
                        &transport,
                        &pty,
                        "startup",
                        PathBuf::from("/work"),
                        vec!["app.exe".to_string()],
                    )
                }
            });

            let startup_conn = transport.accept(startup).unwrap();
            assert!(matches!(
                transport.recv(startup_conn).unwrap(),
                Message::StartupOk { .. }
            ));

            // The startup connection stays open, which is what holds the
            // session up for the attach that `dure run` is about to make.
            let client = transport
                .connect(&transport.pipe_name("nonce"), CONNECT_TIMEOUT)
                .unwrap();
            transport
                .send(client, &Message::Attach { cols: 80, rows: 24 })
                .unwrap();
            assert!(matches!(
                transport.recv(client).unwrap(),
                Message::Attached { .. }
            ));
            assert_eq!(
                transport.recv(client).unwrap(),
                Message::AppExited { status: 7 }
            );
            assert_eq!(supervisor.join().unwrap().unwrap(), 7);
            transport.disconnect(startup_conn);
        });
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn a_session_nobody_comes_for_ends_when_its_initiator_gives_up() {
        with_watchdog(|| {
            let transport = MemoryTransport::new();
            let pty = MemoryPseudoconsole::new();
            let dir = tempfile::TempDir::new().unwrap();
            let store = FsSessionStore::new(dir.path().to_path_buf());
            let exit = Arc::new((Mutex::new(true), Condvar::new()));
            let processes = mock_processes(Arc::clone(&exit));

            let startup = transport.listen("startup").unwrap();
            let supervisor = thread::spawn({
                let transport = transport.clone();
                move || {
                    run_supervisor(
                        &processes,
                        &store,
                        &transport,
                        &pty,
                        "startup",
                        PathBuf::from("/work"),
                        vec!["app.exe".to_string()],
                    )
                }
            });

            let startup_conn = transport.accept(startup).unwrap();
            assert!(matches!(
                transport.recv(startup_conn).unwrap(),
                Message::StartupOk { .. }
            ));
            // Nobody will ever attach, so the gate must open on this instead.
            transport.disconnect(startup_conn);

            assert_eq!(supervisor.join().unwrap().unwrap(), 7);
            let leftover = FsSessionStore::new(dir.path().to_path_buf());
            assert!(leftover.list().unwrap().is_empty());
        });
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn init_failure_sends_startup_err_and_closes_job() {
        with_watchdog(|| {
            let transport = MemoryTransport::new();
            let pty = MemoryPseudoconsole::new();
            let dir = tempfile::TempDir::new().unwrap();
            let store = FsSessionStore::new(dir.path().to_path_buf());
            let mut processes = MockProcesses::new();
            processes
                .expect_durability()
                .returning(|| Durability::Durable);
            processes
                .expect_create_lifetime_job()
                .returning(|| Ok(JobId(1)));
            processes
                .expect_spawn_app()
                .returning(|_| Err(PalError::new(PalErrorKind::Other)));
            processes.expect_close_job().times(1).returning(|_| ());

            let startup = transport.listen("startup").unwrap();
            let supervisor = thread::spawn({
                let transport = transport.clone();
                let store = store.clone();
                move || {
                    run_supervisor(
                        &processes,
                        &store,
                        &transport,
                        &pty,
                        "startup",
                        PathBuf::from("/work"),
                        vec!["app.exe".to_string()],
                    )
                }
            });

            let startup_conn = transport.accept(startup).unwrap();
            assert_eq!(transport.recv(startup_conn).unwrap(), Message::StartupErr);
            supervisor.join().unwrap().unwrap_err();
            assert!(store.list().unwrap().is_empty());
        });
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn a_supervisor_that_cannot_outlive_its_launcher_says_so_on_the_startup_pipe() {
        with_watchdog(|| {
            let exit = Arc::new((Mutex::new(false), Condvar::new()));
            let transport = MemoryTransport::new();
            let pty = MemoryPseudoconsole::new();
            let dir = tempfile::TempDir::new().unwrap();
            let store = FsSessionStore::new(dir.path().to_path_buf());
            let processes = mock_processes_with(Arc::clone(&exit), Durability::TiedToLauncher);

            let startup = transport.listen("startup").unwrap();
            let supervisor = thread::spawn({
                let transport = transport.clone();
                move || {
                    run_supervisor(
                        &processes,
                        &store,
                        &transport,
                        &pty,
                        "startup",
                        PathBuf::from("/work"),
                        vec!["app.exe".to_string()],
                    )
                }
            });

            let startup_conn = transport.accept(startup).unwrap();
            // Only the client has a console to report this on.
            let Message::StartupOk { durability, .. } = transport.recv(startup_conn).unwrap()
            else {
                panic!("expected startup ok");
            };
            assert_eq!(durability, Durability::TiedToLauncher);
            transport.disconnect(startup_conn);
            {
                let (lock, cvar) = &*exit;
                *lock.lock().expect("exit lock") = true;
                cvar.notify_all();
            }
            supervisor.join().unwrap().unwrap();
        });
    }

    #[test]
    fn breakaway_denial_keeps_its_identity() {
        let denied = map_startup(&PalError::new(PalErrorKind::BreakawayDenied));
        assert!(denied.find_source::<BreakawayDeniedError>().is_some());
        let other = map_startup(&PalError::new(PalErrorKind::Other));
        assert!(other.find_source::<StartupFailedError>().is_some());
    }

    /// Session with a live pty, no client attached.
    fn shared_session(
        transport: &MemoryTransport,
        pty_host: &MemoryPseudoconsole,
    ) -> Shared<MemoryTransport, MemoryPseudoconsole> {
        Shared {
            transport: transport.clone(),
            pty_host: pty_host.clone(),
            pty: pty_host.create(DEFAULT_PTY_SIZE).unwrap(),
            session_id: SessionId::from_u32(1).unwrap(),
            client: Mutex::new(None),
            attach: Mutex::new(()),
            first_attach: Mutex::new(FirstAttach::default()),
            first_attach_changed: Condvar::new(),
            stopping: AtomicBool::new(false),
        }
    }

    /// Connection the client slot currently holds, if any.
    fn client_conn(shared: &Shared<MemoryTransport, MemoryPseudoconsole>) -> Option<ConnId> {
        shared.client().as_ref().map(|client| client.conn)
    }

    /// Connected pair as `(supervisor side, client side)`.
    fn connected_pair(transport: &MemoryTransport, name: &str) -> (ConnId, ConnId) {
        let listener = transport.listen(name).unwrap();
        let client = transport.connect(name, CONNECT_TIMEOUT).unwrap();
        let supervisor = transport.accept(listener).unwrap();
        (supervisor, client)
    }

    /// Records every attached-flag publication in order.
    fn attach_recorder() -> (Arc<Mutex<Vec<bool>>>, impl Fn(bool)) {
        let flags = Arc::new(Mutex::new(Vec::new()));
        let recorder = {
            let flags = Arc::clone(&flags);
            move |attached: bool| flags.lock().expect("flag lock").push(attached)
        };
        (flags, recorder)
    }

    #[test]
    fn acknowledgement_failure_leaves_the_slot_empty() {
        let transport = MemoryTransport::new();
        let pty_host = MemoryPseudoconsole::new();
        let shared = shared_session(&transport, &pty_host);
        let (supervisor, client) = connected_pair(&transport, "pipe");

        transport
            .send(client, &Message::Attach { cols: 80, rows: 24 })
            .unwrap();
        transport.disconnect(client);

        let (flags, recorder) = attach_recorder();
        client_loop(&shared, supervisor, &recorder);

        assert!(client_conn(&shared).is_none());
        assert!(flags.lock().unwrap().is_empty());
    }

    #[test]
    fn an_attach_that_arrives_after_teardown_claims_the_slot_is_refused() {
        let transport = MemoryTransport::new();
        let pty_host = MemoryPseudoconsole::new();
        let shared = shared_session(&transport, &pty_host);
        let (supervisor, client) = connected_pair(&transport, "pipe");
        // Teardown has already routed the exit status to whoever owned the
        // slot, so there is nothing left for a new client to be given.
        shared.stopping.store(true, Ordering::SeqCst);

        transport
            .send(client, &Message::Attach { cols: 80, rows: 24 })
            .unwrap();

        let (flags, recorder) = attach_recorder();
        client_loop(&shared, supervisor, &recorder);

        // Refused before the acknowledgement, so the client sees a session that
        // is gone rather than one that broke mid-relay.
        assert!(client_conn(&shared).is_none());
        assert!(flags.lock().unwrap().is_empty());
        transport.recv(client).unwrap_err();
    }

    #[test]
    fn a_client_that_does_not_attach_first_is_dropped() {
        let transport = MemoryTransport::new();
        let pty_host = MemoryPseudoconsole::new();
        let shared = shared_session(&transport, &pty_host);
        let (supervisor, client) = connected_pair(&transport, "pipe");

        transport
            .send(client, &Message::Input(b"x".to_vec()))
            .unwrap();

        let (flags, recorder) = attach_recorder();
        client_loop(&shared, supervisor, &recorder);

        assert!(client_conn(&shared).is_none());
        assert!(flags.lock().unwrap().is_empty());
        assert!(pty_host.take_input(shared.pty).is_empty());
        transport.recv(client).unwrap_err();
    }

    #[test]
    fn the_relay_forwards_input_and_resize_until_the_client_stops() {
        let transport = MemoryTransport::new();
        let pty_host = MemoryPseudoconsole::new();
        let shared = shared_session(&transport, &pty_host);
        let (supervisor, client) = connected_pair(&transport, "pipe");

        transport
            .send(client, &Message::Attach { cols: 80, rows: 24 })
            .unwrap();
        transport
            .send(
                client,
                &Message::Resize {
                    cols: 120,
                    rows: 40,
                },
            )
            .unwrap();
        transport
            .send(client, &Message::Input(b"hi".to_vec()))
            .unwrap();
        // Anything the supervisor does not relay ends the client loop.
        transport.send(client, &Message::StartupErr).unwrap();

        let (flags, recorder) = attach_recorder();
        client_loop(&shared, supervisor, &recorder);

        assert_eq!(
            pty_host.size(shared.pty),
            Some(WindowSize {
                cols: 120,
                rows: 40
            })
        );
        assert_eq!(pty_host.take_input(shared.pty), b"hi");
        assert!(client_conn(&shared).is_none());
        assert_eq!(*flags.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn a_displaced_relay_leaves_the_new_client_installed() {
        let transport = MemoryTransport::new();
        let pty_host = MemoryPseudoconsole::new();
        let shared = Arc::new(shared_session(&transport, &pty_host));
        let (supervisor, client) = connected_pair(&transport, "pipe");
        let (successor, _successor_client) = connected_pair(&transport, "successor");
        let successor_outbox = Outbox::start(transport.clone(), successor);
        let displaced = Arc::new(Mutex::new(None));
        // Stands in for a steal that lands between the acknowledgement and the
        // first relayed message. The displaced relay must not reach the app.
        let steal = {
            let shared = Arc::clone(&shared);
            let successor_outbox = Arc::clone(&successor_outbox);
            let displaced = Arc::clone(&displaced);
            move |attached: bool| {
                if attached {
                    let previous = shared.client().replace(Client {
                        conn: successor,
                        outbox: Arc::clone(&successor_outbox),
                    });
                    // What the real steal does to the client it replaces.
                    if let Some(previous) = previous {
                        previous.outbox.finish();
                        *displaced.lock().unwrap() = Some(previous.outbox);
                    }
                }
            }
        };

        transport
            .send(client, &Message::Attach { cols: 80, rows: 24 })
            .unwrap();
        transport
            .send(client, &Message::Input(b"x".to_vec()))
            .unwrap();

        client_loop(&shared, supervisor, &steal);

        assert!(pty_host.take_input(shared.pty).is_empty());
        assert_eq!(client_conn(&shared), Some(successor));

        displaced
            .lock()
            .unwrap()
            .take()
            .expect("the steal displaced the first client")
            .flush();
        successor_outbox.finish();
        successor_outbox.flush();
    }

    #[test]
    fn output_is_relayed_to_the_installed_client() {
        let transport = MemoryTransport::new();
        let pty_host = MemoryPseudoconsole::new();
        let shared = shared_session(&transport, &pty_host);
        let (supervisor, client) = connected_pair(&transport, "pipe");
        let outbox = Outbox::start(transport.clone(), supervisor);
        *shared.client() = Some(Client {
            conn: supervisor,
            outbox: Arc::clone(&outbox),
        });

        pty_host.push_output(shared.pty, b"out");
        // Ends the loop once the output has been drained.
        pty_host.close(shared.pty);

        pty_output_loop(&shared);
        outbox.finish();
        outbox.flush();

        assert!(matches!(
            transport.recv(client).unwrap(),
            Message::Output(bytes) if bytes == b"out",
        ));
    }

    #[test]
    fn output_for_a_client_that_is_gone_is_discarded() {
        let transport = MemoryTransport::new();
        let pty_host = MemoryPseudoconsole::new();
        let shared = shared_session(&transport, &pty_host);
        let (supervisor, client) = connected_pair(&transport, "pipe");
        let outbox = Outbox::start(transport.clone(), supervisor);
        *shared.client() = Some(Client {
            conn: supervisor,
            outbox: Arc::clone(&outbox),
        });
        transport.disconnect(client);

        pty_host.push_output(shared.pty, b"out");
        // Ends the loop once the undeliverable output has been drained.
        pty_host.close(shared.pty);

        // The pump must not be the thread that notices, so it completes even
        // though nothing can be delivered.
        pty_output_loop(&shared);
        outbox.flush();
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn a_failure_after_id_allocation_releases_the_id() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let transport = MemoryTransport::new();
        let pty = MemoryPseudoconsole::new();
        let mut processes = MockProcesses::new();
        processes
            .expect_durability()
            .returning(|| Durability::Durable);
        processes
            .expect_create_lifetime_job()
            .returning(|| Ok(JobId(1)));
        processes.expect_spawn_app().returning(|_| Ok(AppId(1)));
        processes
            .expect_random_nonce()
            .returning(|| "nonce".to_string());
        processes
            .expect_current_identity()
            .returning(|| Err(PalError::new(PalErrorKind::Other)));
        processes.expect_close_job().returning(|_| ());

        {
            let mut guard = InitGuard {
                processes: &processes,
                store: &store,
                pty_host: &pty,
                job: None,
                pty: None,
                session: None,
                committed: false,
            };
            let error = initialize(
                &mut guard,
                &processes,
                &store,
                &transport,
                &pty,
                PathBuf::from("/work"),
                vec!["app.exe".to_string()],
            )
            .err()
            .expect("identity lookup fails");
            assert!(error.find_source::<StartupFailedError>().is_some());
        }

        assert!(!dir.path().join("1.json").exists());
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn attached_flag_publishes_only_while_the_record_lives() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
        store
            .publish(&SessionRecord {
                id: id.get(),
                supervisor_pid: 10,
                supervisor_creation_time: 100,
                pipe_name: "pipe".to_string(),
                launch_directory: PathBuf::from("/work"),
                command: vec!["app.exe".to_string()],
                started_at_unix_ms: 1,
                attached: false,
            })
            .unwrap();

        let record_live = Arc::new(Mutex::new(true));
        let set_attached = store_attached_flag(&store, id, Arc::clone(&record_live));
        set_attached(true);
        assert!(store.read(id).unwrap().unwrap().attached);

        *record_live.lock().expect("record_live lock") = false;
        set_attached(false);
        assert!(store.read(id).unwrap().unwrap().attached);
    }
}
