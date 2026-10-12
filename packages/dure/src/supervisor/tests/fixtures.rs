//! Prepared supervisor state and deterministic coordination for scenario tests.

use super::*;

/// A publication time with no structure of its own; the age column has its
/// own tests in `list_fmt`.
pub(crate) const SOME_STARTED_AT_MS: u64 = 1;

/// What a teardown ended, for tests about the order it ends things in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Torn {
    /// The job that owns the app's and its descendants' lifetime.
    Job,
    /// The pseudoconsole they are attached to.
    Console,
}

/// An ordinary session to run, for tests about something other than what
/// the session happens to be.
pub(crate) fn sample_spec() -> SessionSpec {
    SessionSpec {
        launch_directory: PathBuf::from("/work"),
        command: AppCommand::for_test(&["app.exe"]),
        started_at_unix_ms: SOME_STARTED_AT_MS,
    }
}

/// Arbitrary nonzero status the mock app exits with, so a test can tell a
/// forwarded status from a defaulted one.
pub(crate) const SAMPLE_APP_EXIT: i32 = 7;

/// Ordinary valid geometry for tests where resize behavior is out of scope.
pub(crate) const ORDINARY_ATTACH: Message = Message::Attach {
    size: WindowSize::new(80, 24).expect("a fixture size is not empty"),
};

/// What the supervisor reported when it came up.
///
/// Tests take the session pipe from here rather than rebuilding it from the
/// nonce the mock happens to return, so a change to that fixture cannot break
/// a test about something else.
pub(crate) struct StartedSession {
    pub(crate) startup_conn: ConnId,
    pub(crate) session_id: SessionId,
    pub(crate) launcher_tie: LauncherTie,
    pub(crate) pipe_name: String,
}

/// Completes the client side of the startup commit handshake.
pub(crate) fn commit_startup(
    transport: &MemoryTransport,
    listener: ListenerId,
    phase_reporter: &WatchdogPhaseReporter,
) -> StartedSession {
    phase_reporter.report("waiting for the supervisor startup connection");
    let conn = transport.accept(listener).unwrap();
    phase_reporter.report("waiting for the supervisor startup response");
    let Message::StartupOk {
        session_id,
        launcher_tie,
        pipe_name,
    } = transport.recv(conn).unwrap()
    else {
        panic!("expected startup ok");
    };
    transport.send(conn, &Message::StartupCommit).unwrap();
    StartedSession {
        startup_conn: conn,
        session_id,
        launcher_tie,
        pipe_name,
    }
}

pub(crate) fn mock_processes(exit: Arc<(Mutex<bool>, Condvar)>) -> MockProcesses {
    mock_processes_with(exit, LauncherTie::NoneDetected, AppWait::Reports)
}

/// How the mock app's wait ends once the test releases it.
#[derive(Clone, Copy)]
pub(crate) enum AppWait {
    /// The app exited and its status is known.
    Reports,
    /// The wait itself failed, so no status exists to report.
    Fails,
}

pub(crate) fn mock_processes_with(
    exit: Arc<(Mutex<bool>, Condvar)>,
    launcher_tie: LauncherTie,
    wait: AppWait,
) -> MockProcesses {
    mock_processes_with_close_job(exit, launcher_tie, wait, |_| {})
}

/// Configures startup, controlled app completion, and observable job cleanup
/// without starting a process or depending on scheduler delays.
pub(crate) fn mock_processes_with_close_job(
    exit: Arc<(Mutex<bool>, Condvar)>,
    launcher_tie: LauncherTie,
    wait: AppWait,
    close_job: impl Fn(JobId) + Send + Sync + 'static,
) -> MockProcesses {
    let mut processes = MockProcesses::new();
    processes
        .expect_launcher_tie()
        .returning(move || launcher_tie);
    processes
        .expect_create_lifetime_job()
        .returning(|| Ok(JobId::for_test(1)));
    processes
        .expect_spawn_app()
        .returning(|_| Ok(AppId::for_test(1)));
    processes.expect_current_identity().returning(|| {
        Ok(ProcessIdentity {
            pid: 10_u32.try_into().unwrap(),
            creation_time: 100,
        })
    });
    processes
        .expect_random_nonce()
        .returning(|| "nonce".to_string());
    processes.expect_close_job().returning(close_job);
    processes.expect_wait_app().returning(move |_| {
        let (lock, cvar) = &*exit;
        let mut done = lock
            .lock()
            .expect("the exit flag is read or set without callbacks or assertions");
        while !*done {
            done = cvar
                .wait(done)
                .expect("the exit flag is read or set without callbacks or assertions");
        }
        match wait {
            AppWait::Reports => Ok(SAMPLE_APP_EXIT),
            AppWait::Fails => Err(PalError::new(PalErrorKind::Other)),
        }
    });
    processes
}

/// Session with a live pty, no client attached.
pub(crate) fn shared_session<T: Transport + Clone, C: Pseudoconsole + Clone>(
    transport: &T,
    pty_host: &C,
) -> Shared<T, C> {
    Shared {
        transport: transport.clone(),
        pty_host: pty_host.clone(),
        pty: pty_host.create(DEFAULT_PTY_SIZE).unwrap(),
        session_id: SessionId::from_u32(1).unwrap(),
        client: Mutex::new(None),
        attach: Mutex::new(()),
        attached_generation: Arc::new(AtomicU64::default()),
        preamble: Mutex::new(Some(Vec::new())),
        first_attach: Mutex::new(FirstAttach::default()),
        first_attach_changed: Condvar::new(),
        stopping: AtomicBool::new(false),
    }
}

/// Connection the client slot currently holds, if any.
pub(crate) fn client_conn<T: Transport, C>(shared: &Shared<T, C>) -> Option<ConnId> {
    shared.client().as_ref().map(|client| client.conn)
}

/// Connected pair as `(supervisor side, client side)`.
pub(crate) fn connected_pair(transport: &MemoryTransport, name: &str) -> (ConnId, ConnId) {
    let listener = transport.listen(name).unwrap();
    let client = transport.connect(name, CONNECT_TIMEOUT).unwrap();
    let supervisor = transport.accept(listener).unwrap();
    (supervisor, client)
}

/// Records every attached-flag publication in order.
pub(crate) fn attach_recorder() -> (Arc<Mutex<Vec<bool>>>, impl Fn(u64, bool)) {
    let flags = Arc::new(Mutex::new(Vec::new()));
    let recorder = {
        let flags = Arc::clone(&flags);
        move |_generation: u64, attached: bool| {
            flags
                .lock()
                .expect("the recorder only appends values under its lock")
                .push(attached);
        }
    };
    (flags, recorder)
}
