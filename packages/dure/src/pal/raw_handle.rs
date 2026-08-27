//! Integer form of a Windows `HANDLE` so PAL tables can be `Send`.

use windows::Win32::Foundation::HANDLE;

/// Process-local handle stored as an integer.
///
/// `HANDLE` is a raw pointer and therefore `!Send`. Windows handles are
/// pointer-sized integers; storing that integer lets a mutex table be shared
/// across the supervisor's relay threads.
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
