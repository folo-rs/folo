//! Integer form of a Windows `HANDLE` so PAL tables can be `Send`.

use windows::Win32::Foundation::HANDLE;

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
}
