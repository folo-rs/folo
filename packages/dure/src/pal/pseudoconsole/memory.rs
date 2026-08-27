//! In-memory pseudoconsole for unit tests.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::pal::error::{PalError, PalErrorKind};
use crate::pal::ids::PtyId;
use crate::pal::pseudoconsole::{Pseudoconsole, WindowSize};

struct PtyState {
    size: WindowSize,
    input: VecDeque<u8>,
    output: VecDeque<u8>,
    closed: bool,
}

struct Inner {
    next_id: AtomicU64,
    ptys: Mutex<HashMap<PtyId, PtyState>>,
    cond: Condvar,
}

/// Byte-pump stand-in for a pseudoconsole.
#[derive(Clone)]
pub(crate) struct MemoryPseudoconsole {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for MemoryPseudoconsole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryPseudoconsole").finish()
    }
}

impl MemoryPseudoconsole {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                next_id: AtomicU64::new(1),
                ptys: Mutex::new(HashMap::new()),
                cond: Condvar::new(),
            }),
        }
    }

    /// Push output as if the app wrote to its console.
    pub(crate) fn push_output(&self, pty: PtyId, data: &[u8]) {
        let mut ptys = self.inner.ptys.lock().expect("pty map lock");
        if let Some(state) = ptys.get_mut(&pty) {
            state.output.extend(data.iter().copied());
            self.inner.cond.notify_all();
        }
    }

    /// Take input the supervisor wrote to the app.
    pub(crate) fn take_input(&self, pty: PtyId) -> Vec<u8> {
        let mut ptys = self.inner.ptys.lock().expect("pty map lock");
        ptys.get_mut(&pty)
            .map(|state| state.input.drain(..).collect())
            .unwrap_or_default()
    }

    /// Current size last applied to this pty.
    pub(crate) fn size(&self, pty: PtyId) -> Option<WindowSize> {
        let ptys = self.inner.ptys.lock().expect("pty map lock");
        ptys.get(&pty).map(|state| state.size)
    }
}

impl Default for MemoryPseudoconsole {
    fn default() -> Self {
        Self::new()
    }
}

impl Pseudoconsole for MemoryPseudoconsole {
    fn create(&self, size: WindowSize) -> Result<PtyId, PalError> {
        let id = PtyId(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        self.inner.ptys.lock().expect("pty map lock").insert(
            id,
            PtyState {
                size,
                input: VecDeque::new(),
                output: VecDeque::new(),
                closed: false,
            },
        );
        Ok(id)
    }

    fn resize(&self, pty: PtyId, size: WindowSize) -> Result<(), PalError> {
        let mut ptys = self.inner.ptys.lock().expect("pty map lock");
        let state = ptys
            .get_mut(&pty)
            .ok_or_else(|| PalError::new(PalErrorKind::NotFound))?;
        state.size = size;
        Ok(())
    }

    fn write_input(&self, pty: PtyId, data: &[u8]) -> Result<(), PalError> {
        let mut ptys = self.inner.ptys.lock().expect("pty map lock");
        let state = ptys
            .get_mut(&pty)
            .ok_or_else(|| PalError::new(PalErrorKind::NotFound))?;
        if state.closed {
            return Err(PalError::new(PalErrorKind::NotFound));
        }
        state.input.extend(data.iter().copied());
        self.inner.cond.notify_all();
        Ok(())
    }

    // Blocking condvar wait. A mutation that drops the closed check or the
    // wake hangs tests because watchdogs are disabled under cargo-mutants.
    #[cfg_attr(test, mutants::skip)]
    fn read_output(&self, pty: PtyId) -> Result<Vec<u8>, PalError> {
        let mut ptys = self.inner.ptys.lock().expect("pty map lock");
        loop {
            let Some(state) = ptys.get_mut(&pty) else {
                return Err(PalError::new(PalErrorKind::NotFound));
            };
            if !state.output.is_empty() {
                return Ok(state.output.drain(..).collect());
            }
            if state.closed {
                return Err(PalError::new(PalErrorKind::NotFound));
            }
            ptys = self.inner.cond.wait(ptys).expect("pty condvar");
        }
    }

    fn close(&self, pty: PtyId) {
        let mut ptys = self.inner.ptys.lock().expect("pty map lock");
        if let Some(state) = ptys.get_mut(&pty) {
            state.closed = true;
        }
        self.inner.cond.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pumps_bytes_and_tracks_size() {
        let host = MemoryPseudoconsole::new();
        let pty = host.create(WindowSize { cols: 80, rows: 24 }).unwrap();
        host.write_input(pty, b"in").unwrap();
        assert_eq!(host.take_input(pty), b"in");
        host.resize(pty, WindowSize { cols: 40, rows: 10 }).unwrap();
        assert_eq!(host.size(pty), Some(WindowSize { cols: 40, rows: 10 }));
        host.push_output(pty, b"out");
        assert_eq!(host.read_output(pty).unwrap(), b"out");
        host.close(pty);
    }
}
