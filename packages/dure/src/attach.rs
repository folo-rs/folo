//! Client attach and console relay.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use ohno::AppError;

use crate::pal::error::PalErrorKind;
use crate::pal::ids::ConnId;
use crate::pal::local_console::LocalConsole;
use crate::pal::transport::Transport;
use crate::protocol::Message;
use crate::session_id::SessionId;
use crate::types::Outcome;
use crate::{
    DisplacedError, NoConsoleError, PalFailedError, ResumeTimeoutError, StartupFailedError,
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

fn relay<T, C>(transport: &T, console: &C, conn: ConnId) -> Result<Outcome, AppError>
where
    T: Transport + Clone + Send + Sync + 'static,
    C: LocalConsole + Clone + Send + Sync + 'static,
{
    let done = Arc::new(AtomicBool::new(false));
    let outcome = Arc::new(std::sync::Mutex::new(None::<Result<Outcome, AppError>>));

    thread::spawn({
        let transport = transport.clone();
        let console = console.clone();
        let done = Arc::clone(&done);
        let outcome = Arc::clone(&outcome);
        move || {
            while !done.load(Ordering::SeqCst) {
                let Ok(bytes) = console.read_input() else {
                    break;
                };
                if transport.send(conn, &Message::Input(bytes)).is_err() {
                    break;
                }
            }
            done.store(true, Ordering::SeqCst);
            let mut slot = outcome.lock().expect("outcome slot");
            if slot.is_none() {
                *slot = Some(Ok(Outcome::Success));
            }
        }
    });

    loop {
        match transport.recv(conn) {
            Ok(Message::Output(bytes)) => {
                _ = console.write_output(&bytes);
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
            _ => {
                done.store(true, Ordering::SeqCst);
                transport.disconnect(conn);
                return Ok(Outcome::Success);
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
        console
            .expect_window_size()
            .returning(|| Ok(WindowSize { cols: 80, rows: 24 }));
        let console = LocalConsoleFacade::from_mock(console);
        let id = SessionId::MIN;
        attach(&transport, &console, "missing", id).unwrap_err();
    }
}
