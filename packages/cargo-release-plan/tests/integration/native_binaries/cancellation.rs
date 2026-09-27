//! Cancellation retains process-tree ownership without waiting for a failed fixture handshake.

use std::io::{self, ErrorKind, Read, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use serde_json::json;

#[cfg(windows)]
use crate::native_binaries::cancellation_windows::spawn_controller;
#[cfg(unix)]
use crate::native_binaries::command;
#[cfg(windows)]
use crate::native_binaries::compile_tool;
use crate::native_binaries::{Fixture, SMOKE_WATCHDOG, run, write};

/// Completion notifications for the fixture's current I/O operation and owned controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FixtureEvent {
    Connected,
    Read,
    ControllerClosed,
    Abort,
}

/// A watchdog failure wakes the fixture worker so it can close sockets and reap its controller.
struct AbortFixture(Sender<FixtureEvent>);

impl Drop for AbortFixture {
    fn drop(&mut self) {
        // A completed fixture has already dropped its receiver.
        _ = self.0.send(FixtureEvent::Abort);
    }
}

#[test]
fn cancellation_terminates_the_process_tree_and_cleans_the_source_worktree() {
    let (events, received) = mpsc::channel();
    let _abort = AbortFixture(events.clone());
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, move || {
        let mut fixture = Fixture::new();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        write(fixture.root.path(), "alpha/build.rs", BUILD_SCRIPT);
        fixture.commit_source();
        let mut controller = fixture.batch_command(
            &json!([fixture.binary("alpha"), fixture.binary("beta")]),
            "out",
        );
        controller
            .arg("--no-upload")
            .env(
                "RELEASE_FIXTURE_SOCKET",
                listener.local_addr().unwrap().to_string(),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(unix)]
        let mut process = controller.process_group(0).spawn().unwrap();
        #[cfg(windows)]
        let (mut process, mut control) = spawn_controller(&fixture, &controller);
        let mut stdout = process.stdout.take().unwrap();
        // Observing stdout closure leaves Child unreaped on this thread, so readiness failures
        // cannot race a background waiter recycling the PID before the fixture signals it.
        let closed = thread::spawn({
            let events = events.clone();
            move || {
                let result = io::copy(&mut stdout, &mut io::sink());
                _ = events.send(FixtureEvent::ControllerClosed);
                result
            }
        });
        let mut socket = None;
        let result = (|| -> io::Result<()> {
            socket = Some(accept_or_exit(&listener, &events, &received)?);
            let socket = socket.as_mut().unwrap();
            let ready = read_or_exit(socket, &events, &received, |socket| {
                let mut ready = [0; 5];
                socket.read_exact(&mut ready)?;
                Ok(ready.to_vec())
            })?;
            if ready != b"ready" {
                return Err(io::Error::other("unexpected build-script readiness frame"));
            }
            socket.write_all(b"x")?;
            let waiting = read_or_exit(socket, &events, &received, |socket| {
                let mut waiting = [0; 7];
                socket.read_exact(&mut waiting)?;
                Ok(waiting.to_vec())
            })?;
            if waiting != b"waiting" {
                return Err(io::Error::other("unexpected build-script waiting frame"));
            }
            if process.try_wait()?.is_some() {
                return Err(io::Error::other("controller exited before cancellation"));
            }
            #[cfg(unix)]
            {
                let signal = command(fixture.root.path(), "kill")
                    .args(["-TERM", &process.id().to_string()])
                    .output()?;
                if !signal.status.success() {
                    return Err(io::Error::other("failed to signal the owned controller"));
                }
            }
            #[cfg(windows)]
            control.write_all(b"c")?;
            require_event(received.recv().unwrap(), FixtureEvent::ControllerClosed)?;
            if process.wait()?.success() {
                return Err(io::Error::other("cancelled controller reported success"));
            }
            probe_descendant(socket, &events, &received)
        })();

        let shutdown = if result.is_err() {
            socket.as_ref().map(close_socket).transpose()
        } else {
            Ok(None)
        };
        #[cfg(windows)]
        let aborted = if result.is_err() && process.try_wait().unwrap().is_none() {
            // The launcher owns the actual controller handle; abort never discovers PIDs.
            control.write_all(b"k")
        } else {
            Ok(())
        };
        if result.is_err() {
            #[cfg(windows)]
            {
                // Closing the pipe also aborts if a partially delivered request failed.
                drop(control);
            }
            #[cfg(unix)]
            if process.try_wait().unwrap().is_none() {
                // Only this fixture's unreaped controller group is eligible for forced cleanup.
                let killed = command(fixture.root.path(), "kill")
                    .args(["-KILL", "--", &format!("-{}", process.id())])
                    .output()
                    .unwrap();
                assert!(killed.status.success() || process.try_wait().unwrap().is_some());
            }
        }
        let reaped = process.wait();
        let drained = closed.join().unwrap();
        drop(socket);
        if result.is_err() {
            cleanup_worktrees(&fixture);
        }
        reaped.unwrap();
        drained.unwrap();
        shutdown.unwrap();
        #[cfg(windows)]
        aborted.unwrap();
        result.unwrap();

        let outcomes = fixture.outcomes("out");
        assert_eq!(outcomes[0]["status"], "failed");
        assert_eq!(outcomes[1]["status"], "unattempted");
        assert!(outcomes[0]["cleanup_error"].is_null());
        assert_eq!(
            run(
                fixture.root.path(),
                "git",
                &["worktree", "list", "--porcelain"]
            )
            .matches("worktree ")
            .count(),
            1
        );
    });
}

