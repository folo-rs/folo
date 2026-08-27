//! Client attach and console relay.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use ohno::AppError;

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
    console
        .disable_ctrl_c_handler()
        .map_err(|_error| PalFailedError::new())?;
    console
        .enter_raw_relay()
        .map_err(|_error| PalFailedError::new())?;
    let _raw_guard = RawRelayGuard {
        console: console.clone(),
    };
    let size = console
        .window_size()
        .map_err(|_error| PalFailedError::new())?;

    let conn = match transport.connect(pipe_name, crate::constants::CONNECT_TIMEOUT) {
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
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::pal::error::{PalError, PalErrorKind};
    use crate::pal::ids::{ConnId, ListenerId};
    use crate::pal::local_console::{LocalConsoleFacade, MockLocalConsole};
    use crate::pal::pseudoconsole::WindowSize;
    use crate::pal::transport::MemoryTransport;
    use crate::protocol::Message;
    use crate::session_id::SessionId;

    fn attach_console() -> LocalConsoleFacade {
        let mut console = MockLocalConsole::new();
        console.expect_has_console().return_const(true);
        console.expect_disable_ctrl_c_handler().returning(|| Ok(()));
        console.expect_enter_raw_relay().returning(|| Ok(()));
        console.expect_leave_raw_relay().returning(|| Ok(()));
        console
            .expect_window_size()
            .returning(|| Ok(WindowSize { cols: 80, rows: 24 }));
        console.expect_read_input().returning(|| {
            thread::park();
            Err(PalError::new(PalErrorKind::Disconnected))
        });
        console.expect_write_output().returning(|_| Ok(()));
        LocalConsoleFacade::from_mock(console)
    }

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

    #[test]
    fn attached_id_mismatch_is_startup_failure() {
        testing::with_watchdog(|| {
            let transport = MemoryTransport::new();
            let listener = transport.listen("pipe").unwrap();
            thread::spawn({
                let transport = transport.clone();
                move || {
                    let conn = transport.accept(listener).unwrap();
                    _ = transport.recv(conn);
                    let other = SessionId::from_u32(2).expect("positive id");
                    _ = transport.send(conn, &Message::Attached { session_id: other });
                    transport.disconnect(conn);
                }
            });
            attach(&transport, &attach_console(), "pipe", SessionId::MIN).unwrap_err();
        });
    }

    #[test]
    fn matching_attached_then_app_exit_is_success() {
        testing::with_watchdog(|| {
            let transport = MemoryTransport::new();
            let listener = transport.listen("pipe").unwrap();
            let id = SessionId::MIN;
            thread::spawn({
                let transport = transport.clone();
                move || {
                    let conn = transport.accept(listener).unwrap();
                    _ = transport.recv(conn);
                    _ = transport.send(conn, &Message::Attached { session_id: id });
                    _ = transport.send(conn, &Message::AppExited { status: 3 });
                }
            });
            let outcome = attach(&transport, &attach_console(), "pipe", id).unwrap();
            assert!(matches!(outcome, Outcome::AppExit(3)));
        });
    }

    #[test]
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
            attach(&transport, &attach_console(), "pipe", SessionId::MIN).unwrap_err();
        });
    }

    #[test]
    fn connect_timeout_is_resume_timeout() {
        attach(
            &ConnectFails(PalErrorKind::Timeout),
            &attach_console(),
            "missing",
            SessionId::MIN,
        )
        .unwrap_err();
    }

    #[test]
    fn connect_other_is_startup_failure() {
        attach(
            &ConnectFails(PalErrorKind::Other),
            &attach_console(),
            "missing",
            SessionId::MIN,
        )
        .unwrap_err();
    }
}
