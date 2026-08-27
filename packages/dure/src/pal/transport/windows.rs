//! Windows named-pipe transport.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::PCWSTR;

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::{ConnId, ListenerId};
use crate::pal::raw_handle::RawHandle;
use crate::pal::transport::Transport;
use crate::protocol::{Message, decode_payload, encode, payload_len_ok};

struct PipeTable {
    listeners: HashMap<u64, Listener>,
    conns: HashMap<u64, RawHandle>,
}

struct Listener {
    name: Vec<u16>,
    pending: RawHandle,
}

fn table() -> &'static Mutex<PipeTable> {
    static TABLE: OnceLock<Mutex<PipeTable>> = OnceLock::new();
    TABLE.get_or_init(|| {
        Mutex::new(PipeTable {
            listeners: HashMap::new(),
            conns: HashMap::new(),
        })
    })
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn close(handle: HANDLE) {
    if handle.is_invalid() {
        return;
    }
    // SAFETY: `handle` is a pipe or event handle we own and never use again.
    _ = unsafe { CloseHandle(handle) };
}

fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn create_instance(name: &[u16], first: bool) -> Result<HANDLE, PalError> {
    let mut open_mode = PIPE_ACCESS_DUPLEX.0 | FILE_FLAG_OVERLAPPED.0;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE.0;
    }
    // SAFETY: `name` is a NUL-terminated pipe path. The created handle is owned
    // by the caller. Remote clients are rejected.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(open_mode),
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            65_536,
            65_536,
            0,
            None,
        )
    };
    if handle.is_invalid() {
        return Err(PalError::new(PalErrorKind::Other));
    }
    Ok(handle)
}

fn create_event() -> Result<HANDLE, PalError> {
    // SAFETY: a manual-reset event used only as an overlapped completion event.
    unsafe { CreateEventW(None, true, false, None) }
        .map_err(|_error| PalError::new(PalErrorKind::Other))
}

fn wait_event(event: HANDLE, timeout_ms: u32) -> Result<(), PalError> {
    // SAFETY: `event` is a live event handle created for this overlapped wait.
    let wait = unsafe { WaitForSingleObject(event, timeout_ms) };
    if wait == WAIT_TIMEOUT {
        return Err(PalError::new(PalErrorKind::Timeout));
    }
    if wait != WAIT_OBJECT_0 {
        return Err(PalError::new(PalErrorKind::Other));
    }
    Ok(())
}

fn connect_instance(handle: HANDLE) -> Result<(), PalError> {
    let event = create_event()?;
    let mut overlapped = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    // SAFETY: `handle` is a listening pipe instance. `overlapped` is valid for
    // the duration of this wait.
    let result = unsafe { ConnectNamedPipe(handle, Some(&raw mut overlapped)) };
    if result.is_ok() {
        close(event);
        return Ok(());
    }
    let err = {
        // SAFETY: GetLastError is called immediately after the failed API.
        unsafe { GetLastError() }
    };
    if err == ERROR_PIPE_CONNECTED {
        close(event);
        return Ok(());
    }
    if err != ERROR_IO_PENDING {
        close(event);
        return Err(PalError::new(PalErrorKind::Other));
    }
    let wait = wait_event(event, INFINITE);
    close(event);
    wait
}

fn read_exact(handle: HANDLE, buf: &mut [u8]) -> Result<(), PalError> {
    let mut filled = 0_usize;
    while filled < buf.len() {
        let event = create_event()?;
        let mut overlapped = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let mut transferred = 0_u32;
        let dest = buf
            .get_mut(filled..)
            .ok_or_else(|| PalError::new(PalErrorKind::Other))?;
        // SAFETY: `handle` is a connected overlapped pipe. `dest` is exclusive
        // for the duration of this call.
        let ok = unsafe {
            ReadFile(
                handle,
                Some(dest),
                Some(&raw mut transferred),
                Some(&raw mut overlapped),
            )
        };
        if ok.is_err() {
            let err = {
                // SAFETY: immediately after the failed ReadFile.
                unsafe { GetLastError() }
            };
            if err != ERROR_IO_PENDING {
                close(event);
                return Err(PalError::new(PalErrorKind::Other));
            }
            if let Err(error) = wait_event(event, INFINITE) {
                // SAFETY: cancel the pending read so the OVERLAPPED can drop.
                _ = unsafe { CancelIoEx(handle, Some(&raw const overlapped)) };
                close(event);
                return Err(error);
            }
            // SAFETY: the wait succeeded; `overlapped` still addresses this read.
            if unsafe {
                GetOverlappedResult(handle, &raw const overlapped, &raw mut transferred, false)
            }
            .is_err()
            {
                close(event);
                return Err(PalError::new(PalErrorKind::Other));
            }
        }
        close(event);
        if transferred == 0 {
            return Err(PalError::new(PalErrorKind::Other));
        }
        filled = filled
            .checked_add(transferred as usize)
            .ok_or_else(|| PalError::new(PalErrorKind::Other))?;
    }
    Ok(())
}

