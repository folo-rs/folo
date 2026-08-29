//! Windows `ConPTY` pseudoconsole PAL.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows::Win32::System::Pipes::CreatePipe;

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::PtyId;
use crate::pal::pseudoconsole::{Pseudoconsole, WindowSize};
use crate::pal::raw_handle::PipeHandle;

/// One live pseudoconsole and the host ends of its pipes.
///
/// The pipe ends are shared owners because `read_output` and `write_input`
/// release the table lock before their blocking I/O, so a concurrent `close`
/// must not free a handle they are still using.
/// Ref: docs/implementation.md, "Pseudoconsole".
struct Pty {
    hpcon: HPCON,
    host_input: Arc<PipeHandle>,
    host_output: Arc<PipeHandle>,
}

struct PtyTable {
    ptys: HashMap<u64, Pty>,
}

fn table() -> &'static Mutex<PtyTable> {
    static TABLE: OnceLock<Mutex<PtyTable>> = OnceLock::new();
    TABLE.get_or_init(|| {
        Mutex::new(PtyTable {
            ptys: HashMap::new(),
        })
    })
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn close_handle(handle: HANDLE) {
    if handle.is_invalid() {
        return;
    }
    // SAFETY: `handle` is a pipe handle we own and never use again.
    _ = unsafe { CloseHandle(handle) };
}

fn to_coord(size: WindowSize) -> Result<COORD, PalError> {
    Ok(COORD {
        X: i16::try_from(size.cols.max(1)).map_err(|_error| PalError::new(PalErrorKind::Other))?,
        Y: i16::try_from(size.rows.max(1)).map_err(|_error| PalError::new(PalErrorKind::Other))?,
    })
}

/// Live HPCON for `spawn_app` attribute-list wiring.
pub(crate) fn hpcon_for(pty: PtyId) -> Option<HPCON> {
    table()
        .lock()
        .expect("pty table")
        .ptys
        .get(&pty.0)
        .map(|pty| pty.hpcon)
}

/// Close the pseudoconsole, leaving a pending read to finish on its own.
///
/// `close` cancels pending reads, which discards output the app wrote but the
/// reader has not picked up yet. A caller draining the output of an app that has
/// already exited wants those last bytes, and then the end of the stream that
/// closing the host side produces on its own.
#[cfg(feature = "private-test-util")]
pub(crate) fn close_without_cancel(pty: PtyId) {
    let Some(entry) = table().lock().expect("pty table").ptys.remove(&pty.0) else {
        return;
    };
    // SAFETY: `entry.hpcon` is the unique HPCON created for this pty and is not
    // used after this call.
    unsafe {
        ClosePseudoConsole(entry.hpcon);
    }
}

/// Real Windows `ConPTY` host.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetPseudoconsole;

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
impl Pseudoconsole for BuildTargetPseudoconsole {
    fn create(&self, size: WindowSize) -> Result<PtyId, PalError> {
        let mut input_read = HANDLE::default();
        let mut input_write = HANDLE::default();
        let mut output_read = HANDLE::default();
        let mut output_write = HANDLE::default();
        // SAFETY: the four HANDLE slots are stack values. Inherit handles are
        // not requested; ConPTY duplicates the ends it needs.
        unsafe { CreatePipe(&raw mut input_read, &raw mut input_write, None, 0) }
            .map_err(|_error| PalError::new(PalErrorKind::Other))?;
        // SAFETY: same as the input pipe pair; output ends are distinct stack
        // HANDLEs owned by this function until stored or closed.
        if unsafe { CreatePipe(&raw mut output_read, &raw mut output_write, None, 0) }.is_err() {
            close_handle(input_read);
            close_handle(input_write);
            return Err(PalError::new(PalErrorKind::Other));
        }
        let coord = match to_coord(size) {
            Ok(coord) => coord,
            Err(error) => {
                close_handle(input_read);
                close_handle(input_write);
                close_handle(output_read);
                close_handle(output_write);
                return Err(error);
            }
        };
        // SAFETY: `input_read` and `output_write` are the ConPTY ends of pipes
        // we just created. They are valid until ClosePseudoConsole. `coord` is
        // a positive console size.
        let hpcon = unsafe { CreatePseudoConsole(coord, input_read, output_write, 0) };
        let Ok(hpcon) = hpcon else {
            close_handle(input_read);
            close_handle(input_write);
            close_handle(output_read);
            close_handle(output_write);
            return Err(PalError::new(PalErrorKind::Other));
        };
        close_handle(input_read);
        close_handle(output_write);
        let id = next_id();
        table().lock().expect("pty table").ptys.insert(
            id,
            Pty {
                hpcon,
                host_input: PipeHandle::new(input_write),
                host_output: PipeHandle::new(output_read),
            },
        );
        Ok(PtyId(id))
    }

