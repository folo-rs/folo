//! Client attach and console relay.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use ohno::AppError;

use crate::constants::CONNECT_TIMEOUT;
use crate::pal::error::PalErrorKind;
use crate::pal::ids::ConnId;
use crate::pal::local_console::{ConsoleInput, LocalConsole};
use crate::pal::transport::Transport;
use crate::protocol::Message;
use crate::session_id::SessionId;
use crate::types::Outcome;
use crate::{
    DisplacedError, NoConsoleError, PalFailedError, RelayFailedError, ResumeTimeoutError,
    StartupFailedError,
};

/// Connect to a live supervisor and funnel console I/O until the relay ends.
pub(crate) fn attach<T, C>(
    transport: &T,
    console: &C,
    pipe_name: &str,
    session_id: SessionId,
) -> Result<Outcome, AppError>
where
    T: Transport + Clone + Send + Sync + 'static,
    C: LocalConsole + Clone + Send + Sync + 'static,
{
    if !console.has_console() {
        return Err(NoConsoleError::new().into());
    }
    // Installed before the first console mutation so a partial failure inside
    // either call below still leaves the user with a cooked console.
    let _raw_guard = RawRelayGuard {
        console: console.clone(),
    };
    console
        .disable_ctrl_c_handler()
        .map_err(|_error| PalFailedError::new())?;
    console
        .enter_raw_relay()
        .map_err(|_error| PalFailedError::new())?;
    let size = console
        .window_size()
        .map_err(|_error| PalFailedError::new())?;

    let conn = match transport.connect(pipe_name, CONNECT_TIMEOUT) {
        Ok(conn) => conn,
        Err(error) if error.kind() == PalErrorKind::Timeout => {
            return Err(ResumeTimeoutError::for_id(session_id).into());
        }
        Err(_) => return Err(StartupFailedError::new().into()),
    };

    transport
        .send(
            conn,
            &Message::Attach {
                cols: size.cols,
                rows: size.rows,
            },
        )
        .map_err(|_error| StartupFailedError::new())?;

    match transport.recv(conn) {
        Ok(Message::Attached {
            session_id: attached_id,
        }) if attached_id == session_id => {}
        Ok(Message::Displaced) => {
            transport.disconnect(conn);
            return Err(DisplacedError::new().into());
        }
        _ => {
            transport.disconnect(conn);
            return Err(StartupFailedError::new().into());
        }
    }

    eprintln!("session {session_id}");
    relay(transport, console, conn)
}

/// Restores cooked console modes when attach ends, including on error paths.
struct RawRelayGuard<C: LocalConsole> {
    console: C,
}

impl<C: LocalConsole> Drop for RawRelayGuard<C> {
    fn drop(&mut self) {
        _ = self.console.leave_raw_relay();
    }
}

