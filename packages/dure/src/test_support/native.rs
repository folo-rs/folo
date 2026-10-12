//! Owned native resources for fault and lifetime integration scenarios.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fmt, io};

use windows::Win32::Foundation::HANDLE;

use crate::AppCommand;
use crate::constants::CONNECT_TIMEOUT;
use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::{ConnId, ListenerId};
use crate::pal::processes::{Processes, ProcessesFacade, search_path_in};
use crate::pal::pseudoconsole::{Pseudoconsole, PseudoconsoleFacade};
use crate::pal::raw_handle::PipeHandle;
use crate::pal::session_store::FsSessionStore;
use crate::pal::transport::{Transport, TransportFacade, accept_with_replacement};
use crate::supervisor::{InitGuard, Initialized, SessionSpec, initialize};

/// Owns a real listener through an injected replacement failure.
#[derive(Debug)]
pub struct Listener {
    transport: TransportFacade,
    id: ListenerId,
    name: String,
}

impl Listener {
    /// Creates a uniquely named, user-restricted pipe.
    #[must_use]
    pub fn new() -> Self {
        let transport = TransportFacade::target();
        let name = transport.pipe_name(&ProcessesFacade::target().random_nonce());
        let id = transport
            .listen(&name)
            .expect("create native test listener");
        Self {
            transport,
            id,
            name,
        }
    }

    /// Opens the client endpoint before the server accepts it.
    #[must_use]
    pub fn connect(&self) -> Connection {
        let id = self
            .transport
            .connect(&self.name, CONNECT_TIMEOUT)
            .expect("connect native test pipe");
        Connection {
            transport: self.transport.clone(),
            id,
        }
    }

    /// Injects a native-instance creation error after the connection is accepted.
    pub fn accept_failing_replacement(&self, cause: &str) -> io::Result<()> {
        accept_with_replacement(self.id, None, |_| {
            Err(PalError::with_source(
                PalErrorKind::Other,
                io::Error::other(cause.to_owned()),
            ))
        })
        .map(|_| ())
        .map_err(io::Error::other)
    }
}

impl Default for Listener {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.transport.close_listener(self.id);
    }
}

/// Owns the peer whose closure the native fault scenario observes.
#[derive(Debug)]
pub struct Connection {
    transport: TransportFacade,
    id: ConnId,
}

impl Connection {
    /// Waits for a frame or a real peer-closure error.
    pub fn receive(&self) -> io::Result<()> {
        self.transport.recv(self.id).map(|_| ()).map_err(|error| {
            let kind = if error.kind() == PalErrorKind::Disconnected {
                io::ErrorKind::BrokenPipe
            } else {
                io::ErrorKind::Other
            };
            io::Error::new(kind, error)
        })
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.transport.disconnect(self.id);
    }
}

/// A real startup deliberately left without the client's commit or serving pump.
pub struct UncommittedApp {
    initialized: Initialized,
    store: FsSessionStore,
}

impl fmt::Debug for UncommittedApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .finish_non_exhaustive()
    }
}

impl UncommittedApp {
    /// Acquires real startup resources but does not commit the startup exchange.
    #[must_use]
    pub fn spawn(exe: &Path, args: &[String], directory: &Path) -> Self {
        let processes = ProcessesFacade::target();
        let transport = TransportFacade::target();
        let pty_host = PseudoconsoleFacade::target();
        let store = FsSessionStore::new(directory.to_path_buf());
        let mut guard = InitGuard {
            processes: &processes,
            store: &store,
            transport: &transport,
            pty_host: &pty_host,
            job: None,
            pty: None,
            listener: None,
            session: None,
            committed: false,
        };
        let mut argv = vec![exe.to_string_lossy().into_owned()];
        argv.extend_from_slice(args);
        let spec = SessionSpec {
            launch_directory: directory.to_path_buf(),
            command: AppCommand::from_argv(argv).unwrap(),
            // An arbitrary fixture timestamp: no age is rendered in this scenario.
            started_at_unix_ms: 1,
        };
        let initialized = initialize(&mut guard, &processes, &store, &transport, &pty_host, spec)
            .unwrap_or_else(|failure| panic!("native test startup failed: {}", failure.error));
        // Ownership moves into this fixture, whose Drop invokes the same
        // uncommitted guard. No serving thread or startup acknowledgement exists.
        guard.committed = true;
        drop(guard);
        Self { initialized, store }
    }

    /// Reads a single burst to establish that output exists before rollback.
    ///
    /// This starts no pump: further output remains undrained during closure.
    pub fn read_output(&self) -> io::Result<Option<Vec<u8>>> {
        PseudoconsoleFacade::target()
            .read_output(self.initialized.pty)
            .map_err(io::Error::other)
    }
}

impl Drop for UncommittedApp {
    fn drop(&mut self) {
        let processes = ProcessesFacade::target();
        let transport = TransportFacade::target();
        let pty_host = PseudoconsoleFacade::target();
        let resources = &self.initialized;
        drop(InitGuard {
            processes: &processes,
            store: &self.store,
            transport: &transport,
            pty_host: &pty_host,
            job: Some(resources.job),
            pty: Some(resources.pty),
            listener: Some(resources.listener),
            session: Some((resources.session_id, resources.identity)),
            committed: false,
        });
        _ = processes
            .wait_app(resources.app)
            .expect("the rollback-ended app remains waitable");
    }
}

/// Searches only a test-owned directory, without mutating process-wide PATH.
#[must_use]
pub fn find_executable(directory: &Path, command: &str) -> Option<PathBuf> {
    search_path_in(directory.as_os_str(), command, ".exe")
}

/// Exercises cancellation ordering on a detached, non-owning native handle value.
///
/// The null handle cannot affect a kernel object, but cancellation still crosses
/// the Win32 boundary, so these scenarios belong in integration tests.
#[derive(Clone)]
pub struct CancellationProbe(Arc<PipeHandle>);

impl fmt::Debug for CancellationProbe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .finish_non_exhaustive()
    }
}

impl Default for CancellationProbe {
    fn default() -> Self {
        Self(PipeHandle::new(HANDLE::default()))
    }
}

impl CancellationProbe {
    /// Whether an operation may begin.
    #[must_use]
    pub fn issue(&self) -> bool {
        self.0.issue(|_| ()).is_some()
    }

    /// Prevents subsequent issuance while cancelling outstanding native I/O.
    pub fn cancel(&self) {
        self.0.cancel();
    }
}
