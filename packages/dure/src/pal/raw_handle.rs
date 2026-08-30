//! Windows handle wrappers shared by the PAL implementations.

use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::IO::CancelIoEx;

/// Process-local handle stored as an integer.
///
/// `HANDLE` is a raw pointer and therefore `!Send`. Windows handles are
/// pointer-sized integers; storing that integer lets a mutex table be shared
/// across the supervisor's relay threads. Closing remains the table owner's
/// job; this type does not take ownership on `Drop`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RawHandle(isize);

impl RawHandle {
    pub(crate) fn from_handle(handle: HANDLE) -> Self {
        Self(handle.0 as isize)
    }

    pub(crate) fn as_handle(self) -> HANDLE {
        HANDLE(self.0 as *mut core::ffi::c_void)
    }
}

/// Shared owner of one pipe handle.
///
/// PAL tables hand out handles and release the table lock before doing I/O on
/// them, so closing one on teardown could invalidate a handle another thread is
/// still operating on, or let Windows reuse the value for an unrelated object.
/// Ownership is shared instead: teardown cancels the I/O outstanding on the
/// handle and drops the table's reference, and the handle is closed once the
/// last in-flight operation releases it.
/// Ref: docs/implementation.md, "Transport" and "Pseudoconsole".
pub(crate) struct PipeHandle {
    handle: RawHandle,
    /// Whether teardown has cancelled this handle.
    ///
    /// Also serialises starting an operation against cancelling one, which is
    /// what makes `cancel` reliable; see `issue`.
    cancelled: Mutex<bool>,
}

impl PipeHandle {
    pub(crate) fn new(handle: HANDLE) -> Arc<Self> {
        Arc::new(Self {
            handle: RawHandle::from_handle(handle),
            cancelled: Mutex::new(false),
        })
    }

    pub(crate) fn as_handle(&self) -> HANDLE {
        self.handle.as_handle()
    }

    /// Starts an overlapped operation, unless this handle is already cancelled.
    ///
    /// `CancelIoEx` only cancels operations that are already pending, so a
    /// caller that started one after teardown had cancelled would wait with
    /// nothing left to release it. Starting and cancelling therefore exclude
    /// each other: an operation either becomes pending before `cancel` runs, and
    /// is cancelled by it, or is refused outright, reported here as `None`.
    ///
    /// `start` must only issue the operation. Waiting for it belongs outside,
    /// once the caller has the result.
    pub(crate) fn issue<T>(&self, start: impl FnOnce(HANDLE) -> T) -> Option<T> {
        let cancelled = self.cancelled.lock().expect("pipe cancellation state");
        if *cancelled {
            return None;
        }
        Some(start(self.as_handle()))
    }

    /// Abort the I/O outstanding on this handle so blocked operations return.
    ///
    /// Operations started later are refused rather than cancelled, because this
    /// is the last cancellation the handle receives: teardown has already
    /// dropped the table's reference, so nothing can reach the handle to cancel
    /// it again.
    pub(crate) fn cancel(&self) {
        let mut cancelled = self.cancelled.lock().expect("pipe cancellation state");
        *cancelled = true;
        // SAFETY: `self` owns the handle and keeps it alive across this call. A
        // null OVERLAPPED cancels every operation this process has pending on
        // the handle, so a blocked read or write completes with an aborted
        // status instead of waiting forever.
        _ = unsafe { CancelIoEx(self.as_handle(), None) };
    }
}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        let handle = self.as_handle();
        if handle.is_invalid() {
            return;
        }
        // SAFETY: this is the last reference to a handle we own, so nothing
        // uses it again.
        _ = unsafe { CloseHandle(handle) };
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_handle_value() {
        // Arbitrary non-null value. The type only carries the integer around and
        // never dereferences it, so no real kernel object is needed here.
        let handle = HANDLE(0x1234 as *mut core::ffi::c_void);
        assert_eq!(RawHandle::from_handle(handle).as_handle(), handle);
    }

    /// A null handle names no kernel object, so the cancel and close this test
    /// provokes both fail harmlessly instead of acting on something real.
    fn detached_pipe() -> Arc<PipeHandle> {
        PipeHandle::new(HANDLE::default())
    }

    #[test]
    fn issues_an_operation_while_live() {
        assert_eq!(detached_pipe().issue(|_handle| 7), Some(7));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Cancellation calls a real Windows API, which Miri cannot execute.
    fn refuses_an_operation_started_after_cancellation() {
        let pipe = detached_pipe();
        pipe.cancel();
        // Refusing is the point: `CancelIoEx` cannot reach an operation that
        // does not exist yet, so one started now would never be released.
        assert_eq!(pipe.issue(|_handle| 7), None);
    }
}