// Blocking recv. A mutation that drops the disconnect path hangs unit tests
// because watchdogs are disabled under cargo-mutants.
#[cfg_attr(test, mutants::skip)]
fn relay<T, C>(transport: &T, console: &C, conn: ConnId) -> Result<Outcome, AppError>
where
    T: Transport + Clone + Send + Sync + 'static,
    C: LocalConsole + Clone + Send + Sync + 'static,
{
    let done = Arc::new(AtomicBool::new(false));
    let input_failed = Arc::new(AtomicBool::new(false));

    thread::spawn({
        let transport = transport.clone();
        let console = console.clone();
        let done = Arc::clone(&done);
        let input_failed = Arc::clone(&input_failed);
        move || {
            while !done.load(Ordering::SeqCst) {
                match console.read_input() {
                    Ok(ConsoleInput::Bytes(bytes)) => {
                        if transport.send(conn, &Message::Input(bytes)).is_err() {
                            break;
                        }
                    }
                    Ok(ConsoleInput::Resize(size)) => {
                        if transport
                            .send(
                                conn,
                                &Message::Resize {
                                    cols: size.cols,
                                    rows: size.rows,
                                },
                            )
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => {
                        input_failed.store(true, Ordering::SeqCst);
                        transport.disconnect(conn);
                        break;
                    }
                }
            }
            done.store(true, Ordering::SeqCst);
        }
    });

    loop {
        match transport.recv(conn) {
            Ok(Message::Output(bytes)) => {
                if console.write_output(&bytes).is_err() {
                    done.store(true, Ordering::SeqCst);
                    transport.disconnect(conn);
                    return Err(RelayFailedError::new().into());
                }
            }
            Ok(Message::AppExited { status }) => {
                done.store(true, Ordering::SeqCst);
                transport.disconnect(conn);
                return Ok(Outcome::AppExit(status));
            }
            Ok(Message::Displaced) => {
                done.store(true, Ordering::SeqCst);
                transport.disconnect(conn);
                return Err(DisplacedError::new().into());
            }
            Err(error) if error.kind() == PalErrorKind::Disconnected => {
                done.store(true, Ordering::SeqCst);
                transport.disconnect(conn);
                if input_failed.load(Ordering::SeqCst) {
                    return Err(RelayFailedError::new().into());
                }
                return Ok(Outcome::Success);
            }
            Ok(_) | Err(_) => {
                done.store(true, Ordering::SeqCst);
                transport.disconnect(conn);
                return Err(RelayFailedError::new().into());
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;
    use std::{thread, vec};

    use super::*;
    use crate::durability::Durability;
    use crate::pal::error::{PalError, PalErrorKind};
    use crate::pal::ids::{ConnId, ListenerId};
    use crate::pal::local_console::{LocalConsoleFacade, MockLocalConsole};
    use crate::pal::pseudoconsole::WindowSize;
    use crate::pal::transport::MemoryTransport;
    use crate::protocol::Message;
    use crate::session_id::SessionId;

    const SAMPLE_SIZE: WindowSize = WindowSize { cols: 80, rows: 24 };

    /// Assembles the `LocalConsole` an attach test needs: the happy path by
    /// default, with individual operations overridden to fail and with console
    /// input supplied as a script.
    struct TestConsole {
        has_console: bool,
        disable_ctrl_c_handler: Result<(), PalErrorKind>,
        enter_raw_relay: Result<(), PalErrorKind>,
        window_size: Result<(), PalErrorKind>,
        write_output: Result<(), PalErrorKind>,
        input: Vec<Result<ConsoleInput, PalErrorKind>>,
        leaves: Arc<AtomicUsize>,
    }

    impl TestConsole {
        fn new() -> Self {
            Self {
                has_console: true,
                disable_ctrl_c_handler: Ok(()),
                enter_raw_relay: Ok(()),
                window_size: Ok(()),
                write_output: Ok(()),
                input: Vec::new(),
                leaves: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn build(self) -> LocalConsoleFacade {
            let Self {
                has_console,
                disable_ctrl_c_handler,
                enter_raw_relay,
                window_size,
                write_output,
                input,
                leaves,
            } = self;

            let mut console = MockLocalConsole::new();
            console.expect_has_console().return_const(has_console);
            console
                .expect_disable_ctrl_c_handler()
                .returning(move || disable_ctrl_c_handler.map_err(PalError::new));
            console
                .expect_enter_raw_relay()
                .returning(move || enter_raw_relay.map_err(PalError::new));
            console.expect_leave_raw_relay().returning(move || {
                leaves.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            console
                .expect_window_size()
                .returning(move || window_size.map(|()| SAMPLE_SIZE).map_err(PalError::new));
            console
                .expect_write_output()
                .returning(move |_| write_output.map_err(PalError::new));

            let input = Mutex::new(input.into_iter());
            console.expect_read_input().returning(move || {
                match input.lock().expect("input script lock").next() {
                    Some(result) => result.map_err(PalError::new),
                    // A real console blocks while the user is idle; parking keeps the
                    // relay's input thread out of the way without spinning.
                    None => loop {
                        thread::park();
                    },
                }
            });

            LocalConsoleFacade::from_mock(console)
        }
    }

    /// Transport whose every operation fails, with a configurable `connect` kind.
    ///
    /// Exercises the attach paths that precede a working connection.
    #[derive(Clone, Debug)]
    struct ConnectFails(PalErrorKind);

    impl Transport for ConnectFails {
        fn listen(&self, _name: &str) -> Result<ListenerId, PalError> {
            Err(PalError::new(PalErrorKind::Other))
        }

        fn accept(&self, _listener: ListenerId) -> Result<ConnId, PalError> {
            Err(PalError::new(PalErrorKind::Other))
        }

        fn connect(&self, _name: &str, _timeout: Duration) -> Result<ConnId, PalError> {
            Err(PalError::new(self.0))
        }

        fn send(&self, _conn: ConnId, _message: &Message) -> Result<(), PalError> {
            Err(PalError::new(PalErrorKind::Other))
        }

        fn recv(&self, _conn: ConnId) -> Result<Message, PalError> {
            Err(PalError::new(PalErrorKind::Other))
        }

        fn disconnect(&self, _conn: ConnId) {}

        fn close_listener(&self, _listener: ListenerId) {}

        fn pipe_name(&self, nonce: &str) -> String {
            nonce.to_string()
        }
    }

    /// Transport that connects but cannot send, so the handshake fails on the
    /// `Attach` message rather than on the connection itself.
    #[derive(Clone, Debug)]
    struct SendFails;

    impl Transport for SendFails {
        fn listen(&self, _name: &str) -> Result<ListenerId, PalError> {
            Err(PalError::new(PalErrorKind::Other))
        }

        fn accept(&self, _listener: ListenerId) -> Result<ConnId, PalError> {
            Err(PalError::new(PalErrorKind::Other))
        }

        fn connect(&self, _name: &str, _timeout: Duration) -> Result<ConnId, PalError> {
            Ok(ConnId(1))
        }

        fn send(&self, _conn: ConnId, _message: &Message) -> Result<(), PalError> {
            Err(PalError::new(PalErrorKind::Other))
        }

        fn recv(&self, _conn: ConnId) -> Result<Message, PalError> {
            Err(PalError::new(PalErrorKind::Other))
        }

        fn disconnect(&self, _conn: ConnId) {}

        fn close_listener(&self, _listener: ListenerId) {}

        fn pipe_name(&self, nonce: &str) -> String {
            nonce.to_string()
        }
    }

    /// Runs `attach` against a supervisor stand-in that completes the handshake
    /// and then hands the live connection to `serve`.
    fn attach_to_scripted_supervisor<F>(console: TestConsole, serve: F) -> Result<Outcome, AppError>
    where
        F: FnOnce(&MemoryTransport, ConnId) + Send + 'static,
    {
        let transport = MemoryTransport::new();
        let listener = transport.listen("pipe").expect("listen");
        let id = SessionId::MIN;
        thread::spawn({
            let transport = transport.clone();
            move || {
                let conn = transport.accept(listener).expect("accept");
                _ = transport.recv(conn);
                _ = transport.send(conn, &Message::Attached { session_id: id });
                serve(&transport, conn);
            }
        });
        attach(&transport, &console.build(), "pipe", id)
    }

    #[test]
    fn without_a_console_attach_is_refused() {
        let error = attach(
            &ConnectFails(PalErrorKind::Other),
            &TestConsole {
                has_console: false,
                ..TestConsole::new()
            }
            .build(),
            "pipe",
            SessionId::MIN,
        )
        .unwrap_err();
        assert!(error.find_source::<NoConsoleError>().is_some());
    }

    #[test]
    fn ctrl_c_handler_failure_is_pal_failure() {
        let error = attach(
            &ConnectFails(PalErrorKind::Other),
            &TestConsole {
                disable_ctrl_c_handler: Err(PalErrorKind::Other),
                ..TestConsole::new()
            }
            .build(),
            "pipe",
            SessionId::MIN,
        )
        .unwrap_err();
        assert!(error.find_source::<PalFailedError>().is_some());
    }

    #[test]
    fn raw_relay_failure_is_pal_failure() {
        let error = attach(
            &ConnectFails(PalErrorKind::Other),
            &TestConsole {
                enter_raw_relay: Err(PalErrorKind::Other),
                ..TestConsole::new()
            }
            .build(),
            "pipe",
            SessionId::MIN,
        )
        .unwrap_err();
        assert!(error.find_source::<PalFailedError>().is_some());
    }

    #[test]
    fn window_size_failure_is_pal_failure() {
        let error = attach(
            &ConnectFails(PalErrorKind::Other),
            &TestConsole {
                window_size: Err(PalErrorKind::Other),
                ..TestConsole::new()
            }
            .build(),
            "pipe",
            SessionId::MIN,
        )
        .unwrap_err();
        assert!(error.find_source::<PalFailedError>().is_some());
    }

    #[test]
    fn handshake_send_failure_is_startup_failure() {
        let error = attach(
            &SendFails,
            &TestConsole::new().build(),
            "pipe",
            SessionId::MIN,
        )
        .unwrap_err();
        assert!(error.find_source::<StartupFailedError>().is_some());
    }

    #[test]
    // Spawns threads that outlive the assertion: the relay's console-input
    // thread blocks until the process exits, which Miri reports as a leak.
    #[cfg_attr(miri, ignore)]
    fn attached_id_mismatch_is_startup_failure() {
        testing::with_watchdog(|| {
            let transport = MemoryTransport::new();
            let listener = transport.listen("pipe").unwrap();
            // Built here rather than inside the thread: a panic in a spawned
            // thread would leave `attach` blocked in `recv` forever.
            let other = SessionId::from_u32(2).expect("positive id");
            thread::spawn({
                let transport = transport.clone();
                move || {
                    let conn = transport.accept(listener).unwrap();
                    _ = transport.recv(conn);
                    _ = transport.send(conn, &Message::Attached { session_id: other });
                    transport.disconnect(conn);
                }
            });
            let error = attach(
                &transport,
                &TestConsole::new().build(),
                "pipe",
                SessionId::MIN,
            )
            .unwrap_err();
            assert!(error.find_source::<StartupFailedError>().is_some());
        });
    }

    #[test]
    // Spawns threads that outlive the assertion: the relay's console-input
    // thread blocks until the process exits, which Miri reports as a leak.
    #[cfg_attr(miri, ignore)]
    fn matching_attached_then_app_exit_is_success() {
        testing::with_watchdog(|| {
            let outcome = attach_to_scripted_supervisor(TestConsole::new(), |transport, conn| {
                _ = transport.send(conn, &Message::AppExited { status: 3 });
            })
            .unwrap();
            assert!(matches!(outcome, Outcome::AppExit(3)));
        });
    }

    #[test]
    // Spawns threads that outlive the assertion: the relay's console-input
    // thread blocks until the process exits, which Miri reports as a leak.
    #[cfg_attr(miri, ignore)]
    fn displaced_handshake_is_displaced() {
        testing::with_watchdog(|| {
            let transport = MemoryTransport::new();
            let listener = transport.listen("pipe").unwrap();
            thread::spawn({
                let transport = transport.clone();
                move || {
                    let conn = transport.accept(listener).unwrap();
                    _ = transport.recv(conn);
                    _ = transport.send(conn, &Message::Displaced);
                    transport.disconnect(conn);
                }
            });
            let error = attach(
                &transport,
                &TestConsole::new().build(),
                "pipe",
                SessionId::MIN,
            )
            .unwrap_err();
            assert!(error.find_source::<DisplacedError>().is_some());
        });
    }

    #[test]
    fn connect_timeout_is_resume_timeout() {
        let error = attach(
            &ConnectFails(PalErrorKind::Timeout),
            &TestConsole::new().build(),
            "missing",
            SessionId::MIN,
        )
        .unwrap_err();
        assert!(error.find_source::<ResumeTimeoutError>().is_some());
    }

    #[test]
    fn connect_other_is_startup_failure() {
        let error = attach(
            &ConnectFails(PalErrorKind::Other),
            &TestConsole::new().build(),
            "missing",
            SessionId::MIN,
        )
        .unwrap_err();
        assert!(error.find_source::<StartupFailedError>().is_some());
    }

    #[test]
    fn raw_relay_is_left_when_attach_fails() {
        let leaves = Arc::new(AtomicUsize::new(0));
        attach(
            &ConnectFails(PalErrorKind::Other),
            &TestConsole {
                leaves: Arc::clone(&leaves),
                ..TestConsole::new()
            }
            .build(),
            "missing",
            SessionId::MIN,
        )
        .unwrap_err();
        assert_eq!(leaves.load(Ordering::SeqCst), 1);
    }

    #[test]
    // Spawns threads that outlive the assertion: the relay's console-input
    // thread blocks until the process exits, which Miri reports as a leak.
    #[cfg_attr(miri, ignore)]
    fn console_input_is_forwarded_to_the_supervisor() {
        testing::with_watchdog(|| {
            let resize = WindowSize { cols: 10, rows: 20 };
            let console = TestConsole {
                input: vec![
                    Ok(ConsoleInput::Bytes(b"hi".to_vec())),
                    Ok(ConsoleInput::Resize(resize)),
                ],
                ..TestConsole::new()
            };
            let outcome = attach_to_scripted_supervisor(console, move |transport, conn| {
                assert!(matches!(
                    transport.recv(conn),
                    Ok(Message::Input(bytes)) if bytes == b"hi"
                ));
                assert!(matches!(
                    transport.recv(conn),
                    Ok(Message::Resize { cols, rows }) if cols == resize.cols && rows == resize.rows
                ));
                _ = transport.send(conn, &Message::AppExited { status: 0 });
            })
            .unwrap();
            assert!(matches!(outcome, Outcome::AppExit(0)));
        });
    }

    #[test]
    // Spawns threads that outlive the assertion: the relay's console-input
    // thread blocks until the process exits, which Miri reports as a leak.
    #[cfg_attr(miri, ignore)]
    fn console_input_failure_makes_the_relay_fail() {
        testing::with_watchdog(|| {
            let console = TestConsole {
                input: vec![Err(PalErrorKind::Other)],
                ..TestConsole::new()
            };
            // The input thread disconnects, so the output loop sees the peer close
            // without an `AppExited` and must not report success.
            let error = attach_to_scripted_supervisor(console, |_transport, _conn| {}).unwrap_err();
            assert!(error.find_source::<RelayFailedError>().is_some());
        });
    }

    #[test]
    // Spawns threads that outlive the assertion: the relay's console-input
    // thread blocks until the process exits, which Miri reports as a leak.
    #[cfg_attr(miri, ignore)]
    fn output_write_failure_is_relay_failure() {
        testing::with_watchdog(|| {
            let console = TestConsole {
                write_output: Err(PalErrorKind::Other),
                ..TestConsole::new()
            };
            let error = attach_to_scripted_supervisor(console, |transport, conn| {
                _ = transport.send(conn, &Message::Output(b"out".to_vec()));
            })
            .unwrap_err();
            assert!(error.find_source::<RelayFailedError>().is_some());
        });
    }

    #[test]
    // Spawns threads that outlive the assertion: the relay's console-input
    // thread blocks until the process exits, which Miri reports as a leak.
    #[cfg_attr(miri, ignore)]
    fn output_is_written_until_the_supervisor_disconnects() {
        testing::with_watchdog(|| {
            let outcome = attach_to_scripted_supervisor(TestConsole::new(), |transport, conn| {
                _ = transport.send(conn, &Message::Output(b"out".to_vec()));
                transport.disconnect(conn);
            })
            .unwrap();
            assert!(matches!(outcome, Outcome::Success));
        });
    }

    #[test]
    // Spawns threads that outlive the assertion: the relay's console-input
    // thread blocks until the process exits, which Miri reports as a leak.
    #[cfg_attr(miri, ignore)]
    fn displacement_during_relay_is_displaced() {
        testing::with_watchdog(|| {
            let error = attach_to_scripted_supervisor(TestConsole::new(), |transport, conn| {
                _ = transport.send(conn, &Message::Displaced);
            })
            .unwrap_err();
            assert!(error.find_source::<DisplacedError>().is_some());
        });
    }

    #[test]
    // Spawns threads that outlive the assertion: the relay's console-input
    // thread blocks until the process exits, which Miri reports as a leak.
    #[cfg_attr(miri, ignore)]
    fn unexpected_relay_message_is_relay_failure() {
        testing::with_watchdog(|| {
            let error = attach_to_scripted_supervisor(TestConsole::new(), |transport, conn| {
                _ = transport.send(
                    conn,
                    &Message::StartupOk {
                        session_id: SessionId::MIN,
                        durability: Durability::Durable,
                    },
                );
            })
            .unwrap_err();
            assert!(error.find_source::<RelayFailedError>().is_some());
        });
    }
}
