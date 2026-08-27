//! Supervisor role: own the app, accept clients, last-connect-wins steal.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use ohno::AppError;

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::{AppId, ConnId, JobId, ListenerId, PtyId};
use crate::pal::processes::{AppSpawn, Processes};
use crate::pal::pseudoconsole::{Pseudoconsole, WindowSize};
use crate::pal::session_store::SessionStore;
use crate::pal::transport::Transport;
use crate::protocol::Message;
use crate::session_id::SessionId;
use crate::session_record::SessionRecord;
use crate::{BreakawayDeniedError, PalFailedError, StartupFailedError, StoreError};

/// Size used until the first client attaches.
///
/// VGA text-mode geometry (`DEFAULT_PTY_COLS` by `DEFAULT_PTY_ROWS`). The first
/// attach always resizes to the client's real size (design.md, "Attach, detach,
/// steal").
const DEFAULT_PTY_SIZE: WindowSize = WindowSize {
    cols: crate::constants::DEFAULT_PTY_COLS,
    rows: crate::constants::DEFAULT_PTY_ROWS,
};

/// Resources that must be torn down if initialization fails.
struct InitGuard<'a, P: Processes, S: SessionStore, C: Pseudoconsole> {
    processes: &'a P,
    store: &'a S,
    pty_host: &'a C,
    job: Option<JobId>,
    pty: Option<PtyId>,
    session_id: Option<SessionId>,
    committed: bool,
}

impl<P: Processes, S: SessionStore, C: Pseudoconsole> Drop for InitGuard<'_, P, S, C> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some(id) = self.session_id {
            _ = self.store.delete(id);
        }
        if let Some(pty) = self.pty {
            self.pty_host.close(pty);
        }
        if let Some(job) = self.job {
            self.processes.close_job(job);
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
        .connect(startup_pipe, crate::constants::CONNECT_TIMEOUT)
        .map_err(|_error| StartupFailedError::new())?;

    let mut guard = InitGuard {
        processes,
        store,
        pty_host,
        job: None,
        pty: None,
        session_id: None,
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
        },
    );
    transport.disconnect(startup);
    guard.committed = true;

    let status = serve(processes, store, transport, pty_host, &initialized)?;
    Ok(status)
}

#[derive(Clone, Copy)]
struct Initialized {
    session_id: SessionId,
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

    let session_id = store.allocate_id().map_err(|_error| StoreError::new())?;
    guard.session_id = Some(session_id);

    let identity = processes
        .current_identity()
        .map_err(|error| map_startup(&error))?;
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

struct Shared<T, C> {
    transport: T,
    pty_host: C,
    pty: PtyId,
    session_id: SessionId,
    client: Mutex<Option<ConnId>>,
    stopping: AtomicBool,
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
) -> Result<i32, AppError>
where
    P: Processes,
    S: SessionStore + Clone,
    T: Transport + Clone + Send + Sync + 'static,
    C: Pseudoconsole + Clone + Send + Sync + 'static,
{
    let Initialized {
        session_id,
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
        stopping: AtomicBool::new(false),
    });

    let store_flag = store_attached_flag(store, session_id, Arc::clone(&record_live));
    thread::spawn({
        let shared = Arc::clone(&shared);
        let transport = transport.clone();
        let store_flag = store_flag.clone();
        move || accept_loop(&shared, &transport, listener, store_flag)
    });

    thread::spawn({
        let shared = Arc::clone(&shared);
        move || pty_output_loop(&shared, store_flag)
    });

    let status = processes
        .wait_app(app)
        .map_err(|_error| PalFailedError::new())?;
    transport.close_listener(listener);
    shared.stopping.store(true, Ordering::SeqCst);

