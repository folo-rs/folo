//! Helpers for Windows integration tests. Not part of the product.

use std::path::Path;

use crate::pal::ids::{AppId, JobId, PtyId};
use crate::pal::processes::{AppSpawn, Processes, ProcessesFacade};
use crate::pal::pseudoconsole::{Pseudoconsole, PseudoconsoleFacade, WindowSize};

/// A process started inside a test-owned pseudoconsole.
///
/// Integration tests use this so they do not depend on the runner having an
/// interactive console (implementation.md, "Integration tests").
#[derive(Debug)]
pub struct ConsoleProcess {
    processes: ProcessesFacade,
    pty_host: PseudoconsoleFacade,
    app: AppId,
    pty: PtyId,
    job: JobId,
    closed: bool,
}

impl ConsoleProcess {
    /// Spawn `exe` with `args` in `cwd`, attached to a new `ConPTY`.
    #[must_use]
    pub fn spawn(exe: &Path, args: &[String], cwd: &Path) -> Self {
        let processes = ProcessesFacade::target();
        let pty_host = PseudoconsoleFacade::target();
        let job = processes
            .create_lifetime_job()
            .expect("create test lifetime job");
        let pty = pty_host
            .create(WindowSize {
                cols: crate::constants::DEFAULT_PTY_COLS,
                rows: crate::constants::DEFAULT_PTY_ROWS,
            })
            .expect("create test pseudoconsole");
        let mut command = Vec::with_capacity(args.len().saturating_add(1));
        command.push(exe.to_string_lossy().into_owned());
        command.extend(args.iter().cloned());
        let app = processes
            .spawn_app(&AppSpawn {
                command,
                launch_directory: cwd.to_path_buf(),
                pty,
                job,
            })
            .expect("spawn test client in pseudoconsole");
        Self {
            processes,
            pty_host,
            app,
            pty,
            job,
            closed: false,
        }
    }

    /// Write bytes to the child's console input.
    pub fn write_input(&self, data: &[u8]) {
        self.pty_host
            .write_input(self.pty, data)
            .expect("write test console input");
    }

    /// Block until the child writes console output.
    #[must_use]
    pub fn read_output(&self) -> Vec<u8> {
        self.pty_host
            .read_output(self.pty)
            .expect("read test console output")
    }

    /// Wait for the child to exit and tear down the job and pseudoconsole.
    ///
    /// Output is drained on a helper thread so a child that writes to the
    /// pseudoconsole cannot block on a full pipe while this wait runs.
    #[must_use]
    pub fn wait(mut self) -> i32 {
        let drain = std::thread::spawn({
            let pty_host = self.pty_host.clone();
            let pty = self.pty;
            move || loop {
                match pty_host.read_output(pty) {
                    Ok(bytes) if bytes.is_empty() => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
        let status = self.processes.wait_app(self.app).expect("wait test child");
        self.shutdown();
        _ = drain.join();
        status
    }

    fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        // Same ordering the supervisor's teardown relies on: the child stays
        // attached to the pseudoconsole until the job that owns its lifetime is
        // closed, and closing a pseudoconsole waits for its attached clients. A
        // drop while the child is still running would otherwise never reach
        // `close_job`.
        self.processes.close_job(self.job);
        self.pty_host.close(self.pty);
        self.closed = true;
    }
}

impl Drop for ConsoleProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}
