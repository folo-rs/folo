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
/// Matches the historical Windows default console size. The first attach
/// always sends a resize with the client's real size (design.md, "Attach,
/// detach, steal").
const DEFAULT_PTY_SIZE: WindowSize = WindowSize { cols: 80, rows: 24 };

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

    let shared = Arc::new(Shared {
        transport: transport.clone(),
        pty_host: pty_host.clone(),
        pty,
        session_id,
        client: Mutex::new(None),
        stopping: AtomicBool::new(false),
    });

    thread::spawn({
        let shared = Arc::clone(&shared);
        let transport = transport.clone();
        let store_flag = store_attached_flag(store, session_id);
        move || accept_loop(&shared, &transport, listener, store_flag)
    });

    thread::spawn({
        let shared = Arc::clone(&shared);
        move || pty_output_loop(&shared)
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
        _ = shared
            .transport
            .send(client, &Message::AppExited { status });
        shared.transport.disconnect(client);
    }

    store
        .delete(session_id)
        .map_err(|_error| StoreError::new())?;
    pty_host.close(pty);
    processes.close_job(job);
    Ok(status)
}

fn store_attached_flag<S: SessionStore + Clone>(
    store: &S,
    id: SessionId,
) -> impl Fn(bool) + Clone + Send + 'static {
    let store = store.clone();
    move |attached: bool| {
        if let Ok(Some(mut record)) = store.read(id) {
            record.attached = attached;
            _ = store.publish(&record);
        }
    }
}

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
        let previous = {
            let mut slot = shared
                .client
                .lock()
                .expect("client slot is only copied or replaced, never held across a panic");
            slot.replace(conn)
        };
        if let Some(old) = previous {
            _ = transport.send(old, &Message::Displaced);
            transport.disconnect(old);
        }
        thread::spawn({
            let shared = Arc::clone(shared);
            let set_attached = set_attached.clone();
            move || client_loop(&shared, conn, &set_attached)
        });
    }
}

fn client_loop<T, C>(shared: &Shared<T, C>, conn: ConnId, set_attached: &impl Fn(bool))
where
    T: Transport,
    C: Pseudoconsole,
{
    match shared.transport.recv(conn) {
        Ok(Message::Attach { cols, rows }) => {
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
                return;
            }
            set_attached(true);
        }
        _ => {
            shared.transport.disconnect(conn);
            return;
        }
    }

    loop {
        match shared.transport.recv(conn) {
            Ok(Message::Input(data)) => {
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
    set_attached(false);
}

fn pty_output_loop<T, C>(shared: &Shared<T, C>)
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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;

    use testing::with_watchdog;

    use crate::pal::ids::{AppId, JobId};
    use crate::pal::processes::MockProcesses;
    use crate::pal::pseudoconsole::MemoryPseudoconsole;
    use crate::pal::session_store::FsSessionStore;
    use crate::pal::transport::MemoryTransport;
    use crate::protocol::Message;
    use crate::session_record::ProcessIdentity;

    use super::*;

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
        });
    }

    #[test]
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
}