    fn resize(&self, pty: PtyId, size: WindowSize) -> Result<(), PalError> {
        let coord = to_coord(size)?;
        let table = table().lock().expect("pty table");
        let hpcon = table
            .ptys
            .get(&pty.0)
            .map(|pty| pty.hpcon)
            .ok_or_else(|| PalError::new(PalErrorKind::NotFound))?;
        // SAFETY: `hpcon` is borrowed from the table entry for `pty`. The guard
        // is held for this nonblocking call so `close` cannot free it first.
        unsafe { ResizePseudoConsole(hpcon, coord) }
            .map_err(|_error| PalError::new(PalErrorKind::Other))
    }

    fn write_input(&self, pty: PtyId, data: &[u8]) -> Result<(), PalError> {
        // Cloned out of the table so a concurrent `close` cannot free the
        // handle while the write below is blocked on it.
        let handle = table()
            .lock()
            .expect("pty table")
            .ptys
            .get(&pty.0)
            .map(|pty| Arc::clone(&pty.host_input))
            .ok_or_else(|| PalError::new(PalErrorKind::NotFound))?;
        let mut remaining = data;
        while !remaining.is_empty() {
            let mut transferred = 0_u32;
            // SAFETY: `handle` is the host input pipe for a pty and this
            // reference keeps it open across the call; `remaining` is exclusive
            // for this call. Closing this handle is reserved for `close`, so
            // detach never sends EOF.
            unsafe {
                WriteFile(
                    handle.as_handle(),
                    Some(remaining),
                    Some(&raw mut transferred),
                    None,
                )
            }
            .map_err(|_error| PalError::new(PalErrorKind::Other))?;
            if transferred == 0 {
                return Err(PalError::new(PalErrorKind::Other));
            }
            remaining = remaining
                .get(transferred as usize..)
                .ok_or_else(|| PalError::new(PalErrorKind::Other))?;
        }
        Ok(())
    }

    fn read_output(&self, pty: PtyId) -> Result<Vec<u8>, PalError> {
        // Cloned out of the table so a concurrent `close` cannot free the
        // handle while the read below is blocked on it.
        let handle = table()
            .lock()
            .expect("pty table")
            .ptys
            .get(&pty.0)
            .map(|pty| Arc::clone(&pty.host_output))
            .ok_or_else(|| PalError::new(PalErrorKind::NotFound))?;
        let mut buf = vec![0_u8; 4096];
        let mut transferred = 0_u32;
        // SAFETY: `handle` is the host output pipe for a pty and this reference
        // keeps it open across the call; `buf` is exclusive for this call.
        unsafe {
            ReadFile(
                handle.as_handle(),
                Some(buf.as_mut_slice()),
                Some(&raw mut transferred),
                None,
            )
        }
        .map_err(|_error| PalError::new(PalErrorKind::Other))?;
        buf.truncate(transferred as usize);
        Ok(buf)
    }

    fn close(&self, pty: PtyId) {
        let Some(entry) = table().lock().expect("pty table").ptys.remove(&pty.0) else {
            return;
        };
        // SAFETY: `entry.hpcon` is the unique HPCON created for this pty and is
        // not used after this call.
        unsafe {
            ClosePseudoConsole(entry.hpcon);
        }
        // A relay thread may be blocked reading or writing these handles.
        // Cancelling releases it; the handles close once it drops its
        // references, which is what keeps this from freeing a handle in use.
        entry.host_input.cancel();
        entry.host_output.cancel();
    }
}