    if let Some(client) = shared
        .client
        .lock()
        .expect("client slot is only copied or replaced, never held across a panic")
        .take()
    {
        // Deliver the exit status before tearing down the store or pipes.
        // Attach treats a disconnect without `AppExited` as a relay failure
        // when the input thread has already stopped, so the status must
        // arrive first.
        _ = shared
            .transport
            .send(client, &Message::AppExited { status });
        shared.transport.disconnect(client);
    }

    {
        // Client threads take this lock before publishing an attached-flag
        // update. Clearing it first prevents a late publish from recreating
        // the record after delete, including over a reused session id.
        let mut live = record_live
            .lock()
            .expect("record_live is only set false here, never held across a panic");
        *live = false;
        store
            .delete(session_id)
            .map_err(|_error| StoreError::new())?;
    }
    pty_host.close(pty);
    processes.close_job(job);
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
    T: Transport,
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
    T: Transport,
    C: Pseudoconsole,
{
    match shared.transport.recv(conn) {
        Ok(Message::Attach { cols, rows }) => {
            // Resize failure means the pty is already gone; wait_app and
            // read_output observe that and stop the relay.
            _ = shared
                .pty_host
                .resize(shared.pty, WindowSize { cols, rows });
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
                shared.transport.disconnect(conn);
                return;
            }
        }
        _ => {
            shared.transport.disconnect(conn);
            return;
        }
    }

    let previous = {
        let mut slot = shared
            .client
            .lock()
            .expect("client slot is only copied or replaced, never held across a panic");
        slot.replace(conn)
    };
    if let Some(old) = previous {
        // The displaced client may already have disconnected. Steal still
        // proceeds; last-connect-wins does not depend on this notice.
        _ = shared.transport.send(old, &Message::Displaced);
        shared.transport.disconnect(old);
    }
    set_attached(true);

    loop {
        match shared.transport.recv(conn) {
            Ok(Message::Input(data)) => {
                // Input after the app has exited is dropped. wait_app publishes
                // AppExited to the live client.
                _ = shared.pty_host.write_input(shared.pty, &data);
            }
            Ok(Message::Resize { cols, rows }) => {
                _ = shared
                    .pty_host
                    .resize(shared.pty, WindowSize { cols, rows });
            }
            Ok(_) | Err(_) => break,
        }
        let current = shared
            .client
            .lock()
            .expect("client slot is only copied or replaced, never held across a panic");
        if current.as_ref() != Some(&conn) {
            break;
        }
    }
    {
        let mut slot = shared
            .client
            .lock()
            .expect("client slot is only copied or replaced, never held across a panic");
        if slot.as_ref() == Some(&conn) {
            *slot = None;
            // Holding the slot so a replacement cannot publish attached=true
            // before this disconnect publishes attached=false.
            set_attached(false);
        }
    }
}