fn write_all(handle: HANDLE, mut buf: &[u8]) -> Result<(), PalError> {
    while !buf.is_empty() {
        let event = create_event()?;
        let mut overlapped = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let mut transferred = 0_u32;
        // SAFETY: `handle` is a connected overlapped pipe. `buf` is exclusive
        // for the duration of this call.
        let ok = unsafe {
            WriteFile(
                handle,
                Some(buf),
                Some(&raw mut transferred),
                Some(&raw mut overlapped),
            )
        };
        if ok.is_err() {
            let err = {
                // SAFETY: immediately after the failed WriteFile.
                unsafe { GetLastError() }
            };
            if err != ERROR_IO_PENDING {
                close(event);
                return Err(PalError::new(PalErrorKind::Other));
            }
            if let Err(error) = wait_event(event, INFINITE) {
                // SAFETY: cancel the pending write so the OVERLAPPED can drop.
                _ = unsafe { CancelIoEx(handle, Some(&raw const overlapped)) };
                close(event);
                return Err(error);
            }
            // SAFETY: the wait succeeded; `overlapped` still addresses this write.
            if unsafe {
                GetOverlappedResult(handle, &raw const overlapped, &raw mut transferred, false)
            }
            .is_err()
            {
                close(event);
                return Err(PalError::new(PalErrorKind::Other));
            }
        }
        close(event);
        if transferred == 0 {
            return Err(PalError::new(PalErrorKind::Other));
        }
        buf = buf
            .get(transferred as usize..)
            .ok_or_else(|| PalError::new(PalErrorKind::Other))?;
    }
    Ok(())
}

fn conn_handle(conn: ConnId) -> Result<HANDLE, PalError> {
    table()
        .lock()
        .expect("pipe table")
        .conns
        .get(&conn.0)
        .copied()
        .map(RawHandle::as_handle)
        .ok_or_else(|| PalError::new(PalErrorKind::NotFound))
}

/// Real Windows named-pipe transport.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetTransport;

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl Transport for BuildTargetTransport {
    fn listen(&self, name: &str) -> Result<ListenerId, PalError> {
        let name = wide_z(name);
        let pending = create_instance(&name, true)?;
        let id = next_id();
        table().lock().expect("pipe table").listeners.insert(
            id,
            Listener {
                name,
                pending: RawHandle::from_handle(pending),
            },
        );
        Ok(ListenerId(id))
    }

    fn accept(&self, listener: ListenerId) -> Result<ConnId, PalError> {
        let (pending, name) = {
            let table = table().lock().expect("pipe table");
            let listener = table
                .listeners
                .get(&listener.0)
                .ok_or_else(|| PalError::new(PalErrorKind::NotFound))?;
            (listener.pending, listener.name.clone())
        };
        connect_instance(pending.as_handle())?;
        let next = create_instance(&name, false)?;
        let mut table = table().lock().expect("pipe table");
        if let Some(listener) = table.listeners.get_mut(&listener.0) {
            listener.pending = RawHandle::from_handle(next);
        } else {
            close(next);
        }
        let id = next_id();
        table.conns.insert(id, pending);
        Ok(ConnId(id))
    }

    fn connect(&self, name: &str, timeout: Duration) -> Result<ConnId, PalError> {
        let name = wide_z(name);
        let timeout_ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        let access = FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0;
        // SAFETY: `name` is a NUL-terminated pipe path. WaitNamedPipeW does not
        // retain the pointer after it returns.
        let ready = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), timeout_ms) };
        if !ready.as_bool() {
            return Err(PalError::new(PalErrorKind::Timeout));
        }
        // SAFETY: WaitNamedPipeW reported an instance. CreateFile opens a new
        // client handle we own. FILE_FLAG_OVERLAPPED matches the server end.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                access,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        };
        let Ok(handle) = handle else {
            let err = {
                // SAFETY: immediately after the failed CreateFileW.
                unsafe { GetLastError() }
            };
            if err == ERROR_PIPE_BUSY {
                return Err(PalError::new(PalErrorKind::Timeout));
            }
            return Err(PalError::new(PalErrorKind::NotFound));
        };
        let id = next_id();
        table()
            .lock()
            .expect("pipe table")
            .conns
            .insert(id, RawHandle::from_handle(handle));
        Ok(ConnId(id))
    }

    fn send(&self, conn: ConnId, message: &Message) -> Result<(), PalError> {
        let handle = conn_handle(conn)?;
        write_all(handle, &encode(message))
    }

    fn recv(&self, conn: ConnId) -> Result<Message, PalError> {
        let handle = conn_handle(conn)?;
        let mut header = [0_u8; 4];
        read_exact(handle, &mut header)?;
        let len = u32::from_le_bytes(header);
        if !payload_len_ok(len) {
            return Err(PalError::new(PalErrorKind::Other));
        }
        let mut payload = vec![0_u8; len as usize];
        read_exact(handle, &mut payload)?;
        decode_payload(&payload).map_err(|_error| PalError::new(PalErrorKind::Other))
    }

    fn disconnect(&self, conn: ConnId) {
        if let Some(handle) = table().lock().expect("pipe table").conns.remove(&conn.0) {
            close(handle.as_handle());
        }
    }

    fn close_listener(&self, listener: ListenerId) {
        if let Some(listener) = table()
            .lock()
            .expect("pipe table")
            .listeners
            .remove(&listener.0)
        {
            close(listener.pending.as_handle());
        }
    }

    fn pipe_name(&self, nonce: &str) -> String {
        format!(r"\\.\pipe\dure-{nonce}")
    }
}
