//! Win32 file replacement for the session store.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::core::PCWSTR;

/// Replace `dest` with `tmp`, committing the rename to disk before returning.
pub(crate) fn move_file_replace(tmp: &Path, dest: &Path) -> io::Result<()> {
    let src: Vec<u16> = tmp.as_os_str().encode_wide().chain([0]).collect();
    let dst: Vec<u16> = dest.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: `src` and `dst` are NUL-terminated paths in the same directory.
    unsafe {
        MoveFileExW(
            PCWSTR(src.as_ptr()),
            PCWSTR(dst.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| io::Error::other(error.message()))
}