fn require_event(actual: FixtureEvent, expected: FixtureEvent) -> io::Result<()> {
    match actual {
        event if event == expected => Ok(()),
        FixtureEvent::ControllerClosed => Err(io::Error::new(
            ErrorKind::UnexpectedEof,
            "controller exited before the fixture operation completed",
        )),
        FixtureEvent::Abort => Err(io::Error::new(ErrorKind::Interrupted, "fixture aborted")),
        _ => Err(io::Error::other("unexpected fixture event")),
    }
}

fn accept_or_exit(
    listener: &TcpListener,
    events: &Sender<FixtureEvent>,
    received: &Receiver<FixtureEvent>,
) -> io::Result<TcpStream> {
    let address = listener.local_addr()?;
    let worker_listener = listener.try_clone()?;
    let accepted = thread::spawn({
        let events = events.clone();
        move || {
            let result = worker_listener.accept().map(|(socket, _)| socket);
            _ = events.send(FixtureEvent::Connected);
            result
        }
    });
    let event = received.recv().unwrap();
    if event != FixtureEvent::Connected {
        // Keep the listener owned until this wakeup and join; its port cannot be reused meanwhile.
        drop(TcpStream::connect(address)?);
    }
    let socket = accepted.join().unwrap()?;
    require_event(event, FixtureEvent::Connected)?;
    Ok(socket)
}

fn read_or_exit(
    socket: &TcpStream,
    events: &Sender<FixtureEvent>,
    received: &Receiver<FixtureEvent>,
    read: impl FnOnce(&mut TcpStream) -> io::Result<Vec<u8>> + Send + 'static,
) -> io::Result<Vec<u8>> {
    let mut input = socket.try_clone()?;
    let reader = thread::spawn({
        let events = events.clone();
        move || {
            let result = read(&mut input);
            _ = events.send(FixtureEvent::Read);
            result
        }
    });
    let event = received.recv().unwrap();
    let shutdown = if event != FixtureEvent::Read {
        close_socket(socket)
    } else {
        Ok(())
    };
    let result = reader.join().unwrap();
    shutdown?;
    require_event(event, FixtureEvent::Read)?;
    result
}

fn close_socket(socket: &TcpStream) -> io::Result<()> {
    match socket.shutdown(Shutdown::Both) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotConnected | ErrorKind::ConnectionReset
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn probe_descendant(
    socket: &mut TcpStream,
    events: &Sender<FixtureEvent>,
    received: &Receiver<FixtureEvent>,
) -> io::Result<()> {
    // Only probe after controller exit. Letting the helper finish earlier would mask a missing
    // process-tree kill; an actual survivor must identify itself before its controlled exit.
    match socket.write_all(b"p") {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
            ) =>
        {
            return Ok(());
        }
        Err(error) => return Err(error),
    }
    let response = read_or_exit(socket, events, received, |socket| {
        let mut response = Vec::new();
        match socket.read_to_end(&mut response) {
            Ok(_) => Ok(response),
            Err(error) if error.kind() == ErrorKind::ConnectionReset => Ok(response),
            Err(error) => Err(error),
        }
    })?;
    if response.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(
            "build descendant survived controller cancellation",
        ))
    }
}

fn cleanup_worktrees(fixture: &Fixture) {
    let root = fixture.root.path().canonicalize().unwrap();
    for line in run(
        fixture.root.path(),
        "git",
        &["worktree", "list", "--porcelain"],
    )
    .lines()
    {
        if let Some(path) = line.strip_prefix("worktree ") {
            match Path::new(path).canonicalize() {
                Ok(path) if path != root => {
                    run(
                        fixture.root.path(),
                        "git",
                        &["worktree", "remove", "--force", path.to_str().unwrap()],
                    );
                }
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => panic!("{error}"),
            }
        }
    }
    run(fixture.root.path(), "git", &["worktree", "prune"]);
}