// Blocking read of pty output. A mutation that drops the stop check hangs
// unit tests because watchdogs are disabled under cargo-mutants.
#[cfg_attr(test, mutants::skip)]
fn pty_output_loop<T, C>(shared: &Shared<T, C>, set_attached: impl Fn(bool))
where
    T: Transport,
    C: Pseudoconsole,
{
    while !shared.stopping.load(Ordering::SeqCst) {
        let Ok(bytes) = shared.pty_host.read_output(shared.pty) else {
            break;
        };
        let client = shared
            .client
            .lock()
            .expect("client slot is only copied or replaced, never held across a panic")
            .as_ref()
            .copied();
        if let Some(conn) = client
            && shared
                .transport
                .send(conn, &Message::Output(bytes))
                .is_err()
        {
            let mut slot = shared
                .client
                .lock()
                .expect("client slot is only copied or replaced, never held across a panic");
            if slot.as_ref() == Some(&conn) {
                *slot = None;
                shared.transport.disconnect(conn);
                set_attached(false);
            }
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
    use crate::pal::ids::{AppId, ConnId, JobId};
    use crate::pal::processes::MockProcesses;
    use crate::pal::pseudoconsole::MemoryPseudoconsole;
    use crate::pal::session_store::FsSessionStore;
    use crate::pal::transport::MemoryTransport;
    use crate::protocol::Message;
    use crate::session_record::ProcessIdentity;

    fn mock_processes(exit: Arc<(Mutex<bool>, Condvar)>) -> MockProcesses {
        let mut processes = MockProcesses::new();
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
            let Message::StartupOk { session_id } = transport.recv(startup_conn).unwrap() else {
                panic!("expected startup ok");
            };
            transport.disconnect(startup_conn);

            let pipe = transport.pipe_name("nonce");
            let first = transport
                .connect(&pipe, crate::constants::CONNECT_TIMEOUT)
                .unwrap();
            transport
                .send(first, &Message::Attach { cols: 80, rows: 24 })
                .unwrap();
            assert!(matches!(
                transport.recv(first).unwrap(),
                Message::Attached { session_id: id } if id == session_id
            ));

            let second = transport
                .connect(&pipe, crate::constants::CONNECT_TIMEOUT)
                .unwrap();
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
    fn init_failure_sends_startup_err_and_closes_job() {
        with_watchdog(|| {
            let transport = MemoryTransport::new();
            let pty = MemoryPseudoconsole::new();
            let dir = tempfile::TempDir::new().unwrap();
            let store = FsSessionStore::new(dir.path().to_path_buf());
            let mut processes = MockProcesses::new();
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
            assert!(matches!(
                transport.recv(startup_conn).unwrap(),
                Message::StartupErr
            ));
            supervisor.join().unwrap().unwrap_err();
            assert!(store.list().unwrap().is_empty());
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
            stopping: AtomicBool::new(false),
        }
    }

    /// Connected pair as `(supervisor side, client side)`.
    fn connected_pair(transport: &MemoryTransport, name: &str) -> (ConnId, ConnId) {
        let listener = transport.listen(name).unwrap();
        let client = transport
            .connect(name, crate::constants::CONNECT_TIMEOUT)
            .unwrap();
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

        assert!(shared.client.lock().unwrap().is_none());
        assert!(flags.lock().unwrap().is_empty());
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

        assert!(shared.client.lock().unwrap().is_none());
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
        assert!(shared.client.lock().unwrap().is_none());
        assert_eq!(*flags.lock().unwrap(), vec![true, false]);
    }

    #[test]
    fn a_displaced_relay_leaves_the_new_client_installed() {
        let transport = MemoryTransport::new();
        let pty_host = MemoryPseudoconsole::new();
        let shared = Arc::new(shared_session(&transport, &pty_host));
        let (supervisor, client) = connected_pair(&transport, "pipe");
        // Stands in for a steal that lands between the acknowledgement and the
        // first relayed message, which the loop can only observe on its own
        // next pass over the client slot.
        let successor = ConnId(u64::MAX);
        let steal = {
            let shared = Arc::clone(&shared);
            move |attached: bool| {
                if attached {
                    *shared.client.lock().expect("client lock") = Some(successor);
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

        assert_eq!(pty_host.take_input(shared.pty), b"x");
        assert_eq!(*shared.client.lock().unwrap(), Some(successor));
    }

    #[test]
    fn output_that_cannot_be_delivered_drops_the_client() {
        let transport = MemoryTransport::new();
        let pty_host = MemoryPseudoconsole::new();
        let shared = shared_session(&transport, &pty_host);
        let (supervisor, client) = connected_pair(&transport, "pipe");
        *shared.client.lock().unwrap() = Some(supervisor);
        transport.disconnect(client);

        pty_host.push_output(shared.pty, b"out");
        // Ends the loop once the undeliverable output has been drained.
        pty_host.close(shared.pty);

        let (flags, recorder) = attach_recorder();
        pty_output_loop(&shared, recorder);

        assert!(shared.client.lock().unwrap().is_none());
        assert_eq!(*flags.lock().unwrap(), vec![false]);
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
                session_id: None,
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
        let id = store.allocate_id().unwrap();
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
