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
        Ok(Message::Attached { .. }) => {}
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
    use crate::pal::local_console::{LocalConsoleFacade, MockLocalConsole};
    use crate::pal::pseudoconsole::WindowSize;
    use crate::pal::transport::MemoryTransport;
    use crate::session_id::SessionId;

    use super::*;

    #[test]
    fn connect_timeout_is_resume_timeout() {
        let transport = MemoryTransport::new();
        let mut console = MockLocalConsole::new();
        console.expect_has_console().return_const(true);
        console.expect_disable_ctrl_c_handler().returning(|| Ok(()));
        console.expect_enter_raw_relay().returning(|| Ok(()));
        console.expect_leave_raw_relay().returning(|| Ok(()));
        console
            .expect_window_size()
            .returning(|| Ok(WindowSize { cols: 80, rows: 24 }));
        let console = LocalConsoleFacade::from_mock(console);
        let id = SessionId::MIN;
        attach(&transport, &console, "missing", id).unwrap_err();
    }
}