#[test]
fn controller_exit_before_connection_unblocks_the_fixture_listener() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let (events, received) = mpsc::channel();
        let mut controller = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"));
        controller
            .arg("--help")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        let mut process = controller.spawn().unwrap();
        #[cfg(windows)]
        let fixture = Fixture::new();
        #[cfg(windows)]
        let (mut process, _control) = spawn_controller(&fixture, &controller);
        let mut stdout = process.stdout.take().unwrap();
        let closed = thread::spawn({
            let events = events.clone();
            move || {
                io::copy(&mut stdout, &mut io::sink()).unwrap();
                events.send(FixtureEvent::ControllerClosed).unwrap();
            }
        });
        assert_eq!(
            accept_or_exit(&listener, &events, &received)
                .unwrap_err()
                .kind(),
            ErrorKind::UnexpectedEof
        );
        assert!(process.wait().unwrap().success());
        closed.join().unwrap();
    });
}

#[cfg(windows)]
#[test]
fn launcher_abort_reaps_a_controller_that_cannot_finish_by_itself() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let fixture = Fixture::new();
        let tools = fixture.root.path().join("out/abort-fixture");
        compile_tool(
            &tools,
            "blocked-controller",
            r#"
fn main() {
    let _ready = std::net::TcpStream::connect(std::env::var("FIXTURE_READY").unwrap()).unwrap();
    loop { std::thread::park(); }
}
"#,
        );
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut controller = Command::new(tools.join("blocked-controller.exe"));
        controller.env("FIXTURE_READY", listener.local_addr().unwrap().to_string());
        let (mut process, mut control) = spawn_controller(&fixture, &controller);
        let (events, received) = mpsc::channel();
        let mut stdout = process.stdout.take().unwrap();
        let closed = thread::spawn({
            let events = events.clone();
            move || {
                io::copy(&mut stdout, &mut io::sink()).unwrap();
                _ = events.send(FixtureEvent::ControllerClosed);
            }
        });
        let ready = accept_or_exit(&listener, &events, &received);
        let aborted = control.write_all(b"k");
        drop(control);
        assert!(!process.wait().unwrap().success());
        closed.join().unwrap();
        aborted.unwrap();
        let mut ready = ready.unwrap();
        match ready.read(&mut [0]) {
            Ok(length) => assert_eq!(length, 0),
            Err(error) => assert_eq!(error.kind(), ErrorKind::ConnectionReset),
        }
    });
}

#[test]
fn exit_and_abort_events_unblock_incomplete_socket_reads() {
    testing::with_watchdog(|| {
        for event in [FixtureEvent::ControllerClosed, FixtureEvent::Abort] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (socket, _) = listener.accept().unwrap();
            let (events, received) = mpsc::channel();
            if event == FixtureEvent::Abort {
                drop(AbortFixture(events.clone()));
            } else {
                events.send(event).unwrap();
            }
            let error = read_or_exit(&socket, &events, &received, |socket| {
                let mut data = [0; 5];
                socket.read_exact(&mut data)?;
                Ok(data.to_vec())
            })
            .unwrap_err();
            assert_eq!(
                error.kind(),
                if event == FixtureEvent::Abort {
                    ErrorKind::Interrupted
                } else {
                    ErrorKind::UnexpectedEof
                }
            );
            match peer.read(&mut [0]) {
                Ok(length) => assert_eq!(length, 0),
                Err(error) => assert_eq!(error.kind(), ErrorKind::ConnectionReset),
            }
        }
    });
}

#[test]
fn a_surviving_descendant_answers_the_probe_instead_of_hanging() {
    testing::with_watchdog(|| {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut socket, _) = listener.accept().unwrap();
        let peer = thread::spawn(move || {
            let mut request = [0];
            peer.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"p");
            peer.write_all(b"alive").unwrap();
        });
        let (events, received) = mpsc::channel();
        probe_descendant(&mut socket, &events, &received).unwrap_err();
        peer.join().unwrap();
    });
}

// Only a survivor can answer the final probe. EOF/abort exits explicitly so a failed test
// cannot leave this controlled helper waiting after its controller is gone.
const BUILD_SCRIPT: &str = r#"
use std::io::{Read, Write};
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
}
#[cfg(windows)]
unsafe extern "system" fn ignore_console_event(_event: u32) -> i32 { 1 }
fn main() {
    #[cfg(windows)]
    {
        // Ignore console events: only owned process-tree termination can close this connection.
        // SAFETY: The static callback has the Win32 signature and retains no borrowed state.
        let registered = unsafe { SetConsoleCtrlHandler(Some(ignore_console_event), 1) };
        assert_ne!(registered, 0);
    }
    let mut socket = std::net::TcpStream::connect(std::env::var("RELEASE_FIXTURE_SOCKET").unwrap()).unwrap();
    socket.write_all(b"ready").unwrap();
    let mut request = [0];
    if socket.read_exact(&mut request).is_err() { std::process::exit(1); }
    socket.write_all(b"waiting").unwrap();
    if socket.read_exact(&mut request).is_ok() { socket.write_all(b"alive").unwrap(); }
    std::process::exit(1);
}
"#;
